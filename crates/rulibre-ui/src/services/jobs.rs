//! UI identities remain stable while requests wait for core capacity/resources.
use rulibre_core::{
    ErrorKind,
    converter::Workspace,
    jobs::{JobEvent, JobExecutor, JobHandle, JobOutput, JobRequest, JobState, Stage, SubmitError},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, mpsc::TryRecvError},
};
pub const HISTORY_LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct JobId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Scan,
    Metadata,
    Backends,
    Convert,
    Send,
}
impl JobKind {
    fn of(request: &JobRequest) -> Self {
        match request {
            JobRequest::Scan { .. } => Self::Scan,
            JobRequest::Metadata { .. } => Self::Metadata,
            JobRequest::Backends => Self::Backends,
            JobRequest::Send { .. } => Self::Send,
            JobRequest::Convert { .. } | JobRequest::ConvertFiles { .. } => Self::Convert,
        }
    }
}
#[derive(Clone, Debug)]
pub struct JobView {
    pub id: JobId,
    pub title: String,
    pub kind: JobKind,
    pub state: JobState,
    pub progress: Option<(u64, u64)>,
    pub stage: Option<Stage>,
    pub error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct JobUpdate {
    pub view: JobView,
    pub result: Option<Arc<Result<JobOutput, String>>>,
}

trait Handle: Send {
    fn cancel(&self);
    fn events(&self) -> Vec<JobEvent>;
    fn result(&self) -> Option<(JobState, Result<JobOutput, String>)>;
}
impl Handle for JobHandle {
    fn cancel(&self) {
        self.cancel();
    }
    fn events(&self) -> Vec<JobEvent> {
        self.events.try_iter().collect()
    }
    fn result(&self) -> Option<(JobState, Result<JobOutput, String>)> {
        match self.try_result() {
            Ok(Ok(output)) => Some((JobState::Succeeded, Ok(output))),
            Ok(Err(error)) => Some((
                if matches!(error.kind, ErrorKind::Cancelled) {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                },
                Err(error_message(&error)),
            )),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some((JobState::Failed, Err("Job worker disconnected".into())))
            }
        }
    }
}
/// Human-readable diagnostics are built at the UI boundary, while categories
/// are still available. In particular a partial transfer retains its path.
pub fn error_message(error: &rulibre_core::Error) -> String {
    let path = error
        .path
        .as_ref()
        .map(|p| format!(": {}", p.display()))
        .unwrap_or_default();
    match &error.kind {
        ErrorKind::Cancelled => "Cancelled".into(),
        ErrorKind::Conflict => {
            format!("Destination already exists{path}. Existing file was not overwritten.")
        }
        ErrorKind::Io(source) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            format!("Permission denied{path}. Check folder permissions and try again.")
        }
        ErrorKind::PartialTransfer {
            cause,
            partial_path,
            ..
        } => format!(
            "Transfer interrupted. {} A partial file may remain at {}. Reconnect the reader and check this file before retrying.",
            error_message(cause),
            partial_path.display()
        ),
        _ => error.to_string(),
    }
}
trait Executor: Send {
    fn submit(&self, request: JobRequest) -> Result<Box<dyn Handle>, SubmitError>;
    fn shutdown(&mut self);
}
impl Executor for JobExecutor {
    fn submit(&self, request: JobRequest) -> Result<Box<dyn Handle>, SubmitError> {
        self.submit(request)
            .map(|handle| Box::new(handle) as Box<dyn Handle>)
    }
    fn shutdown(&mut self) {
        self.shutdown();
    }
}
struct Waiting {
    id: JobId,
    request: JobRequest,
    workspace: Option<Workspace>,
}
struct Running {
    handle: Box<dyn Handle>,
    workspace: Option<Workspace>,
}

