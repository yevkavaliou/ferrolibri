//! The only boundary that performs IO. Views consume data and emit messages.
pub mod covers;
pub mod devices;
pub mod jobs;
pub mod plugins;
use crate::settings::UiSettings;
use ferrolibri_core::{
    book::{BookFile, Format},
    calibre,
    config::{self, LibraryPathState},
    converter::{CancellationToken, Workspace},
    cover::CoverFile,
    device::{Device, PreparationOptions},
    jobs::JobRequest,
};
use jobs::{Coordinator, JobId, JobUpdate};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

/// How many entries of a folder without a library are shown before creating one.
pub const FIRST_FILES_SHOWN: usize = 5;

/// What the library-choice screen needs to know about a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryInspection {
    pub path: PathBuf,
    pub state: LibraryPathState,
    /// First entries of a folder that has files but no library.
    pub files: Vec<String>,
    /// `calibredb` is installed; only looked up for folders where a library could be created.
    pub calibre_available: bool,
}

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
        _kind: ferrolibri_core::device::DeviceKind,
    ) -> ServiceFuture<Device> {
        Box::pin(async { Err("Reader validation unavailable".into()) })
    }
    fn remember_reader(&self, _reader: &Device) -> u64 {
        0
    }
    fn load_thumbnail(
        &self,
        _id: ferrolibri_core::book::BookId,
        _book: PathBuf,
    ) -> ServiceFuture<Option<iced::widget::image::Handle>> {
        Box::pin(async { Ok(None) })
    }
    fn enqueue(&self, title: String, request: JobRequest) -> JobId;
    /// Queues a job defined outside the core (a download for a plugin). The job's title is
    /// the title in the list. If it is refused (for example `calibredb` is missing) the job
    /// fails at once with the reason as its message.
    fn enqueue_external(
        &self,
        job: std::sync::Arc<dyn ferrolibri_core::jobs::ExternalJob>,
    ) -> JobId {
        self.enqueue(job.title(), JobRequest::External(job))
    }
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
    /// Classify the folder typed or picked on the setup screen. Nothing is written.
    /// `native` is a path from the folder picker and takes precedence over `input`.
    fn inspect_library(
        &self,
        input: String,
        native: Option<PathBuf>,
    ) -> ServiceFuture<LibraryInspection> {
        blocking(move || {
            let path = match native {
                Some(path) => path,
                None => config::sanitize_path(&input)
                    .map_err(|e| format!("Enter a library folder. {e}"))?,
            };
            inspect(path)
        })
    }
    /// Inspect the library saved in the config, if there is one. Used at startup to
    /// explain why it cannot be opened; an unreadable config yields `None`.
    fn inspect_saved_library(&self) -> ServiceFuture<Option<LibraryInspection>> {
        Box::pin(async { Ok(None) })
    }
    /// Create an empty library with `calibredb`. `among_files` is set only after the
    /// user explicitly confirmed creating one in a folder that already has files.
    fn create_library(&self, path: PathBuf, among_files: bool) -> ServiceFuture<PathBuf> {
        blocking(move || create_library(path, among_files))
    }
    fn validate_export(&self, library: PathBuf, output: PathBuf) -> ServiceFuture<PathBuf> {
        blocking(move || {
            ferrolibri_core::converter::validate_output_dir(&library, &output).map_err(|e| {
                format!("Choose an existing writable export folder outside Calibre libraries. {e}")
            })
        })
    }
    /// Every installed plugin with its settings, as the Settings screen shows them.
    fn list_plugins(&self) -> ServiceFuture<Vec<plugins::PluginRow>> {
        Box::pin(async { Err("Plugins are unavailable".into()) })
    }
    /// Looks at a folder or an address and says what is there. Nothing is installed.
    fn check_plugin_source(
        &self,
        _request: plugins::SourceRequest,
        _cancel: CancellationToken,
    ) -> plugins::PluginFuture<plugins::Checked> {
        Box::pin(async { Err(plugins::Problem::unavailable()) })
    }
    /// Downloads and checks one release picked from a GitHub list.
    fn inspect_plugin_release(
        &self,
        _release: plugins::ReleaseView,
        _cancel: CancellationToken,
    ) -> plugins::PluginFuture<plugins::Candidate> {
        Box::pin(async { Err(plugins::Problem::unavailable()) })
    }
    /// Installs what the user confirmed.
    fn install_plugin(
        &self,
        _candidate: plugins::Candidate,
        _options: plugins::InstallOptions,
    ) -> plugins::PluginFuture<plugins::Installed> {
        Box::pin(async { Err(plugins::Problem::unavailable()) })
    }
    fn set_plugin_enabled(&self, _id: String, _enabled: bool) -> ServiceFuture<()> {
        Box::pin(async { Err("Plugins are unavailable".into()) })
    }
    fn remove_plugin(&self, _id: String) -> ServiceFuture<()> {
        Box::pin(async { Err("Plugins are unavailable".into()) })
    }
    /// Replaces the saved values of a plugin with the whole form.
    fn save_plugin_settings(
        &self,
        _id: String,
        _values: Vec<(String, plugins::FieldValue)>,
    ) -> ServiceFuture<()> {
        Box::pin(async { Err("Plugins are unavailable".into()) })
    }
    /// One page of results from a search plugin. The first page has no cursor; the next one
    /// passes the `next_cursor` of the previous page. Cancelling `cancel` ends the call.
    fn search_plugin(
        &self,
        _id: String,
        _text: String,
        _page: u32,
        _cursor: Option<String>,
        _cancel: CancellationToken,
    ) -> plugins::SourceFuture<plugins::RemotePage> {
        Box::pin(async { Err(plugins::SourceError::unavailable()) })
    }
    /// The plugin's `details` for a result, merged into it.
    fn plugin_details(
        &self,
        _id: String,
        _hit: plugins::RemoteHit,
        _cancel: CancellationToken,
    ) -> plugins::SourceFuture<plugins::RemoteHit> {
        Box::pin(async { Err(plugins::SourceError::unavailable()) })
    }
    /// A cover by address, through the plugin's own network rules, cached on disk.
    /// `Err` is a reason for the log; the screen draws a placeholder either way.
    fn load_remote_cover(
        &self,
        _id: String,
        _url: String,
        _cancel: CancellationToken,
    ) -> ServiceFuture<iced::widget::image::Handle> {
        Box::pin(async { Err("Covers of plugins are unavailable".into()) })
    }
    /// Whether Calibre (`calibredb`) is installed, which adding a downloaded book needs.
    /// `recheck` looks again even if it was found. The default says yes, so a fake that does
    /// not care about downloads shows the usual icons.
    fn calibre_available(&self, _recheck: bool) -> ServiceFuture<bool> {
        Box::pin(async { Ok(true) })
    }
    /// Builds the job that downloads one offer and adds the book to the library; the caller
    /// queues it with `enqueue`. Refused with `NeedsCalibre` when `calibredb` is missing.
    fn prepare_download(&self, _request: plugins::DownloadRequest) -> plugins::DownloadFuture {
        Box::pin(async {
            Err(plugins::DownloadError::Failed(
                "Downloads are unavailable".into(),
            ))
        })
    }
    /// Opens a web page in the user's browser. Only `http` and `https` addresses.
    fn open_url(&self, url: String) -> ServiceFuture<()> {
        blocking(move || open_web_address(&url))
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
    plugins: plugins::PluginBackend,
}
impl RealServices {
    pub fn new() -> Result<Self, String> {
        // Portable/fixture mode: keep config, preferences and the cover cache in one
        // folder instead of the user's profile (used by the manual checklist).
        if let Some(dir) = std::env::var_os("FERROLIBRI_UI_DATA_DIR").map(PathBuf::from) {
            return Ok(
                Self::with_paths(dir.join("config.toml"), dir.join("ui.toml"))?
                    .with_cover_cache(dir.join("covers")),
            );
        }
        Self::with_paths(
            ferrolibri_core::config::Config::path().map_err(|e| e.to_string())?,
            UiSettings::path().map_err(|e| e.to_string())?,
        )
        .map(|services| {
            // Plugins live in the user's data directory, not next to `ui.toml`.
            match ferrolibri_plugins::PluginStore::default_location() {
                Ok(store) => services.with_plugin_store(store),
                Err(_) => services,
            }
        })
    }
    /// Explicit paths let fixture runs avoid the user's saved configuration.
    pub fn with_paths(config_path: PathBuf, settings_path: PathBuf) -> Result<Self, String> {
        Self::with_paths_and_executor(
            config_path,
            settings_path,
            ferrolibri_core::jobs::JobExecutor::new(2, 16).map_err(|e| e.to_string())?,
        )
    }
    /// Keep real IO/coordinator behavior while injecting the core's volume feed.
    pub fn with_paths_and_executor(
        config_path: PathBuf,
        settings_path: PathBuf,
        executor: ferrolibri_core::jobs::JobExecutor,
    ) -> Result<Self, String> {
        // Without a better place, plugins sit next to the preferences, so a run with explicit
        // paths (a fixture, a test) never touches the user's own plugins.
        let home = settings_path
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf);
        let plugin_store =
            ferrolibri_plugins::PluginStore::new(home.join("plugins"), home.join("plugins.toml"));
        Ok(Self {
            jobs: Mutex::new(Coordinator::with_core_executor(executor)),
            settings_path,
            config_path,
            settings_write: Arc::new(tokio::sync::Mutex::new(())),
            cover_cache_path: covers::cache_dir(),
            devices: Arc::new(devices::DeviceFeed::default()),
            plugins: plugins::PluginBackend::new(plugin_store),
        })
    }
    /// Keep plugins and their switches somewhere other than beside the preferences.
    pub fn with_plugin_store(mut self, store: ferrolibri_plugins::PluginStore) -> Self {
        self.plugins = plugins::PluginBackend::new(store);
        self
    }
    /// Plugins with their own HTTP client, for tests that stand a local server in for a site.
    pub fn with_plugin_http(
        mut self,
        store: ferrolibri_plugins::PluginStore,
        http: ferrolibri_plugins::Http,
    ) -> Self {
        self.plugins = plugins::PluginBackend::with_http(store, http);
        self
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
    match ferrolibri_core::config::is_calibre_library(&path) {
        Ok(true) => Ok(path),
        Ok(false) => Err(format!(
            "{} is not an available Calibre library. Choose the folder containing metadata.db.",
            path.display()
        )),
        Err(error) => Err(format!("Could not access the library folder. {error}")),
    }
}
fn inspect(path: PathBuf) -> Result<LibraryInspection, String> {
    let state = config::inspect_library_path(&path)
        .map_err(|error| format!("Could not access the library folder. {error}"))?;
    let creatable = matches!(
        state,
        LibraryPathState::NotLibraryEmpty | LibraryPathState::NotLibraryNotEmpty
    );
    let files = if state == LibraryPathState::NotLibraryNotEmpty {
        config::first_entries(&path, FIRST_FILES_SHOWN).unwrap_or_default()
    } else {
        Vec::new()
    };
    let calibre_available = creatable && matches!(calibre::find(), Ok(Some(_)));
    Ok(LibraryInspection {
        path,
        state,
        files,
        calibre_available,
    })
}

