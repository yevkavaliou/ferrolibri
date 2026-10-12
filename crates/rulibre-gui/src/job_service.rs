use crate::{
    commands::DisplayBook,
    types::{ConvertOk, JobId, JobReply, JobValue, ScanReport},
};
use rulibre_core::{
    converter::Workspace,
    jobs::{JobExecutor, JobHandle, JobOutput, JobRequest},
};
use std::{collections::BTreeMap, sync::Mutex};

struct Pending {
    handle: JobHandle,
    workspace: Option<Workspace>,
}
pub struct JobService {
    executor: Mutex<JobExecutor>,
    pending: Mutex<BTreeMap<u64, Pending>>,
    workspaces: Mutex<Vec<Workspace>>,
}
impl JobService {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            executor: Mutex::new(JobExecutor::new(2, 16)?),
            pending: Mutex::new(BTreeMap::new()),
            workspaces: Mutex::new(Vec::new()),
        })
    }
    pub fn submit(
        &self,
        request: JobRequest,
        workspace: Option<Workspace>,
    ) -> Result<JobId, String> {
        let mut pending = self.pending.lock().map_err(|e| e.to_string())?;
        if pending.len() >= 16 {
            return Err("Too many uncollected jobs".into());
        }
        let handle = self
            .executor
            .lock()
            .map_err(|e| e.to_string())?
            .submit(request)
            .map_err(|e| format!("{e:?}"))?;
        let id = JobId(handle.id.0);
        pending.insert(id.0, Pending { handle, workspace });
        Ok(id)
    }
    pub fn poll(&self, id: JobId) -> Result<JobReply, String> {
        let mut pending = self.pending.lock().map_err(|e| e.to_string())?;
        let job = pending.get(&id.0).ok_or("Unknown or collected job")?;
        let result = match job.handle.try_result() {
            Ok(result) => Some(result.map_err(|e| e.to_string()).and_then(encode_output)),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Job worker disconnected".into())),
        };
        let events = job
            .handle
            .events
            .try_iter()
            .map(|event| {
                // Both targets compile the same model; bridge the module identities.
                serde_json::from_value(serde_json::to_value(event).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        if result.is_some() {
            let mut job = pending.remove(&id.0).ok_or("Job disappeared")?;
            if matches!(result, Some(Ok(JobValue::Conversion(_)))) {
                if let Some(workspace) = job.workspace.take() {
                    self.workspaces
                        .lock()
                        .map_err(|e| e.to_string())?
                        .push(workspace);
                }
            }
        }
        Ok(JobReply { id, events, result })
    }
    pub fn cancel(&self, id: JobId) -> Result<(), String> {
        self.pending
            .lock()
            .map_err(|e| e.to_string())?
            .get(&id.0)
            .ok_or("Unknown job")?
            .handle
            .cancel();
        Ok(())
    }
    pub fn shutdown(&self) {
        if let Ok(mut executor) = self.executor.lock() {
            executor.shutdown();
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
        if let Ok(mut workspaces) = self.workspaces.lock() {
            workspaces.clear();
        }
    }
}
fn encode_output(output: JobOutput) -> Result<JobValue, String> {
    match output {
        JobOutput::Scan(report) => {
            let books = report
                .books
                .into_iter()
                .map(DisplayBook::from)
                .map(|book| {
                    serde_json::from_value(serde_json::to_value(book).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(JobValue::Scan(ScanReport {
                books,
                warnings: report.warnings.iter().map(ToString::to_string).collect(),
            }))
        }
        JobOutput::Conversion(result) => Ok(JobValue::Conversion(ConvertOk {
            message: format!(
                "Temporary result: {} (removed on exit)",
                result.output_path.display()
            ),
            output_path: result
                .output_path
                .into_os_string()
                .into_string()
                .map_err(|_| "Output path is not Unicode")?,
        })),
        JobOutput::Sent(path) => Ok(JobValue::Sent(format!("Sent to {}", path.display()))),
        _ => Err("Unexpected job output".into()),
    }
}

#[cfg(test)]
#[path = "../../rulibre-core/src/test_support.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    fn collect(service: &JobService, id: JobId) -> JobReply {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut events = Vec::new();
        loop {
            let mut reply = service.poll(id).unwrap();
            // Exercise exactly the payload consumed by WASM.
            reply = serde_json::from_value(serde_json::to_value(reply).unwrap()).unwrap();
            events.append(&mut reply.events);
            if reply.result.is_some() {
                reply.events = events;
                return reply;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn scan_success_and_backend_error_survive_transport() {
        let fixture = fixtures::Fixture::new();
        let service = JobService::new().unwrap();
        let id = service
            .submit(
                JobRequest::Scan {
                    library: fixture.library.clone(),
                },
                None,
            )
            .unwrap();
        let reply = collect(&service, id);
        match reply.result.unwrap().unwrap() {
            JobValue::Scan(report) => assert_eq!(report.books.len(), 2),
            _ => panic!("wrong output"),
        }
        assert!(service.poll(id).is_err());
        let id = service
            .submit(
                JobRequest::Scan {
                    library: fixture.root.join("missing"),
                },
                None,
            )
            .unwrap();
        assert!(collect(&service, id).result.unwrap().is_err());
        service.shutdown();
        assert!(
            service
                .submit(
                    JobRequest::Scan {
                        library: fixture.library.clone()
                    },
                    None
                )
                .is_err()
        );
    }
    #[test]
    fn synthetic_reader_failure_survives_transport_without_modifying_library() {
        let fixture = fixtures::Fixture::new();
        let before = fixtures::snapshot(&fixture.library);
        let before_device = fixtures::snapshot(&fixture.kobo);
        let service = JobService::new().unwrap();
        let id = service
            .submit(
                JobRequest::Send {
                    book: fixture.book.clone(),
                    author: "Автор Example".into(),
                    device: fixtures::device(rulibre_core::device::DeviceKind::Kobo, &fixture.kobo),
                    options: Default::default(),
                },
                None,
            )
            .unwrap();
        let reply = collect(&service, id);
        assert!(reply.result.unwrap().is_err());
        assert_eq!(before_device, fixtures::snapshot(&fixture.kobo));
        assert_eq!(before, fixtures::snapshot(&fixture.library));
    }
    #[test]
    fn failed_conversion_releases_its_workspace() {
        let fixture = fixtures::Fixture::new();
        let service = JobService::new().unwrap();
        let workspace = Workspace::new(&fixture.library).unwrap();
        let output_dir = workspace.path().to_owned();
        let id = service
            .submit(
                JobRequest::ConvertFiles {
                    library: fixture.library.clone(),
                    output_dir: output_dir.clone(),
                    files: vec![],
                    target: rulibre_core::book::Format::Pdf,
                },
                Some(workspace),
            )
            .unwrap();
        assert!(collect(&service, id).result.unwrap().is_err());
        assert!(!output_dir.exists());
    }
}