pub struct Coordinator {
    executor: Box<dyn Executor>,
    next: u64,
    closed: bool,
    waiting: VecDeque<Waiting>,
    running: BTreeMap<JobId, Running>,
    views: BTreeMap<JobId, JobView>,
    finished: VecDeque<JobId>,
    ready: Vec<JobUpdate>,
    workspaces: Vec<Workspace>,
}
impl Coordinator {
    pub fn new(parallelism: usize, capacity: usize) -> std::io::Result<Self> {
        Ok(Self::with_executor(Box::new(JobExecutor::new(
            parallelism,
            capacity,
        )?)))
    }
    pub fn with_core_executor(executor: JobExecutor) -> Self {
        Self::with_executor(Box::new(executor))
    }
    fn with_executor(executor: Box<dyn Executor>) -> Self {
        Self {
            executor,
            next: 0,
            closed: false,
            waiting: VecDeque::new(),
            running: BTreeMap::new(),
            views: BTreeMap::new(),
            finished: VecDeque::new(),
            ready: Vec::new(),
            workspaces: Vec::new(),
        }
    }
    pub fn enqueue(
        &mut self,
        title: String,
        request: JobRequest,
        workspace: Option<Workspace>,
    ) -> JobId {
        self.next = self.next.checked_add(1).expect("job ID exhausted");
        let id = JobId(self.next);
        self.views.insert(
            id,
            JobView {
                id,
                title,
                kind: JobKind::of(&request),
                state: JobState::Waiting,
                progress: None,
                stage: None,
                error: None,
            },
        );
        if self.closed {
            self.finish(id, JobState::Failed, Err("Job executor is closed".into()));
        } else {
            self.waiting.push_back(Waiting {
                id,
                request,
                workspace,
            });
            self.submit_waiting();
        }
        id
    }
    fn finish(&mut self, id: JobId, state: JobState, result: Result<JobOutput, String>) {
        let view = self.views.get_mut(&id).expect("known job");
        view.state = state;
        view.error = result.as_ref().err().cloned();
        self.ready.push(JobUpdate {
            view: view.clone(),
            result: Some(Arc::new(result)),
        });
        self.finished.push_back(id);
        while self.finished.len() > HISTORY_LIMIT {
            if let Some(old) = self.finished.pop_front() {
                self.views.remove(&old);
            }
        }
    }
    fn submit_waiting(&mut self) {
        while let Some(job) = self.waiting.front() {
            // submit consumes its argument even on refusal. Keep the original
            // until acceptance so retry cannot lose or double-submit a request.
            match self.executor.submit(job.request.clone()) {
                Ok(handle) => {
                    let job = self.waiting.pop_front().unwrap();
                    self.running.insert(
                        job.id,
                        Running {
                            handle,
                            workspace: job.workspace,
                        },
                    );
                }
                Err(SubmitError::Full | SubmitError::Conflict) => break,
                Err(error) => {
                    let job = self.waiting.pop_front().unwrap();
                    let text = match error {
                        SubmitError::Closed => "Job executor is closed".into(),
                        SubmitError::InvalidDestination(reason) => {
                            format!("Invalid destination: {reason}")
                        }
                        _ => unreachable!(),
                    };
                    self.finish(job.id, JobState::Failed, Err(text));
                }
            }
        }
    }
    pub fn has_jobs(&self) -> bool {
        !self.waiting.is_empty() || !self.running.is_empty()
    }
    pub fn view(&self, id: JobId) -> Option<&JobView> {
        self.views.get(&id)
    }
    pub fn cancel(&mut self, id: JobId) {
        if let Some(index) = self.waiting.iter().position(|job| job.id == id) {
            self.waiting.remove(index);
            self.finish(id, JobState::Cancelled, Err("Cancelled".into()));
            self.submit_waiting();
        } else if let Some(job) = self.running.get(&id) {
            job.handle.cancel();
        }
    }
    pub fn poll(&mut self) -> Vec<JobUpdate> {
        let mut finished = Vec::new();
        for (&id, job) in &self.running {
            let view = self.views.get_mut(&id).unwrap();
            // Read result first, then drain events: terminal events are sent
            // before the result, so no last progress/stage event is lost.
            let result = job.handle.result();
            for event in job.handle.events() {
                match event {
                    JobEvent::State {
                        state: state @ (JobState::Waiting | JobState::Running),
                        ..
                    } => view.state = state,
                    JobEvent::State { .. } => {}
                    JobEvent::Stage { stage, .. } => view.stage = Some(stage),
                    JobEvent::CopyProgress {
                        completed, total, ..
                    } if view.kind == JobKind::Send => view.progress = Some((completed, total)),
                    JobEvent::CopyProgress { .. } => {}
                }
            }
            if let Some(result) = result {
                finished.push((id, result));
            }
        }
        for (id, (state, result)) in finished {
            let job = self.running.remove(&id).unwrap();
            if matches!(result, Ok(JobOutput::Conversion(_)))
                && let Some(workspace) = job.workspace
            {
                self.workspaces.push(workspace);
            }
            self.finish(id, state, result);
        }
        if !self.closed {
            self.submit_waiting();
        }
        let mut updates = std::mem::take(&mut self.ready);
        updates.extend(
            self.views
                .values()
                .filter(|view| {
                    self.waiting.iter().any(|job| job.id == view.id)
                        || self.running.contains_key(&view.id)
                })
                .cloned()
                .map(|view| JobUpdate { view, result: None }),
        );
        updates
    }
    pub fn clear_finished(&mut self) {
        self.finished.clear();
        self.views.retain(|id, _| {
            self.running.contains_key(id) || self.waiting.iter().any(|job| job.id == *id)
        });
    }
    pub fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let waiting: Vec<_> = self.waiting.drain(..).collect();
        for job in &waiting {
            self.finish(job.id, JobState::Cancelled, Err("Cancelled".into()));
        }
        for job in self.running.values() {
            job.handle.cancel();
        }
        self.executor.shutdown(); // joins workers before workspaces are removed
        drop(waiting);
        let updates = self.poll();
        self.ready.extend(updates);
        self.workspaces.clear();
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Script {
        refusals: VecDeque<Option<SubmitError>>,
        accepted: Vec<String>,
        handles: Vec<Arc<Mutex<ScriptHandle>>>,
        stopped: bool,
        workspace_paths: Vec<std::path::PathBuf>,
    }
    #[derive(Default)]
    struct ScriptHandle {
        events: Vec<JobEvent>,
        result: Option<(JobState, Result<JobOutput, String>)>,
        cancelled: bool,
    }
    struct FakeHandle(Arc<Mutex<ScriptHandle>>);
    impl Handle for FakeHandle {
        fn cancel(&self) {
            let mut handle = self.0.lock().unwrap();
            handle.cancelled = true;
            handle.result = Some((JobState::Cancelled, Err("Cancelled".into())));
        }
        fn events(&self) -> Vec<JobEvent> {
            std::mem::take(&mut self.0.lock().unwrap().events)
        }
        fn result(&self) -> Option<(JobState, Result<JobOutput, String>)> {
            self.0.lock().unwrap().result.take()
        }
    }
    struct FakeExecutor(Arc<Mutex<Script>>);
    impl Executor for FakeExecutor {
        fn submit(&self, request: JobRequest) -> Result<Box<dyn Handle>, SubmitError> {
            let mut script = self.0.lock().unwrap();
            if script.stopped {
                return Err(SubmitError::Closed);
            }
            if let Some(Some(error)) = script.refusals.pop_front() {
                return Err(error);
            }
            let name = match request {
                JobRequest::Metadata { book } | JobRequest::Send { book, .. } => {
                    book.to_string_lossy().into_owned()
                }
                _ => "other".into(),
            };
            script.accepted.push(name);
            let handle = Arc::new(Mutex::new(ScriptHandle::default()));
            script.handles.push(handle.clone());
            Ok(Box::new(FakeHandle(handle)))
        }
        fn shutdown(&mut self) {
            let mut script = self.0.lock().unwrap();
            assert!(script.workspace_paths.iter().all(|path| path.exists()));
            script.stopped = true;
        }
    }
    fn fake(refusals: Vec<Option<SubmitError>>) -> (Coordinator, Arc<Mutex<Script>>) {
        let script = Arc::new(Mutex::new(Script {
            refusals: refusals.into(),
            ..Default::default()
        }));
        (
            Coordinator::with_executor(Box::new(FakeExecutor(script.clone()))),
            script,
        )
    }
    fn add(jobs: &mut Coordinator, title: &str) -> JobId {
        jobs.enqueue(
            title.into(),
            JobRequest::Metadata { book: title.into() },
            None,
        )
    }
    fn complete(script: &Arc<Mutex<Script>>, index: usize) {
        script.lock().unwrap().handles[index].lock().unwrap().result =
            Some((JobState::Succeeded, Ok(JobOutput::Backends(vec![]))));
    }
    #[test]
    fn full_queue_retries_fifo_once_and_preserves_identity() {
        let (mut jobs, script) = fake(vec![None, Some(SubmitError::Full), Some(SubmitError::Full)]);
        let a = add(&mut jobs, "A");
        let b = add(&mut jobs, "B");
        let c = add(&mut jobs, "C");
        assert_eq!(jobs.view(b).unwrap().state, JobState::Waiting);
        assert_eq!(script.lock().unwrap().accepted, ["A"]);
        complete(&script, 0);
        let updates = jobs.poll();
        assert!(updates.iter().any(|u| u.view.id == a && u.result.is_some()));
        assert_eq!(script.lock().unwrap().accepted, ["A", "B", "C"]);
        complete(&script, 2);
        complete(&script, 1);
        let updates = jobs.poll();
        assert_eq!(
            updates
                .iter()
                .filter(|u| u.result.is_some())
                .map(|u| u.view.id)
                .collect::<Vec<_>>(),
            [b, c]
        );
        assert!(!jobs.has_jobs());
        assert!(jobs.poll().is_empty());
        assert_eq!(script.lock().unwrap().accepted, ["A", "B", "C"]);
    }
    #[test]
    fn conflict_waits_then_retries_after_completion() {
        let (mut jobs, script) = fake(vec![None, Some(SubmitError::Conflict)]);
        add(&mut jobs, "A");
        let b = add(&mut jobs, "B");
        assert_eq!(jobs.view(b).unwrap().state, JobState::Waiting);
        complete(&script, 0);
        jobs.poll();
        assert_eq!(script.lock().unwrap().accepted, ["A", "B"]);
    }
    #[test]
    fn cancel_waiting_never_submits_and_running_cancels_handle() {
        let (mut jobs, script) = fake(vec![None, Some(SubmitError::Full)]);
        let a = add(&mut jobs, "A");
        let b = add(&mut jobs, "B");
        jobs.cancel(b);
        jobs.cancel(a);
        let updates = jobs.poll();
        assert_eq!(
            updates
                .iter()
                .filter(|u| u.view.state == JobState::Cancelled && u.result.is_some())
                .count(),
            2
        );
        assert!(script.lock().unwrap().handles[0].lock().unwrap().cancelled);
        assert_eq!(script.lock().unwrap().accepted, ["A"]);
        assert!(!jobs.has_jobs());
    }
    #[test]
    fn completion_order_does_not_assume_submission_order() {
        let (mut jobs, script) = fake(vec![]);
        let a = add(&mut jobs, "A");
        let b = add(&mut jobs, "B");
        complete(&script, 1);
        let updates = jobs.poll();
        assert_eq!(
            updates
                .iter()
                .filter(|u| u.result.is_some())
                .map(|u| u.view.id)
                .collect::<Vec<_>>(),
            [b]
        );
        complete(&script, 0);
        assert_eq!(jobs.poll()[0].view.id, a);
    }
    #[test]
    fn shutdown_cancels_queued_jobs_and_is_idempotent() {
        let (mut jobs, script) = fake(vec![None, Some(SubmitError::Full)]);
        let a = add(&mut jobs, "A");
        let b = add(&mut jobs, "B");
        jobs.shutdown();
        jobs.shutdown();
        assert!(!jobs.has_jobs());
        assert!(script.lock().unwrap().stopped);
        assert_eq!(jobs.view(a).unwrap().state, JobState::Cancelled);
        assert_eq!(jobs.view(b).unwrap().state, JobState::Cancelled);
        assert_eq!(script.lock().unwrap().accepted, ["A"]);
        let c = add(&mut jobs, "C");
        assert_eq!(jobs.view(c).unwrap().state, JobState::Failed);
    }
    #[test]
    fn permanent_refusals_finish_instead_of_retrying_forever() {
        let (mut jobs, _) = fake(vec![
            Some(SubmitError::Closed),
            Some(SubmitError::InvalidDestination("missing directory".into())),
        ]);
        let a = add(&mut jobs, "A");
        let b = add(&mut jobs, "B");
        let updates = jobs.poll();
        assert_eq!(updates.len(), 2);
        assert!(!jobs.has_jobs());
        assert_eq!(
            jobs.view(a).unwrap().error.as_deref(),
            Some("Job executor is closed")
        );
        assert!(
            jobs.view(b)
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("missing directory")
        );
    }
    #[test]
    fn events_use_ui_identity_and_conversion_never_gets_percentage() {
        let (mut jobs, script) = fake(vec![]);
        let id = jobs.enqueue(
            "Convert".into(),
            JobRequest::ConvertFiles {
                library: "library".into(),
                output_dir: "export".into(),
                files: vec![],
                target: rulibre_core::book::Format::Epub,
            },
            None,
        );
        script.lock().unwrap().handles[0].lock().unwrap().events = vec![
            JobEvent::State {
                id: rulibre_core::jobs::JobId(999),
                state: JobState::Running,
            },
            JobEvent::Stage {
                id: rulibre_core::jobs::JobId(999),
                stage: Stage::Converting,
            },
            JobEvent::CopyProgress {
                id: rulibre_core::jobs::JobId(999),
                completed: 5,
                total: 10,
            },
        ];
        let update = jobs.poll().remove(0);
        assert_eq!(update.view.id, id);
        assert_eq!(update.view.state, JobState::Running);
        assert_eq!(update.view.stage, Some(Stage::Converting));
        assert_eq!(update.view.progress, None);
    }
    #[test]
    fn real_executor_reports_failure_once_and_shuts_down() {
        let dir = tempfile::tempdir().unwrap();
        let mut jobs = Coordinator::new(1, 1).unwrap();
        let id = jobs.enqueue(
            "Scan".into(),
            JobRequest::Scan {
                library: dir.path().join("missing"),
            },
            None,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let updates = jobs.poll();
            if let Some(update) = updates.iter().find(|u| u.result.is_some()) {
                assert_eq!(update.view.id, id);
                assert_eq!(update.view.state, JobState::Failed);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!jobs.has_jobs());
        assert!(jobs.poll().is_empty());
        jobs.shutdown();
    }
    #[test]
    fn terminal_event_before_result_keeps_polling_and_survives_clear_finished() {
        let (mut jobs, script) = fake(vec![]);
        let id = add(&mut jobs, "A");
        script.lock().unwrap().handles[0]
            .lock()
            .unwrap()
            .events
            .push(JobEvent::State {
                id: rulibre_core::jobs::JobId(99),
                state: JobState::Succeeded,
            });
        let updates = jobs.poll();
        assert!(updates[0].result.is_none());
        assert_eq!(updates[0].view.state, JobState::Waiting);
        jobs.clear_finished();
        assert!(jobs.view(id).is_some());
        complete(&script, 0);
        let updates = jobs.poll();
        assert_eq!(updates[0].view.state, JobState::Succeeded);
        assert!(updates[0].result.is_some());
    }
    #[test]
    fn send_progress_maps_only_real_copy_bytes() {
        let (mut jobs, script) = fake(vec![]);
        let id = add(&mut jobs, "A");
        jobs.views.get_mut(&id).unwrap().kind = JobKind::Send;
        script.lock().unwrap().handles[0]
            .lock()
            .unwrap()
            .events
            .push(JobEvent::CopyProgress {
                id: rulibre_core::jobs::JobId(99),
                completed: 42,
                total: 100,
            });
        assert_eq!(jobs.poll()[0].view.progress, Some((42, 100)));
    }
    #[test]
    fn shutdown_joins_before_dropping_running_and_waiting_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        std::fs::create_dir(&library).unwrap();
        let first = Workspace::in_directory(&library, dir.path()).unwrap();
        let second = Workspace::in_directory(&library, dir.path()).unwrap();
        let paths = vec![first.path().to_owned(), second.path().to_owned()];
        let (mut jobs, script) = fake(vec![None, Some(SubmitError::Full)]);
        script.lock().unwrap().workspace_paths = paths.clone();
        jobs.enqueue("A".into(), JobRequest::Backends, Some(first));
        jobs.enqueue("B".into(), JobRequest::Backends, Some(second));
        jobs.shutdown();
        assert!(paths.iter().all(|path| !path.exists()));
        let updates = jobs.poll();
        assert_eq!(updates.iter().filter(|u| u.result.is_some()).count(), 2);
    }
    fn send_request(n: usize) -> JobRequest {
        JobRequest::Send {
            book: format!("book{n}").into(),
            author: "Author".into(),
            device: rulibre_core::device::Device {
                id: rulibre_core::device::DeviceId("reader".into()),
                kind: rulibre_core::device::DeviceKind::Kobo,
                mount_point: "reader".into(),
                label: None,
                filesystem: "FAT32".into(),
                read_only: false,
                evidence: vec![],
            },
            options: Default::default(),
        }
    }
    #[test]
    fn three_sends_conflict_fifo_waiting_and_running_cancellation() {
        let (mut jobs, script) = fake(vec![
            None,
            Some(SubmitError::Conflict),
            Some(SubmitError::Conflict),
            Some(SubmitError::Conflict),
        ]);
        let ids: Vec<_> = (0..3)
            .map(|n| jobs.enqueue(format!("Book {n}"), send_request(n), None))
            .collect();
        assert_eq!(script.lock().unwrap().handles.len(), 1);
        assert_eq!(jobs.view(ids[1]).unwrap().state, JobState::Waiting);
        jobs.cancel(ids[1]);
        assert_eq!(jobs.view(ids[1]).unwrap().state, JobState::Cancelled);
        // Keep the third send blocked until the first handle finishes.
        complete(&script, 0);
        jobs.poll();
        assert_eq!(script.lock().unwrap().handles.len(), 2);
        assert_eq!(script.lock().unwrap().accepted, ["book0", "book2"]);
        jobs.cancel(ids[2]);
        let updates = jobs.poll();
        assert!(
            updates
                .iter()
                .any(|u| u.view.id == ids[2] && u.view.state == JobState::Cancelled)
        );
        assert!(!jobs.has_jobs());
    }
    #[test]
    fn error_categories_keep_conflict_permissions_and_partial_path_readable() {
        use rulibre_core::Error;
        let path = std::path::Path::new("reader/Book.epub");
        let conflict = Error {
            operation: "copy",
            path: Some(path.into()),
            kind: ErrorKind::Conflict,
        };
        assert!(error_message(&conflict).contains("Destination already exists: reader/Book.epub"));
        assert!(error_message(&conflict).contains("not overwritten"));
        let denied = Error::io(
            "copy",
            path,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(error_message(&denied).contains("Permission denied"));
        let partial = Error {
            operation: "copy",
            path: None,
            kind: ErrorKind::PartialTransfer {
                cause: Box::new(Error::io(
                    "copy",
                    path,
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )),
                partial_path: "reader/Book.epub.part".into(),
                cleanup_error: Box::new(denied),
            },
        };
        let message = error_message(&partial);
        assert!(message.contains("reader/Book.epub.part"));
        assert!(message.contains("Reconnect"));
        assert!(!message.contains("PartialTransfer"));
        assert_eq!(
            error_message(&Error {
                operation: "copy",
                path: None,
                kind: ErrorKind::Cancelled
            }),
            "Cancelled"
        );
    }
}