/// Calibre on Windows counts the whole library path; refuse before running anything.
fn long_path_message(path: &Path, windows: bool) -> Option<String> {
    let length = path.as_os_str().to_string_lossy().chars().count();
    (windows && length > calibre::WINDOWS_LIBRARY_PATH_LIMIT).then(|| {
        format!(
            "This path is {length} characters long; Calibre on Windows supports {} or fewer. \
             Choose a shorter folder. Nothing was created.",
            calibre::WINDOWS_LIBRARY_PATH_LIMIT
        )
    })
}

fn create_library(path: PathBuf, among_files: bool) -> Result<PathBuf, String> {
    if let Some(message) = long_path_message(&path, cfg!(windows)) {
        return Err(message);
    }
    // The folder may have changed since it was inspected: check again before writing.
    let state = config::inspect_library_path(&path)
        .map_err(|error| format!("Could not access the library folder. {error}"))?;
    if !matches!(
        (state, among_files),
        (LibraryPathState::NotLibraryEmpty, _) | (LibraryPathState::NotLibraryNotEmpty, true)
    ) {
        return Err(
            "The folder changed since it was checked. Check it again before creating a library."
                .into(),
        );
    }
    let cli = calibre::require().map_err(|error| error.to_string())?;
    let cancel = CancellationToken::default();
    let created = if state == LibraryPathState::NotLibraryEmpty {
        cli.create_library(&path, &cancel)
    } else {
        cli.create_library_among_files(&path, &cancel)
    };
    created.map_err(|error| format!("Could not create the library. {error}"))?;
    validate_library(path)
}

