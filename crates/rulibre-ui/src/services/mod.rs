//! The only boundary that performs IO. Views consume data and emit messages.
pub mod covers;
pub mod devices;
pub mod jobs;
use crate::settings::UiSettings;
use jobs::{Coordinator, JobId, JobUpdate};
use rulibre_core::{
    book::{BookFile, Format},
    converter::Workspace,
    cover::CoverFile,
    device::{Device, PreparationOptions},
    jobs::JobRequest,
};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};

pub type ServiceFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;
pub trait Services: Send + Sync {
    fn open_folder(&self, path: PathBuf) -> ServiceFuture<()> {
        blocking(move || {
            #[cfg(target_os = "windows")]
            let mut command = std::process::Command::new("explorer.exe");
            #[cfg(target_os = "macos")]
            let mut command = std::process::Command::new("open");
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            let mut command = std::process::Command::new("xdg-open");
            command
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
    fn device_subscription(&self) -> iced::Subscription<devices::DeviceUpdate> {
        iced::Subscription::none()
    }
    fn validate_reader(
        &self,
        _path: PathBuf,
        _kind: rulibre_core::device::DeviceKind,
    ) -> ServiceFuture<Device> {
        Box::pin(async { Err("Reader validation unavailable".into()) })
    }
    fn remember_reader(&self, _reader: &Device) -> u64 {
        0
    }
    fn load_thumbnail(
        &self,
        _id: rulibre_core::book::BookId,
        _book: PathBuf,
    ) -> ServiceFuture<Option<iced::widget::image::Handle>> {
        Box::pin(async { Ok(None) })
    }
    fn enqueue(&self, title: String, request: JobRequest) -> JobId;
    fn poll(&self) -> Vec<JobUpdate>;
    fn cancel(&self, id: JobId);
    fn clear_finished(&self);
    fn shutdown(&self);
    fn load_settings(&self) -> ServiceFuture<UiSettings>;
    fn save_settings(&self, settings: UiSettings) -> ServiceFuture<()>;
    fn find_cover(&self, book: PathBuf) -> ServiceFuture<Option<CoverFile>>;
    fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>>;
    fn load_library(&self) -> ServiceFuture<Option<PathBuf>>;
    fn save_library(&self, path: PathBuf) -> ServiceFuture<PathBuf>;
    fn validate_library(&self, input: String) -> ServiceFuture<PathBuf>;
    fn validate_export(&self, library: PathBuf, output: PathBuf) -> ServiceFuture<PathBuf> {
        blocking(move || {
            rulibre_core::converter::validate_output_dir(&library, &output).map_err(|e| {
                format!("Choose an existing writable export folder outside Calibre libraries. {e}")
            })
        })
    }
    fn scan(&self, library: PathBuf) -> JobId {
        self.enqueue("Scan library".into(), JobRequest::Scan { library })
    }
    fn read_metadata(&self, book: PathBuf) -> JobId {
        self.enqueue("Read metadata".into(), JobRequest::Metadata { book })
    }
    fn list_backends(&self) -> JobId {
        self.enqueue("Find converters".into(), JobRequest::Backends)
    }
    fn convert(
        &self,
        title: String,
        library: PathBuf,
        output_dir: PathBuf,
        files: Vec<BookFile>,
        target: Format,
    ) -> JobId {
        self.enqueue(
            title,
            JobRequest::ConvertFiles {
                library,
                output_dir,
                files,
                target,
            },
        )
    }
    fn send(
        &self,
        title: String,
        book: PathBuf,
        author: String,
        device: Device,
        options: PreparationOptions,
    ) -> JobId {
        self.enqueue(
            title,
            JobRequest::Send {
                book,
                author,
                device,
                options,
            },
        )
    }
}

pub struct RealServices {
    jobs: Mutex<Coordinator>,
    settings_path: PathBuf,
    config_path: PathBuf,
    settings_write: Arc<tokio::sync::Mutex<()>>,
    cover_cache_path: PathBuf,
    devices: Arc<devices::DeviceFeed>,
}
impl RealServices {
    pub fn new() -> Result<Self, String> {
        // Portable/fixture mode: keep config, preferences and the cover cache in one
        // folder instead of the user's profile (used by the manual checklist).
        if let Some(dir) = std::env::var_os("RULIBRE_UI_DATA_DIR").map(PathBuf::from) {
            return Ok(
                Self::with_paths(dir.join("config.toml"), dir.join("ui.toml"))?
                    .with_cover_cache(dir.join("covers")),
            );
        }
        Self::with_paths(
            rulibre_core::config::Config::path().map_err(|e| e.to_string())?,
            UiSettings::path().map_err(|e| e.to_string())?,
        )
    }
    /// Explicit paths let fixture runs avoid the user's saved configuration.
    pub fn with_paths(config_path: PathBuf, settings_path: PathBuf) -> Result<Self, String> {
        Self::with_paths_and_executor(
            config_path,
            settings_path,
            rulibre_core::jobs::JobExecutor::new(2, 16).map_err(|e| e.to_string())?,
        )
    }
    /// Keep real IO/coordinator behavior while injecting the core's volume feed.
    pub fn with_paths_and_executor(
        config_path: PathBuf,
        settings_path: PathBuf,
        executor: rulibre_core::jobs::JobExecutor,
    ) -> Result<Self, String> {
        Ok(Self {
            jobs: Mutex::new(Coordinator::with_core_executor(executor)),
            settings_path,
            config_path,
            settings_write: Arc::new(tokio::sync::Mutex::new(())),
            cover_cache_path: covers::cache_dir(),
            devices: Arc::new(devices::DeviceFeed::default()),
        })
    }
    /// Isolate benchmark caches without changing the user's preferences.
    pub fn with_cover_cache(mut self, path: PathBuf) -> Self {
        self.cover_cache_path = path;
        self
    }
    /// Retain temporary conversion output until shutdown, after workers join.
    pub fn enqueue_with_workspace(
        &self,
        title: String,
        request: JobRequest,
        workspace: Workspace,
    ) -> JobId {
        self.jobs
            .lock()
            .unwrap()
            .enqueue(title, request, Some(workspace))
    }
}
fn validate_library(path: PathBuf) -> Result<PathBuf, String> {
    match rulibre_core::config::is_calibre_library(&path) {
        Ok(true) => Ok(path),
        Ok(false) => Err(format!(
            "{} is not an available Calibre library. Choose the folder containing metadata.db.",
            path.display()
        )),
        Err(error) => Err(format!("Could not access the library folder. {error}")),
    }
}
fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> ServiceFuture<T> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| e.to_string())?
    })
}
impl Services for RealServices {
    fn device_subscription(&self) -> iced::Subscription<devices::DeviceUpdate> {
        self.devices.subscription()
    }
    fn validate_reader(
        &self,
        path: PathBuf,
        kind: rulibre_core::device::DeviceKind,
    ) -> ServiceFuture<Device> {
        blocking(move || {
            rulibre_core::device::manual_device(&path, kind).map_err(|e| e.to_string())
        })
    }
    fn remember_reader(&self, reader: &Device) -> u64 {
        self.devices.remember(reader)
    }
    fn load_thumbnail(
        &self,
        id: rulibre_core::book::BookId,
        book: PathBuf,
    ) -> ServiceFuture<Option<iced::widget::image::Handle>> {
        let cache = self.cover_cache_path.clone();
        blocking(move || covers::load(&id, &book, &cache))
    }
    fn load_library(&self) -> ServiceFuture<Option<PathBuf>> {
        let path = self.config_path.clone();
        blocking(move || {
            rulibre_core::config::Config::load_from(&path)
                .map_err(|e| {
                    format!(
                        "Could not load {}. Choose your library again. {e}",
                        path.display()
                    )
                })?
                .map(|config| validate_library(config.library_path))
                .transpose()
        })
    }
    fn validate_library(&self, input: String) -> ServiceFuture<PathBuf> {
        blocking(move || {
            let path = rulibre_core::config::sanitize_path(&input)
                .map_err(|e| format!("Enter a library folder. {e}"))?;
            validate_library(path)
        })
    }
    fn save_library(&self, path: PathBuf) -> ServiceFuture<PathBuf> {
        let config_path = self.config_path.clone();
        blocking(move || {
            let path = validate_library(path)?;
            rulibre_core::config::Config {
                library_path: path.clone(),
            }
            .save_to(&config_path)
            .map_err(|e| format!("Could not save the library setting. {e}"))?;
            Ok(path)
        })
    }
    fn enqueue(&self, title: String, request: JobRequest) -> JobId {
        self.jobs.lock().unwrap().enqueue(title, request, None)
    }
    fn poll(&self) -> Vec<JobUpdate> {
        self.jobs.lock().unwrap().poll()
    }
    fn cancel(&self, id: JobId) {
        self.jobs.lock().unwrap().cancel(id);
    }
    fn clear_finished(&self) {
        self.jobs.lock().unwrap().clear_finished();
    }
    fn shutdown(&self) {
        self.devices.stop();
        self.jobs.lock().unwrap().shutdown();
    }
    fn load_settings(&self) -> ServiceFuture<UiSettings> {
        let path = self.settings_path.clone();
        blocking(move || UiSettings::load_from(&path).map_err(|e| e.to_string()))
    }
    fn save_settings(&self, settings: UiSettings) -> ServiceFuture<()> {
        let path = self.settings_path.clone();
        let lock = self.settings_write.clone();
        Box::pin(async move {
            let _guard = lock.lock().await;
            blocking(move || settings.save_to(&path).map_err(|e| e.to_string())).await
        })
    }
    fn find_cover(&self, book: PathBuf) -> ServiceFuture<Option<CoverFile>> {
        blocking(move || rulibre_core::cover::find_cover(&book).map_err(|e| e.to_string()))
    }
    fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>> {
        // Construct on the event-loop thread (required by macOS), await in iced.
        let dialog = rfd::AsyncFileDialog::new().pick_folder();
        Box::pin(async move { Ok(dialog.await.map(|file| file.path().to_owned())) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_config_validation_scan_and_read_only_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("Calibre Library");
        let book = library.join("Author/Book (1)");
        std::fs::create_dir_all(&book).unwrap();
        // Scanner uses the directory tree; metadata.db is the setup marker.
        let database = library.join("metadata.db");
        let ebook = book.join("Book.epub");
        std::fs::write(&database, b"fixture marker").unwrap();
        std::fs::write(&ebook, b"fixture ebook").unwrap();
        let config_path = dir.path().join("config.toml");
        let services =
            RealServices::with_paths(config_path.clone(), dir.path().join("ui.toml")).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(services.load_library().await.unwrap().is_none());
            let input = format!("  \"{}\"  ", library.display());
            let validated = services.validate_library(input).await.unwrap();
            assert_eq!(validated, library);
            services.save_library(validated).await.unwrap();
            assert_eq!(
                services.load_library().await.unwrap(),
                Some(library.clone())
            );
            let before = std::fs::read(&config_path).unwrap();
            assert!(services.validate_library("  ".into()).await.is_err());
            assert!(
                services
                    .save_library(dir.path().join("missing"))
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&config_path).unwrap(), before);
            let id = services.scan(library.clone());
            let started = std::time::Instant::now();
            loop {
                let updates = services.poll();
                if let Some(result) = updates
                    .into_iter()
                    .find(|update| update.view.id == id && update.result.is_some())
                    .and_then(|update| update.result)
                {
                    let rulibre_core::jobs::JobOutput::Scan(report) =
                        result.as_ref().as_ref().unwrap()
                    else {
                        panic!("scan output")
                    };
                    assert_eq!(report.books.len(), 1);
                    assert!(report.warnings.is_empty());
                    break;
                }
                assert!(started.elapsed() < std::time::Duration::from_secs(10));
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(std::fs::read(&database).unwrap(), b"fixture marker");
            assert_eq!(std::fs::read(&ebook).unwrap(), b"fixture ebook");
            // A deleted saved library produces a setup diagnostic.
            std::fs::remove_file(&database).unwrap();
            assert!(
                services
                    .load_library()
                    .await
                    .unwrap_err()
                    .contains("metadata.db")
            );
            std::fs::write(&config_path, b"invalid = [").unwrap();
            let error = services.load_library().await.unwrap_err();
            assert!(error.contains(&config_path.display().to_string()));
            assert_eq!(std::fs::read(&config_path).unwrap(), b"invalid = [");
            // Config replacement failure never starts a scan or destroys data.
            let failed_config = dir.path().join("blocked-config");
            std::fs::create_dir(&failed_config).unwrap();
            std::fs::write(failed_config.join("keep"), b"previous config").unwrap();
            std::fs::write(&database, b"fixture marker").unwrap();
            let failing =
                RealServices::with_paths(failed_config.clone(), dir.path().join("other-ui.toml"))
                    .unwrap();
            assert!(
                failing
                    .save_library(library)
                    .await
                    .unwrap_err()
                    .contains("Could not save")
            );
            assert_eq!(
                std::fs::read(failed_config.join("keep")).unwrap(),
                b"previous config"
            );
            failing.shutdown();
        });
        services.shutdown();
    }
    #[test]
    fn real_services_use_background_io_for_settings_and_covers() {
        let dir = tempfile::tempdir().unwrap();
        let services = RealServices {
            jobs: Mutex::new(Coordinator::new(1, 1).unwrap()),
            settings_path: dir.path().join("ui.toml"),
            config_path: dir.path().join("config.toml"),
            settings_write: Arc::new(tokio::sync::Mutex::new(())),
            cover_cache_path: dir.path().join("covers"),
            devices: Arc::new(devices::DeviceFeed::default()),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert_eq!(
                services.load_settings().await.unwrap(),
                UiSettings::default()
            );
            let settings = UiSettings {
                window_opacity: 30,
                window_blur: false,
                ..Default::default()
            };
            services.save_settings(settings.clone()).await.unwrap();
            assert_eq!(services.load_settings().await.unwrap(), settings);
            assert!(
                services
                    .find_cover(dir.path().to_owned())
                    .await
                    .unwrap()
                    .is_none()
            );
            std::fs::write(dir.path().join("Cover.JPG"), b"metadata only").unwrap();
            let cover = services
                .find_cover(dir.path().to_owned())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(cover.len, 13);
            std::fs::write(&services.settings_path, b"broken = [").unwrap();
            assert!(
                services
                    .load_settings()
                    .await
                    .unwrap_err()
                    .contains("parse UI settings")
            );
            assert_eq!(
                std::fs::read(&services.settings_path).unwrap(),
                b"broken = ["
            );
        });
        services.shutdown();
    }
}
