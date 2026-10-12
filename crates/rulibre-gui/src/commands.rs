use std::path::{Path, PathBuf};

use rulibre_core::{
    config, converter,
    device::Device,
    metadata::{self, Metadata},
    scanner::{self, BookFile, BookId},
};
use serde::Serialize;

use super::DeviceWatchState;

#[tauri::command]
pub fn load_config() -> Result<Option<config::Config>, String> {
    config::Config::load().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_config(library_path: String) -> Result<(), String> {
    let library_path = config::sanitize_path(&library_path).map_err(|e| e.to_string())?;
    if !config::is_calibre_library(&library_path).map_err(|e| e.to_string())? {
        return Err("Not a valid Calibre library (missing metadata.db).".into());
    }
    config::Config { library_path }
        .save()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn validate_library(path: String) -> Result<String, String> {
    let sanitized = config::sanitize_path(&path).map_err(|e| e.to_string())?;
    let p = sanitized.as_path();
    if !p.is_dir() {
        return Err(format!("Path does not exist: {}", sanitized.display()));
    }
    if !config::is_calibre_library(p).map_err(|e| e.to_string())? {
        return Err("Not a valid Calibre library (missing metadata.db).".into());
    }
    sanitized
        .into_os_string()
        .into_string()
        .map_err(|_| "Library path is not valid Unicode for IPC.".into())
}

#[derive(Serialize)]
pub struct DisplayBook {
    pub id: BookId,
    pub author: String,
    pub title: String,
    pub formats: String,
    pub files: Vec<BookFile>,
    pub path: PathBuf,
}

fn formats_label(files: &[BookFile]) -> String {
    files
        .iter()
        .map(|f| f.format)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|f| f.label())
        .collect::<Vec<_>>()
        .join(", ")
}

impl From<scanner::Book> for DisplayBook {
    fn from(book: scanner::Book) -> Self {
        Self {
            formats: formats_label(&book.files),
            id: book.id,
            author: book.author,
            title: book.title,
            files: book.files,
            path: book.path,
        }
    }
}

#[tauri::command]
pub async fn scan_library(
    path: String,
    state: tauri::State<'_, crate::job_service::JobService>,
) -> Result<crate::types::JobId, String> {
    let valid_path = path.clone();
    let valid = tauri::async_runtime::spawn_blocking(move || {
        config::is_calibre_library(Path::new(&valid_path)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if !valid {
        return Err("Not a valid Calibre library (missing metadata.db).".into());
    }
    state.submit(
        rulibre_core::jobs::JobRequest::Scan {
            library: path.into(),
        },
        None,
    )
}
#[tauri::command]
pub async fn parse_metadata(book_path: String) -> Result<Option<Metadata>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        metadata::read_metadata(Path::new(&book_path))
            .map(Some)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn available_backends() -> Result<Vec<converter::Backend>, String> {
    tauri::async_runtime::spawn_blocking(converter::available_backends)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn target_formats(
    files: Vec<BookFile>,
    backends: Vec<converter::Backend>,
) -> Vec<(String, String)> {
    converter::target_formats(&files, &backends)
        .into_iter()
        .map(|(format, tool)| (format.label().to_owned(), tool))
        .collect()
}

#[cfg(test)]
#[derive(Serialize)]
pub struct ConvertOk {
    pub message: String,
    pub output_path: PathBuf,
}

#[tauri::command]
pub async fn convert_book(
    book_path: String,
    files: Vec<BookFile>,
    target: String,
    state: tauri::State<'_, crate::job_service::JobService>,
) -> Result<crate::types::JobId, String> {
    let target = scanner::Format::from_label(&target).ok_or("Unknown target format")?;
    let (library, workspace) = tauri::async_runtime::spawn_blocking(move || {
        let library = converter::library_root(Path::new(&book_path)).map_err(|e| e.to_string())?;
        let workspace = converter::Workspace::new(&library).map_err(|e| e.to_string())?;
        Ok::<_, String>((library, workspace))
    })
    .await
    .map_err(|e| e.to_string())??;
    state.submit(
        rulibre_core::jobs::JobRequest::ConvertFiles {
            library,
            output_dir: workspace.path().to_owned(),
            files,
            target,
        },
        Some(workspace),
    )
}
#[tauri::command]
pub async fn send_to_device(
    book_path: String,
    author: String,
    state: tauri::State<'_, DeviceWatchState>,
    jobs: tauri::State<'_, crate::job_service::JobService>,
) -> Result<crate::types::JobId, String> {
    let device = state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("No device connected")?;
    jobs.submit(
        rulibre_core::jobs::JobRequest::Send {
            book: book_path.into(),
            author,
            device,
            options: Default::default(),
        },
        None,
    )
}
#[tauri::command]
pub fn current_device(state: tauri::State<'_, DeviceWatchState>) -> Result<Option<Device>, String> {
    Ok(state.0.lock().map_err(|e| e.to_string())?.clone())
}
#[tauri::command]
pub fn poll_job(
    id: crate::types::JobId,
    state: tauri::State<'_, crate::job_service::JobService>,
) -> Result<crate::types::JobReply, String> {
    state.poll(id)
}
#[tauri::command]
pub fn cancel_job(
    id: crate::types::JobId,
    state: tauri::State<'_, crate::job_service::JobService>,
) -> Result<(), String> {
    state.cancel(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types as frontend_types;

    #[test]
    fn config_metadata_and_all_identity_variants_match_transport() {
        let config = config::Config {
            library_path: "C:/Автор/Library with spaces".into(),
        };
        let client: frontend_types::Config =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(client.library_path, config.library_path.to_str().unwrap());
        let metadata = metadata::Metadata {
            title: "Книга".into(),
            authors: vec!["Автор".into()],
            identifiers: vec![("ISBN".into(), "42".into())],
            ..Default::default()
        };
        let json = serde_json::to_value(metadata).unwrap();
        let client: frontend_types::Metadata = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(client).unwrap(), json);
        for id in [
            BookId::Calibre(42),
            BookId::RelativePath("Автор/Без ID".into()),
        ] {
            let json = serde_json::to_value(id).unwrap();
            let client: frontend_types::BookId = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(client).unwrap(), json);
        }
    }

    #[test]
    fn book_and_conversion_transport_match_frontend() {
        let files = vec![BookFile {
            format: scanner::Format::Epub,
            path: "Автор/Книга/book.EPUB".into(),
        }];
        let book = DisplayBook::from(scanner::Book {
            id: BookId::Calibre(42),
            author: "Автор".into(),
            title: "Книга".into(),
            path: "Автор/Книга".into(),
            files: files.clone(),
        });
        let frontend: frontend_types::Book =
            serde_json::from_value(serde_json::to_value(&book).unwrap()).unwrap();
        assert_eq!(frontend.formats, "EPUB");
        assert_eq!(
            serde_json::to_value(&frontend.id).unwrap(),
            serde_json::to_value(&book.id).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&frontend.files).unwrap(),
            serde_json::to_value(&files).unwrap()
        );
        let result = ConvertOk {
            message: "ok".into(),
            output_path: "temporary/book.pdf".into(),
        };
        let frontend: frontend_types::ConvertOk =
            serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap();
        assert_eq!(frontend.output_path, "temporary/book.pdf");
    }

    #[test]
    fn backend_transport_preserves_capabilities_and_diagnostics() {
        let backend = converter::Backend {
            kind: converter::BackendKind::EbookConvert,
            path: Some("C:/Tools/Calibre/ebook-convert.exe".into()),
            version: Some("calibre 8.16.2".into()),
            status: converter::BackendStatus::Ready,
            diagnostic: "KEPUB plugin unavailable".into(),
            kepub_output: false,
        };
        let frontend: frontend_types::Backend =
            serde_json::from_value(serde_json::to_value(&backend).unwrap()).unwrap();
        assert_eq!(frontend.kind, frontend_types::BackendKind::EbookConvert);
        assert_eq!(frontend.status, frontend_types::BackendStatus::Ready);
        assert_eq!(frontend.path, backend.path);
        assert_eq!(frontend.version, backend.version);
        assert_eq!(frontend.diagnostic, backend.diagnostic);
        assert!(!frontend.kepub_output);
    }

    #[test]
    fn device_event_transport_preserves_identity_path_and_evidence() {
        use rulibre_core::device::{DetectionEvidence, DeviceEvent, DeviceId, DeviceKind};
        let device = Device {
            id: DeviceId("Kobo:windows:volume-fixture".into()),
            kind: DeviceKind::Kobo,
            mount_point: "D:/Читалка renamed".into(),
            label: Some("MyReader".into()),
            filesystem: "FAT32".into(),
            read_only: false,
            evidence: vec![
                DetectionEvidence::KoboVersion,
                DetectionEvidence::KoboDatabase,
            ],
        };
        let frontend: frontend_types::DeviceEvent = serde_json::from_value(
            serde_json::to_value(DeviceEvent::Snapshot {
                devices: vec![device.clone()],
                selected_id: Some(device.id.clone()),
                changes: vec![rulibre_core::device_model::DeviceChange::Connected(
                    device.clone(),
                )],
                warnings: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        let frontend: frontend_types::Device = frontend.selected_device().unwrap().clone();
        assert_eq!(frontend.name(), "Kobo");
        assert_eq!(
            serde_json::to_value(frontend).unwrap(),
            serde_json::to_value(&device).unwrap()
        );
        let mut changed = device.clone();
        changed.mount_point = "E:/Reader".into();
        use rulibre_core::device_model::DeviceChange;
        let events = [
            DeviceEvent::Snapshot {
                devices: vec![changed.clone()],
                selected_id: Some(changed.id.clone()),
                changes: vec![
                    DeviceChange::Disconnected(DeviceId("old-reader".into())),
                    DeviceChange::Changed {
                        previous: device,
                        current: changed,
                    },
                ],
                warnings: vec!["unavailable volume".into()],
            },
            DeviceEvent::Snapshot {
                devices: vec![],
                selected_id: None,
                changes: vec![],
                warnings: vec![],
            },
            DeviceEvent::PollFailed {
                diagnostic: "temporary poll failure".into(),
            },
        ];
        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            let frontend: frontend_types::DeviceEvent =
                serde_json::from_value(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(&frontend).unwrap(), json);
            assert_eq!(frontend.message(), event.message());
            assert_eq!(
                serde_json::to_value(frontend.selected_device()).unwrap(),
                serde_json::to_value(event.selected_device()).unwrap()
            );
        }
    }
}