fn saved_library_message(path: &Path, state: LibraryPathState) -> String {
    let reason = match state {
        LibraryPathState::Missing => "is unavailable",
        LibraryPathState::NotDirectory => "is not a folder",
        _ => "has no metadata.db",
    };
    format!(
        "The saved library {} {reason}. Open it again or choose another folder.",
        path.display()
    )
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
fn blocking_source<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, plugins::SourceError> + Send + 'static,
) -> plugins::SourceFuture<T> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| plugins::SourceError::plugin(e.to_string()))?
    })
}
/// Hands an `http(s)` address to the system, which opens it in the default browser. The
/// address comes from a plugin, so it is checked here again and passed as one argument,
/// never through a shell.
fn open_web_address(url: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://"))
        || url.chars().any(char::is_control)
    {
        return Err("Only web addresses can be opened.".into());
    }
    web_address_command(url.trim())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The program that opens `url` in the default browser. On Windows this is the URL
/// protocol handler, not `explorer.exe`: Explorer opens a folder instead when the
/// address has a query string (`?id=…&hl=…`).
fn web_address_command(url: &str) -> std::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    }
}
fn blocking_plugin<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, plugins::Problem> + Send + 'static,
) -> plugins::PluginFuture<T> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| plugins::Problem::failed("Could not check the plugin", e.to_string()))?
    })
}
impl Services for RealServices {
    fn device_subscription(&self) -> iced::Subscription<devices::DeviceUpdate> {
        self.devices.subscription()
    }
    fn validate_reader(
        &self,
        path: PathBuf,
        kind: ferrolibri_core::device::DeviceKind,
    ) -> ServiceFuture<Device> {
        blocking(move || {
            ferrolibri_core::device::manual_device(&path, kind).map_err(|e| e.to_string())
        })
    }
    fn remember_reader(&self, reader: &Device) -> u64 {
        self.devices.remember(reader)
    }
    fn load_thumbnail(
        &self,
        id: ferrolibri_core::book::BookId,
        book: PathBuf,
    ) -> ServiceFuture<Option<iced::widget::image::Handle>> {
        let cache = self.cover_cache_path.clone();
        blocking(move || covers::load(&id, &book, &cache))
    }
    fn load_library(&self) -> ServiceFuture<Option<PathBuf>> {
        let path = self.config_path.clone();
        blocking(move || {
            ferrolibri_core::config::Config::load_from(&path)
                .map_err(|e| {
                    format!(
                        "Could not load {}. Choose your library again. {e}",
                        path.display()
                    )
                })?
                .map(
                    |config| match config::inspect_library_path(&config.library_path) {
                        Ok(LibraryPathState::Ready) => Ok(config.library_path),
                        Ok(state) => Err(saved_library_message(&config.library_path, state)),
                        Err(error) => Err(format!(
                            "Could not access the saved library folder. {error}"
                        )),
                    },
                )
                .transpose()
        })
    }
    fn inspect_saved_library(&self) -> ServiceFuture<Option<LibraryInspection>> {
        let path = self.config_path.clone();
        blocking(move || match config::Config::load_from(&path) {
            Ok(Some(config)) if !config.library_path.as_os_str().is_empty() => {
                inspect(config.library_path).map(Some)
            }
            _ => Ok(None),
        })
    }
    fn validate_library(&self, input: String) -> ServiceFuture<PathBuf> {
        blocking(move || {
            let path = ferrolibri_core::config::sanitize_path(&input)
                .map_err(|e| format!("Enter a library folder. {e}"))?;
            validate_library(path)
        })
    }
    fn save_library(&self, path: PathBuf) -> ServiceFuture<PathBuf> {
        let config_path = self.config_path.clone();
        blocking(move || {
            let path = validate_library(path)?;
            ferrolibri_core::config::Config {
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
        blocking(move || ferrolibri_core::cover::find_cover(&book).map_err(|e| e.to_string()))
    }
    fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>> {
        // Construct on the event-loop thread (required by macOS), await in iced.
        let dialog = rfd::AsyncFileDialog::new().pick_folder();
        Box::pin(async move { Ok(dialog.await.map(|file| file.path().to_owned())) })
    }
    fn list_plugins(&self) -> ServiceFuture<Vec<plugins::PluginRow>> {
        let backend = self.plugins.clone();
        blocking(move || backend.list())
    }
    fn check_plugin_source(
        &self,
        request: plugins::SourceRequest,
        cancel: CancellationToken,
    ) -> plugins::PluginFuture<plugins::Checked> {
        let backend = self.plugins.clone();
        blocking_plugin(move || backend.check(&request, &cancel))
    }
    fn inspect_plugin_release(
        &self,
        release: plugins::ReleaseView,
        cancel: CancellationToken,
    ) -> plugins::PluginFuture<plugins::Candidate> {
        let backend = self.plugins.clone();
        blocking_plugin(move || backend.inspect_release(&release, &cancel))
    }
    fn install_plugin(
        &self,
        candidate: plugins::Candidate,
        options: plugins::InstallOptions,
    ) -> plugins::PluginFuture<plugins::Installed> {
        let backend = self.plugins.clone();
        blocking_plugin(move || backend.install(&candidate, options))
    }
    fn set_plugin_enabled(&self, id: String, enabled: bool) -> ServiceFuture<()> {
        let backend = self.plugins.clone();
        blocking(move || backend.set_enabled(&id, enabled))
    }
    fn remove_plugin(&self, id: String) -> ServiceFuture<()> {
        let backend = self.plugins.clone();
        blocking(move || backend.remove(&id))
    }
    fn save_plugin_settings(
        &self,
        id: String,
        values: Vec<(String, plugins::FieldValue)>,
    ) -> ServiceFuture<()> {
        let backend = self.plugins.clone();
        blocking(move || backend.save_settings(&id, &values))
    }
    fn search_plugin(
        &self,
        id: String,
        text: String,
        page: u32,
        cursor: Option<String>,
        cancel: CancellationToken,
    ) -> plugins::SourceFuture<plugins::RemotePage> {
        let backend = self.plugins.clone();
        blocking_source(move || {
            let query = ferrolibri_plugins::SearchQuery { text, page, cursor };
            backend.search(&id, &query, &cancel)
        })
    }
    fn plugin_details(
        &self,
        id: String,
        hit: plugins::RemoteHit,
        cancel: CancellationToken,
    ) -> plugins::SourceFuture<plugins::RemoteHit> {
        let backend = self.plugins.clone();
        blocking_source(move || backend.details(&id, &hit, &cancel))
    }
    fn calibre_available(&self, recheck: bool) -> ServiceFuture<bool> {
        let backend = self.plugins.clone();
        blocking(move || Ok(backend.calibre(recheck).is_some()))
    }
    fn prepare_download(&self, request: plugins::DownloadRequest) -> plugins::DownloadFuture {
        let backend = self.plugins.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || backend.prepare_download(&request))
                .await
                .map_err(|e| plugins::DownloadError::Failed(e.to_string()))?
        })
    }
    fn load_remote_cover(
        &self,
        id: String,
        url: String,
        cancel: CancellationToken,
    ) -> ServiceFuture<iced::widget::image::Handle> {
        let backend = self.plugins.clone();
        let cache = self.cover_cache_path.join("remote");
        blocking(move || {
            covers::load_remote(&url, &cache, || backend.fetch_cover(&id, &url, &cancel))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn web_address_is_one_argument_and_never_goes_to_explorer() {
        let url = "https://books.google.com/books?id=AbC&hl=&source=gbs_api";
        let command = web_address_command(url);
        let program = command.get_program().to_string_lossy().to_lowercase();
        let arguments: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!program.contains("explorer"), "{program}");
        assert_eq!(arguments.last().map(String::as_str), Some(url));
        #[cfg(target_os = "windows")]
        assert_eq!(arguments, ["url.dll,FileProtocolHandler", url]);
    }
    #[test]
    fn only_web_addresses_are_opened() {
        for bad in [
            "file:///C:/Windows",
            "javascript:alert(1)",
            "C:\\Windows",
            "https://x\ny",
        ] {
            assert!(open_web_address(bad).is_err(), "{bad}");
        }
    }
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
                    let ferrolibri_core::jobs::JobOutput::Scan(report) =
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
    fn inspection_reports_every_folder_state_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let documents = dir.path().join("documents");
        std::fs::create_dir(&documents).unwrap();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(documents.join(name), b"x").unwrap();
        }
        let file = dir.path().join("book.epub");
        std::fs::write(&file, b"x").unwrap();
        let library = dir.path().join("library");
        std::fs::create_dir(&library).unwrap();
        std::fs::write(library.join("metadata.db"), b"marker").unwrap();
        let state = |path: &Path| inspect(path.to_owned()).unwrap();
        assert_eq!(state(&library).state, LibraryPathState::Ready);
        assert!(
            !state(&library).calibre_available,
            "only asked where creation is possible"
        );
        assert_eq!(state(&empty).state, LibraryPathState::NotLibraryEmpty);
        let with_files = state(&documents);
        assert_eq!(with_files.state, LibraryPathState::NotLibraryNotEmpty);
        assert_eq!(with_files.files, ["a.txt", "b.txt"]);
        assert_eq!(state(&file).state, LibraryPathState::NotDirectory);
        let missing = state(&dir.path().join("unplugged"));
        assert_eq!(missing.state, LibraryPathState::Missing);
        assert!(!missing.calibre_available && missing.files.is_empty());
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
    }

