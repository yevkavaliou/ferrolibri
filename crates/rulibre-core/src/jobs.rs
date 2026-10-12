//! UI-independent, bounded background execution. See docs/BACKGROUND_JOBS.md.
use crate::{
    Error, ErrorKind,
    book::Format,
    converter::{self, CancellationToken, ConversionResult},
    device::{self, Device, PreparationOptions},
    scanner::{self, ScanReport},
};
use std::{
    collections::{BTreeMap, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub use crate::job_model::{JobEvent, JobId, JobState, Stage};

pub enum JobRequest {
    Metadata {
        book: PathBuf,
    },
    Backends,
    ConvertFiles {
        library: PathBuf,
        output_dir: PathBuf,
        files: Vec<crate::book::BookFile>,
        target: Format,
    },
    Scan {
        library: PathBuf,
    },
    /// The output directory must already exist and be outside the library.
    Convert {
        library: PathBuf,
        output_dir: PathBuf,
        source: PathBuf,
        target: Format,
    },
    Send {
        book: PathBuf,
        author: String,
        device: Device,
        options: PreparationOptions,
    },
}

#[derive(Debug)]
pub enum JobOutput {
    Metadata(crate::metadata::Metadata),
    Backends(Vec<converter::Backend>),
    Scan(ScanReport),
    Conversion(ConversionResult),
    Sent(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Closed,
    Full,
    Conflict,
    InvalidDestination(String),
}

type Task = Box<dyn FnOnce(&Context) -> Result<JobOutput, Error> + Send>;

struct Entry {
    id: JobId,
    task: Task,
    cancel: CancellationToken,
    events: mpsc::Sender<JobEvent>,
    result: mpsc::Sender<Result<JobOutput, Error>>,
}

#[derive(Clone)]
enum Resource {
    Path(PathBuf),
    Device(crate::device::DeviceId),
}
impl Resource {
    fn conflicts(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Path(a), Self::Path(b)) => a.starts_with(b) || b.starts_with(a),
            (Self::Device(a), Self::Device(b)) => a == b,
            _ => false,
        }
    }
}
struct Reservation {
    keys: Vec<Resource>,
    cancel: CancellationToken,
}
struct Queue {
    closed: bool,
    next: u64,
    pending: VecDeque<Entry>,
    reservations: BTreeMap<JobId, Reservation>,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

/// Owns all workers. Drop cancels pending/running jobs and joins every worker.
pub struct JobExecutor {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
    capacity: usize,
}

/// Dropping a handle does not cancel the operation. Call cancel explicitly.
pub struct JobHandle {
    pub id: JobId,
    pub events: mpsc::Receiver<JobEvent>,
    result: mpsc::Receiver<Result<JobOutput, Error>>,
    shared: Arc<Shared>,
}

fn cancelled() -> Error {
    Error {
        operation: "execute background job",
        path: None,
        kind: ErrorKind::Cancelled,
    }
}

impl JobHandle {
    /// Pending cancellation removes the job immediately, releasing its reservation.
    pub fn cancel(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        if let Some(reservation) = queue.reservations.get(&self.id) {
            reservation.cancel.cancel();
        }
        if let Some(index) = queue.pending.iter().position(|entry| entry.id == self.id) {
            let entry = queue.pending.remove(index).unwrap();
            queue.reservations.remove(&self.id);
            finish(entry, Err(cancelled()));
        }
    }

    /// Poll from the UI event loop; never blocks.
    pub fn try_result(&self) -> Result<Result<JobOutput, Error>, mpsc::TryRecvError> {
        self.result.try_recv()
    }

    /// Blocking convenience for CLI/tests. Do not call from a UI event loop.
    pub fn wait(self) -> Result<JobOutput, Error> {
        self.result
            .recv()
            .unwrap_or_else(|_| Err(Error::unavailable("receive job result", None)))
    }
}

struct Context {
    id: JobId,
    cancel: CancellationToken,
    events: mpsc::Sender<JobEvent>,
}
impl Context {
    fn check(&self) -> Result<(), Error> {
        if self.cancel.is_cancelled() {
            Err(cancelled())
        } else {
            Ok(())
        }
    }
    fn stage(&self, stage: Stage) {
        let _ = self.events.send(JobEvent::Stage { id: self.id, stage });
    }

    fn copy_progress(&self) -> impl FnMut(u64, u64) + '_ {
        let mut last = Instant::now() - Duration::from_secs(1);
        move |completed, total| {
            if completed == total || last.elapsed() >= Duration::from_millis(100) {
                let _ = self.events.send(JobEvent::CopyProgress {
                    id: self.id,
                    completed,
                    total,
                });
                last = Instant::now();
            }
        }
    }
}

impl JobExecutor {
    /// capacity limits all accepted unfinished jobs, including running jobs.
    pub fn new(parallelism: usize, capacity: usize) -> std::io::Result<Self> {
        if parallelism == 0 || capacity == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "job limits must be positive",
            ));
        }
        let mut executor = Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue {
                    closed: false,
                    next: 1,
                    pending: VecDeque::new(),
                    reservations: BTreeMap::new(),
                }),
                ready: Condvar::new(),
            }),
            workers: Vec::new(),
            capacity,
        };
        for index in 0..parallelism {
            let shared = executor.shared.clone();
            executor.workers.push(
                thread::Builder::new()
                    .name(format!("rulibre-job-{index}"))
                    .spawn(move || worker(shared))?,
            );
        }
        Ok(executor)
    }

    pub fn submit(&self, request: JobRequest) -> Result<JobHandle, SubmitError> {
        // Reserve whole destination directories conservatively, including aliases.
        // Only path resolution happens here; scans/processes/copying run on workers.
        let keys = match &request {
            JobRequest::Scan { .. } | JobRequest::Metadata { .. } | JobRequest::Backends => vec![],
            JobRequest::ConvertFiles { output_dir, .. } => vec![destination_key(output_dir)?],
            JobRequest::Convert { output_dir, .. } => vec![destination_key(output_dir)?],
            JobRequest::Send { device, .. } => vec![
                Resource::Device(device.id.clone()),
                destination_key(&device.mount_point)?,
            ],
        };
        self.enqueue(
            keys,
            Box::new(move |ctx| match request {
                JobRequest::Metadata { book } => {
                    ctx.check()?;
                    let metadata = crate::metadata::read_metadata(&book)?;
                    ctx.check()?;
                    Ok(JobOutput::Metadata(metadata))
                }
                JobRequest::Backends => {
                    let backends = converter::available_backends_cancellable(&ctx.cancel);
                    ctx.check()?;
                    Ok(JobOutput::Backends(backends))
                }
                JobRequest::ConvertFiles {
                    library,
                    output_dir,
                    files,
                    target,
                } => {
                    ctx.stage(Stage::Converting);
                    let backends = converter::available_backends_cancellable(&ctx.cancel);
                    ctx.check()?;
                    let plan = converter::plan_conversion(&files, target, &backends)?;
                    converter::execute_plan(&library, &output_dir, &plan, &ctx.cancel)
                        .map(JobOutput::Conversion)
                }
                JobRequest::Scan { library } => {
                    ctx.stage(Stage::Scanning);
                    scanner::scan_library_cancellable(&library, &ctx.cancel).map(JobOutput::Scan)
                }
                JobRequest::Convert {
                    library,
                    output_dir,
                    source,
                    target,
                } => {
                    ctx.stage(Stage::Converting);
                    converter::convert_cancellable(
                        &library,
                        &output_dir,
                        &source,
                        target,
                        &ctx.cancel,
                    )
                    .map(JobOutput::Conversion)
                }
                JobRequest::Send {
                    book,
                    author,
                    device,
                    options,
                } => {
                    ctx.stage(Stage::Preparing);
                    crate::transfer::validate_device(&device)?;
                    let file =
                        device::prepare_file_cancellable(&book, &device, options, &ctx.cancel)?;
                    let library = converter::library_root(&book)?;
                    converter::validate_output_dir(&library, &device.mount_point)?;
                    ctx.check()?;
                    ctx.stage(Stage::Copying);
                    crate::transfer::transfer_file(
                        &file.path,
                        &device,
                        &author,
                        &ctx.cancel,
                        ctx.copy_progress(),
                    )
                    .map(JobOutput::Sent)
                }
            }),
        )
    }

    fn enqueue(&self, keys: Vec<Resource>, task: Task) -> Result<JobHandle, SubmitError> {
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.closed {
            return Err(SubmitError::Closed);
        }
        if queue.reservations.values().any(|r| {
            r.keys
                .iter()
                .any(|key| keys.iter().any(|other| key.conflicts(other)))
        }) {
            return Err(SubmitError::Conflict);
        }
        if queue.reservations.len() >= self.capacity {
            return Err(SubmitError::Full);
        }
        let id = JobId(queue.next);
        queue.next = queue.next.checked_add(1).ok_or(SubmitError::Full)?;
        let cancel = CancellationToken::default();
        let (events, receiver) = mpsc::channel();
        let (result, output) = mpsc::channel();
        let _ = events.send(JobEvent::State {
            id,
            state: JobState::Waiting,
        });
        queue.reservations.insert(
            id,
            Reservation {
                keys,
                cancel: cancel.clone(),
            },
        );
        queue.pending.push_back(Entry {
            id,
            task,
            cancel,
            events,
            result,
        });
        self.shared.ready.notify_one();
        Ok(JobHandle {
            id,
            events: receiver,
            result: output,
            shared: self.shared.clone(),
        })
    }

    pub fn shutdown(&mut self) {
        {
            let mut queue = self.shared.queue.lock().unwrap();
            queue.closed = true;
            for reservation in queue.reservations.values() {
                reservation.cancel.cancel();
            }
            while let Some(entry) = queue.pending.pop_front() {
                queue.reservations.remove(&entry.id);
                finish(entry, Err(cancelled()));
            }
            self.shared.ready.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for JobExecutor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn destination_key(path: &std::path::Path) -> Result<Resource, SubmitError> {
    let resolved =
        std::fs::canonicalize(path).map_err(|e| SubmitError::InvalidDestination(e.to_string()))?;
    // Preserve native encoding rather than collapsing different non-Unicode paths.
    #[cfg(windows)]
    let resolved = {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<u16> = resolved
            .as_os_str()
            .encode_wide()
            .map(|c| if (65..=90).contains(&c) { c + 32 } else { c })
            .collect();
        PathBuf::from(std::ffi::OsString::from_wide(&wide))
    };
    Ok(Resource::Path(resolved))
}

fn finish(entry: Entry, result: Result<JobOutput, Error>) {
    let state = match &result {
        Ok(_) => JobState::Succeeded,
        Err(error) if matches!(error.kind, ErrorKind::Cancelled) => JobState::Cancelled,
        Err(_) => JobState::Failed,
    };
    let _ = entry.events.send(JobEvent::State {
        id: entry.id,
        state,
    });
    let _ = entry.result.send(result);
}

fn worker(shared: Arc<Shared>) {
    loop {
        let mut entry = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if let Some(entry) = queue.pending.pop_front() {
                    break entry;
                }
                if queue.closed {
                    return;
                }
                queue = shared.ready.wait(queue).unwrap();
            }
        };
        let _ = entry.events.send(JobEvent::State {
            id: entry.id,
            state: JobState::Running,
        });
        let ctx = Context {
            id: entry.id,
            cancel: entry.cancel.clone(),
            events: entry.events.clone(),
        };
        let task = std::mem::replace(&mut entry.task, Box::new(|_| unreachable!()));
        let result = catch_unwind(AssertUnwindSafe(|| {
            ctx.check()?;
            task(&ctx)
        }))
        .unwrap_or_else(|_| {
            Err(Error {
                operation: "background worker panicked",
                path: None,
                kind: ErrorKind::InvalidData("worker panicked".into()),
            })
        });
        // A successful publication wins over a late cancellation request.
        // Never report a committed transfer as cancelled.
        let mut queue = shared.queue.lock().unwrap();
        queue.reservations.remove(&entry.id);
        finish(entry, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, snapshot};
    use std::sync::atomic::{AtomicBool, Ordering};

    const TIMEOUT: Duration = Duration::from_secs(5);
    fn ok() -> Result<JobOutput, Error> {
        Ok(JobOutput::Sent("test-output".into()))
    }
    fn terminal(handle: &JobHandle) -> Vec<JobEvent> {
        let mut events = Vec::new();
        loop {
            let event = handle.events.recv_timeout(TIMEOUT).unwrap();
            let done = matches!(
                event,
                JobEvent::State {
                    state: JobState::Succeeded | JobState::Failed | JobState::Cancelled,
                    ..
                }
            );
            events.push(event);
            if done {
                return events;
            }
        }
    }
    fn blocker(executor: &JobExecutor, keys: Vec<Resource>) -> (JobHandle, mpsc::Sender<()>) {
        let (started, start) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let handle = executor
            .enqueue(
                keys,
                Box::new(move |_| {
                    started.send(thread::current().id()).unwrap();
                    gate.recv_timeout(TIMEOUT).unwrap();
                    ok()
                }),
            )
            .unwrap();
        assert_ne!(start.recv_timeout(TIMEOUT).unwrap(), thread::current().id());
        (handle, release)
    }

    #[test]
    fn metadata_job_returns_typed_data_errors_and_supports_queued_cancellation() {
        let fixture = Fixture::new();
        let executor = JobExecutor::new(1, 4).unwrap();
        let handle = executor
            .submit(JobRequest::Metadata {
                book: fixture.book.clone(),
            })
            .unwrap();
        let JobOutput::Metadata(metadata) = handle.wait().unwrap() else {
            panic!("wrong result")
        };
        assert_eq!(metadata.title, "A small book");
        std::fs::write(fixture.book.join("metadata.opf"), b"broken").unwrap();
        let handle = executor
            .submit(JobRequest::Metadata {
                book: fixture.book.clone(),
            })
            .unwrap();
        assert!(handle.wait().is_err());
        let (block, release) = blocker(&executor, vec![]);
        let queued = executor
            .submit(JobRequest::Metadata {
                book: fixture.book.clone(),
            })
            .unwrap();
        queued.cancel();
        assert!(matches!(
            queued.wait().unwrap_err().kind,
            ErrorKind::Cancelled
        ));
        release.send(()).unwrap();
        block.wait().unwrap();
    }

    #[test]
    fn scan_runs_on_worker_and_reports_ordered_states() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.library);
        let executor = JobExecutor::new(2, 4).unwrap();
        let handle = executor
            .submit(JobRequest::Scan {
                library: fixture.library.clone(),
            })
            .unwrap();
        let id = handle.id;
        assert_eq!(
            terminal(&handle),
            vec![
                JobEvent::State {
                    id,
                    state: JobState::Waiting
                },
                JobEvent::State {
                    id,
                    state: JobState::Running
                },
                JobEvent::Stage {
                    id,
                    stage: Stage::Scanning
                },
                JobEvent::State {
                    id,
                    state: JobState::Succeeded
                },
            ]
        );
        let JobOutput::Scan(report) = handle.wait().unwrap() else {
            panic!("wrong result")
        };
        assert_eq!(report.books.len(), 2);
        assert_eq!(before, snapshot(&fixture.library));
    }

    #[test]
    fn fifo_queue_is_bounded_and_does_not_block_caller() {
        let executor = JobExecutor::new(1, 3).unwrap();
        let (first, release) = blocker(&executor, vec![]);
        let (order, received) = mpsc::channel();
        let second_order = order.clone();
        let second = executor
            .enqueue(
                vec![],
                Box::new(move |_| {
                    second_order.send(2).unwrap();
                    ok()
                }),
            )
            .unwrap();
        let third = executor
            .enqueue(
                vec![],
                Box::new(move |_| {
                    order.send(3).unwrap();
                    ok()
                }),
            )
            .unwrap();
        assert!(first.id < second.id && second.id < third.id);
        assert!(matches!(
            executor.enqueue(vec![], Box::new(|_| ok())),
            Err(SubmitError::Full)
        ));
        assert!(matches!(
            second.try_result(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        first.wait().unwrap();
        second.wait().unwrap();
        third.wait().unwrap();
        assert_eq!(received.iter().collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn parallelism_is_limited_but_independent_jobs_can_overlap() {
        let executor = JobExecutor::new(2, 3).unwrap();
        let (first, a) = blocker(&executor, vec![]);
        let (second, b) = blocker(&executor, vec![]);
        let third = executor.enqueue(vec![], Box::new(|_| ok())).unwrap();
        assert!(matches!(third.try_result(), Err(mpsc::TryRecvError::Empty)));
        a.send(()).unwrap();
        b.send(()).unwrap();
        first.wait().unwrap();
        second.wait().unwrap();
        third.wait().unwrap();
    }

    #[test]
    fn destinations_aliases_and_descendants_conflict_until_completion() {
        let fixture = Fixture::new();
        let executor = JobExecutor::new(2, 4).unwrap();
        let (first, release) = blocker(&executor, vec![destination_key(&fixture.root).unwrap()]);
        for path in [&fixture.root.join("."), &fixture.kobo] {
            let request = JobRequest::Convert {
                library: fixture.library.clone(),
                output_dir: path.clone(),
                source: fixture.book.join("A small book.epub"),
                target: Format::Pdf,
            };
            assert!(matches!(
                executor.submit(request),
                Err(SubmitError::Conflict)
            ));
        }
        release.send(()).unwrap();
        first.wait().unwrap();
        let next = executor
            .enqueue(
                vec![destination_key(&fixture.root).unwrap()],
                Box::new(|_| ok()),
            )
            .unwrap();
        next.wait().unwrap();
    }

    #[test]
    fn cancelling_waiting_job_releases_reservation_without_running_it() {
        let executor = JobExecutor::new(1, 2).unwrap();
        let (first, release) = blocker(&executor, vec![]);
        let key = Resource::Device(crate::device::DeviceId("reader".into()));
        let pending = executor
            .enqueue(vec![key.clone()], Box::new(|_| panic!("cancelled job ran")))
            .unwrap();
        assert!(matches!(
            executor.enqueue(vec![key.clone()], Box::new(|_| ok())),
            Err(SubmitError::Conflict)
        ));
        pending.cancel();
        pending.cancel();
        assert_eq!(
            terminal(&pending),
            vec![
                JobEvent::State {
                    id: pending.id,
                    state: JobState::Waiting
                },
                JobEvent::State {
                    id: pending.id,
                    state: JobState::Cancelled
                },
            ]
        );
        assert!(matches!(
            pending.wait().unwrap_err().kind,
            ErrorKind::Cancelled
        ));
        let replacement = executor.enqueue(vec![key], Box::new(|_| ok())).unwrap();
        release.send(()).unwrap();
        first.wait().unwrap();
        replacement.wait().unwrap();
    }

    fn cancellable(executor: &JobExecutor, ended: Arc<AtomicBool>) -> JobHandle {
        let (started, start) = mpsc::channel();
        let handle = executor
            .enqueue(
                vec![],
                Box::new(move |ctx| {
                    started.send(()).unwrap();
                    let deadline = Instant::now() + TIMEOUT;
                    while !ctx.cancel.is_cancelled() && Instant::now() < deadline {
                        thread::park_timeout(Duration::from_millis(1));
                    }
                    ended.store(true, Ordering::SeqCst);
                    ctx.check()?;
                    panic!("cancellation was not delivered")
                }),
            )
            .unwrap();
        start.recv_timeout(TIMEOUT).unwrap();
        handle
    }

    #[test]
    fn running_cancellation_uses_operation_token() {
        let executor = JobExecutor::new(1, 2).unwrap();
        let ended = Arc::new(AtomicBool::new(false));
        let handle = cancellable(&executor, ended.clone());
        handle.cancel();
        assert!(matches!(
            terminal(&handle).last(),
            Some(JobEvent::State {
                state: JobState::Cancelled,
                ..
            })
        ));
        assert!(matches!(
            handle.wait().unwrap_err().kind,
            ErrorKind::Cancelled
        ));
        assert!(ended.load(Ordering::SeqCst));
    }

    #[test]
    fn drop_cancels_queue_and_joins_running_workers() {
        let executor = JobExecutor::new(1, 2).unwrap();
        let ended = Arc::new(AtomicBool::new(false));
        let active = cancellable(&executor, ended.clone());
        let pending = executor
            .enqueue(vec![], Box::new(|_| panic!("shutdown ran pending task")))
            .unwrap();
        drop(executor);
        assert!(ended.load(Ordering::SeqCst));
        for handle in [active, pending] {
            assert!(matches!(
                handle.wait().unwrap_err().kind,
                ErrorKind::Cancelled
            ));
        }
    }

    #[test]
    fn shutdown_is_idempotent_and_rejects_new_work() {
        let mut executor = JobExecutor::new(2, 4).unwrap();
        executor.shutdown();
        executor.shutdown();
        assert!(matches!(
            executor.submit(JobRequest::Scan {
                library: "missing".into()
            }),
            Err(SubmitError::Closed)
        ));
        assert!(JobExecutor::new(0, 1).is_err());
        assert!(JobExecutor::new(1, 0).is_err());
    }

    #[test]
    fn failure_and_panic_produce_one_terminal_state_and_worker_survives() {
        let executor = JobExecutor::new(1, 2).unwrap();
        let failure = executor
            .submit(JobRequest::Scan {
                library: "missing-library".into(),
            })
            .unwrap();
        assert!(matches!(
            terminal(&failure).last(),
            Some(JobEvent::State {
                state: JobState::Failed,
                ..
            })
        ));
        assert!(matches!(failure.wait().unwrap_err().kind, ErrorKind::Io(_)));
        let panic = executor
            .enqueue(vec![], Box::new(|_| panic!("injected worker failure")))
            .unwrap();
        assert!(matches!(
            terminal(&panic).last(),
            Some(JobEvent::State {
                state: JobState::Failed,
                ..
            })
        ));
        assert_eq!(
            panic.wait().unwrap_err().operation,
            "background worker panicked"
        );
        executor
            .enqueue(vec![], Box::new(|_| ok()))
            .unwrap()
            .wait()
            .unwrap();
    }

    #[test]
    fn successful_committed_result_wins_over_late_cancellation() {
        let executor = JobExecutor::new(1, 1).unwrap();
        let (handle, release) = blocker(&executor, vec![]);
        handle.cancel();
        release.send(()).unwrap();
        assert!(matches!(
            terminal(&handle).last(),
            Some(JobEvent::State {
                state: JobState::Succeeded,
                ..
            })
        ));
        handle.wait().unwrap();
    }

    #[test]
    fn copy_events_report_actual_bytes_and_complete_before_success() {
        let fixture = Fixture::new();
        let source = fixture.book.join("A small book.epub");
        let data = vec![17u8; 300_000];
        std::fs::write(&source, &data).unwrap();
        let device = crate::test_support::device(device::DeviceKind::Kobo, &fixture.kobo);
        let executor = JobExecutor::new(1, 1).unwrap();
        let handle = executor
            .enqueue(
                vec![],
                Box::new(move |ctx| {
                    ctx.stage(Stage::Copying);
                    crate::transfer::transfer_fixture_cancellable(
                        &source,
                        &device,
                        "Author",
                        &ctx.cancel,
                        ctx.copy_progress(),
                    )
                    .map(JobOutput::Sent)
                }),
            )
            .unwrap();
        let events = terminal(&handle);
        let bytes: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                JobEvent::CopyProgress {
                    id,
                    completed,
                    total,
                } => {
                    assert_eq!(*id, handle.id);
                    Some((*completed, *total))
                }
                _ => None,
            })
            .collect();
        assert!(!bytes.is_empty());
        assert!(bytes.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert!(
            bytes
                .iter()
                .all(|(done, total)| done <= total && *total == data.len() as u64)
        );
        assert_eq!(bytes.last(), Some(&(data.len() as u64, data.len() as u64)));
        let JobOutput::Sent(path) = handle.wait().unwrap() else {
            panic!("wrong result")
        };
        assert_eq!(std::fs::read(path).unwrap(), data);
    }

    #[test]
    fn cancellation_reaches_copy_loop_and_cleans_partial_file() {
        let fixture = Fixture::new();
        let source = fixture.book.join("A small book.epub");
        std::fs::write(&source, vec![23u8; 300_000]).unwrap();
        let before = snapshot(&fixture.library);
        let device = crate::test_support::device(device::DeviceKind::Kobo, &fixture.kobo);
        let executor = JobExecutor::new(1, 1).unwrap();
        let (started, start) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let handle = executor
            .enqueue(
                vec![],
                Box::new(move |ctx| {
                    let mut progress = ctx.copy_progress();
                    let mut first = true;
                    crate::transfer::transfer_fixture_cancellable(
                        &source,
                        &device,
                        "Author",
                        &ctx.cancel,
                        |done, total| {
                            progress(done, total);
                            if first {
                                first = false;
                                started.send(()).unwrap();
                                gate.recv_timeout(TIMEOUT).unwrap();
                            }
                        },
                    )
                    .map(JobOutput::Sent)
                }),
            )
            .unwrap();
        start.recv_timeout(TIMEOUT).unwrap();
        handle.cancel();
        release.send(()).unwrap();
        assert!(matches!(
            terminal(&handle).last(),
            Some(JobEvent::State {
                state: JobState::Cancelled,
                ..
            })
        ));
        assert!(matches!(
            handle.wait().unwrap_err().kind,
            ErrorKind::Cancelled
        ));
        assert!(
            std::fs::read_dir(fixture.kobo.join("Author"))
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(snapshot(&fixture.library), before);
    }

    #[test]
    fn conversion_stage_is_indeterminate_and_preserves_typed_error() {
        let fixture = Fixture::new();
        let executor = JobExecutor::new(1, 1).unwrap();
        let handle = executor
            .submit(JobRequest::Convert {
                library: fixture.library.clone(),
                output_dir: fixture.root.clone(),
                source: fixture.book.join("cover.jpg"),
                target: Format::Epub,
            })
            .unwrap();
        let events = terminal(&handle);
        assert!(events.contains(&JobEvent::Stage {
            id: handle.id,
            stage: Stage::Converting
        }));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, JobEvent::CopyProgress { .. }))
        );
        assert!(matches!(
            handle.wait().unwrap_err().kind,
            ErrorKind::InvalidInput
        ));
    }
}