    #[test]
    fn creation_is_refused_for_missing_files_and_changed_folders_before_calibre_runs() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("unplugged");
        assert!(
            create_library(missing.clone(), false)
                .unwrap_err()
                .contains("changed")
        );
        assert!(!missing.exists(), "a missing folder is never created");
        let documents = dir.path().join("documents");
        std::fs::create_dir(&documents).unwrap();
        std::fs::write(documents.join("notes.txt"), b"keep").unwrap();
        // A folder with files needs the explicit confirmation flag.
        assert!(
            create_library(documents.clone(), false)
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(std::fs::read_dir(&documents).unwrap().count(), 1);
        let library = dir.path().join("library");
        std::fs::create_dir(&library).unwrap();
        std::fs::write(library.join("metadata.db"), b"marker").unwrap();
        assert!(
            create_library(library.clone(), true)
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(
            std::fs::read(library.join("metadata.db")).unwrap(),
            b"marker"
        );
    }

    #[test]
    fn windows_long_paths_are_explained_before_calibredb_is_used() {
        let long = PathBuf::from(format!("D:\\{}", "x".repeat(100)));
        let message = long_path_message(&long, true).unwrap();
        assert!(
            message.contains("103 characters") && message.contains("88"),
            "{message}"
        );
        assert!(long_path_message(&long, false).is_none());
        let short = PathBuf::from(format!("D:\\{}", "x".repeat(85)));
        assert_eq!(short.as_os_str().len(), 88);
        assert!(long_path_message(&short, true).is_none());
    }

    #[test]
    fn saved_library_inspection_explains_a_lost_library() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let services =
            RealServices::with_paths(config_path.clone(), dir.path().join("ui.toml")).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(services.inspect_saved_library().await.unwrap().is_none());
            let library = dir.path().join("Calibre Library");
            std::fs::create_dir(&library).unwrap();
            std::fs::write(library.join("metadata.db"), b"marker").unwrap();
            ferrolibri_core::config::Config {
                library_path: library.clone(),
            }
            .save_to(&config_path)
            .unwrap();
            let ready = services.inspect_saved_library().await.unwrap().unwrap();
            assert_eq!(ready.state, LibraryPathState::Ready);
            std::fs::remove_file(library.join("metadata.db")).unwrap();
            let lost = services.inspect_saved_library().await.unwrap().unwrap();
            assert_eq!(lost.state, LibraryPathState::NotLibraryEmpty);
            let message = services.load_library().await.unwrap_err();
            assert!(message.contains("no metadata.db") && message.contains("Open it again"));
            std::fs::remove_dir(&library).unwrap();
            let gone = services.inspect_saved_library().await.unwrap().unwrap();
            assert_eq!(gone.state, LibraryPathState::Missing);
            assert!(
                services
                    .load_library()
                    .await
                    .unwrap_err()
                    .contains("unavailable")
            );
            std::fs::write(&config_path, b"invalid = [").unwrap();
            assert!(services.inspect_saved_library().await.unwrap().is_none());
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
            plugins: plugins::PluginBackend::new(ferrolibri_plugins::PluginStore::new(
                dir.path().join("plugins"),
                dir.path().join("plugins.toml"),
            )),
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
