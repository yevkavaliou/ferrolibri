//! Drive actual iced tasks, atomic config IO and scans against two fixture
//! libraries. Folder-picker responses are injected; no physical reader is used.
#[path = "../../rulibre-core/src/test_support.rs"]
mod test_support;
use iced::futures::StreamExt;
use rulibre_core::{cover::CoverFile, jobs::JobRequest};
use rulibre_ui::{
    app::{self, Message, ScanStatus, Screen, State},
    services::{
        RealServices, ServiceFuture, Services,
        jobs::{JobId, JobUpdate},
    },
    settings::UiSettings,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

struct FixtureServices {
    real: RealServices,
    folders: Mutex<VecDeque<PathBuf>>,
}
impl Services for FixtureServices {
    fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>> {
        let folder = self.folders.lock().unwrap().pop_front();
        Box::pin(async move { Ok(folder) })
    }
    fn enqueue(&self, title: String, request: JobRequest) -> JobId {
        self.real.enqueue(title, request)
    }
    fn poll(&self) -> Vec<JobUpdate> {
        self.real.poll()
    }
    fn cancel(&self, id: JobId) {
        self.real.cancel(id);
    }
    fn clear_finished(&self) {
        self.real.clear_finished();
    }
    fn shutdown(&self) {
        self.real.shutdown();
    }
    fn load_settings(&self) -> ServiceFuture<UiSettings> {
        self.real.load_settings()
    }
    fn save_settings(&self, settings: UiSettings) -> ServiceFuture<()> {
        self.real.save_settings(settings)
    }
    fn find_cover(&self, book: PathBuf) -> ServiceFuture<Option<CoverFile>> {
        self.real.find_cover(book)
    }
    fn load_library(&self) -> ServiceFuture<Option<PathBuf>> {
        self.real.load_library()
    }
    fn save_library(&self, path: PathBuf) -> ServiceFuture<PathBuf> {
        self.real.save_library(path)
    }
    fn validate_library(&self, input: String) -> ServiceFuture<PathBuf> {
        self.real.validate_library(input)
    }
}
async fn drive(state: &mut State, message: Message, services: &dyn Services) {
    let mut tasks = VecDeque::from([app::update(state, message, services)]);
    while let Some(task) = tasks.pop_front() {
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(action) = stream.next().await {
                match action {
                    iced_runtime::Action::Output(message) => {
                        tasks.push_back(app::update(state, message, services))
                    }
                    iced_runtime::Action::Widget(_) => {} // No window/focus/scroll needed.
                    _ => panic!("Unexpected non-widget runtime effect"),
                }
            }
        }
    }
}
async fn scan(state: &mut State, services: &dyn Services) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.scan_status == ScanStatus::Running {
        drive(state, Message::Tick, services).await;
        assert!(Instant::now() < deadline, "fixture scan timeout");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(state.scan_status, ScanStatus::Ready);
}
#[test]
fn settings_persist_restart_switch_library_and_invalid_choices_preserve_old_config() {
    let fixture = test_support::Fixture::new();
    let second = fixture.root.join("Second library");
    let book = second.join("Other/Cyberiada (99)");
    std::fs::create_dir_all(&book).unwrap();
    std::fs::write(second.join("metadata.db"), b"fixture marker").unwrap();
    std::fs::write(book.join("Cyberiada.epub"), b"fixture ebook").unwrap();
    let export = fixture.root.join("Экспорт książki");
    std::fs::create_dir(&export).unwrap();
    let config_path = fixture.root.join("config.toml");
    let ui_path = fixture.root.join("ui.toml");
    let services = FixtureServices {
        real: RealServices::with_paths(config_path.clone(), ui_path.clone()).unwrap(),
        folders: Mutex::new(VecDeque::new()),
    };
    let before = test_support::snapshot(&fixture.library);
    let before_second = test_support::snapshot(&second);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        services
            .save_library(fixture.library.clone())
            .await
            .unwrap();
        let mut state = State::default();
        drive(&mut state, Message::LoadSettings, &services).await;
        drive(&mut state, Message::LoadLibrary, &services).await;
        scan(&mut state, &services).await;
        drive(&mut state, Message::Navigate(Screen::Settings), &services).await;
        drive(&mut state, Message::SetWindowOpacity(30), &services).await;
        drive(&mut state, Message::SetPreferKepub(false), &services).await;
        drive(&mut state, Message::SetTextSize(20), &services).await;
        drive(&mut state, Message::SetCoverSize(200), &services).await;
        services.folders.lock().unwrap().push_back(export.clone());
        drive(&mut state, Message::ExportBrowse, &services).await;
        let saved = state.settings.clone();
        assert_eq!(
            saved.export_folder,
            Some(std::fs::canonicalize(&export).unwrap())
        );
        assert_eq!(saved.window_opacity, 30);
        assert!(!saved.prefer_kepub);
        assert_eq!(UiSettings::load_from(&ui_path).unwrap(), saved);
        let bytes = std::fs::read(&ui_path).unwrap();
        for invalid in [&fixture.book, &fixture.root.join("missing")] {
            services.folders.lock().unwrap().push_back(invalid.clone());
            drive(&mut state, Message::ExportBrowse, &services).await;
            assert_eq!(std::fs::read(&ui_path).unwrap(), bytes);
            assert_eq!(state.settings, saved);
            assert!(state.toasts.last().unwrap().error);
        }
        let permissions = std::fs::metadata(&export).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&export, readonly).unwrap();
        services.folders.lock().unwrap().push_back(export.clone());
        drive(&mut state, Message::ExportBrowse, &services).await;
        std::fs::set_permissions(&export, permissions).unwrap();
        assert_eq!(std::fs::read(&ui_path).unwrap(), bytes);
        assert_eq!(state.settings, saved);
        assert!(state.toasts.last().unwrap().error);
        #[cfg(windows)]
        {
            let permissions = std::fs::metadata(&ui_path).unwrap().permissions();
            let mut readonly = permissions.clone();
            readonly.set_readonly(true);
            std::fs::set_permissions(&ui_path, readonly).unwrap();
            drive(&mut state, Message::SetWindowOpacity(80), &services).await;
            std::fs::set_permissions(&ui_path, permissions).unwrap();
            assert_eq!(std::fs::read(&ui_path).unwrap(), bytes);
            assert_eq!(state.visible_settings(), &saved);
            assert!(
                state
                    .toasts
                    .last()
                    .unwrap()
                    .message
                    .contains("Could not save settings")
            );
        }
        let previous_config = std::fs::read(&config_path).unwrap();
        services.folders.lock().unwrap().push_back(export.clone());
        drive(&mut state, Message::SettingsLibraryBrowse, &services).await;
        assert_eq!(std::fs::read(&config_path).unwrap(), previous_config);
        assert_eq!(state.library, Some(fixture.library.clone()));
        assert_eq!(state.screen, Screen::Settings);
        drive(&mut state, Message::Rescan, &services).await;
        let old_scan = state.active_scan.unwrap();
        let selected = state.catalog.order[0].clone();
        state.selection.selected.insert(selected.clone());
        state.covers.insert(
            selected,
            Some(iced::widget::image::Handle::from_rgba(1, 1, vec![255; 4])),
        );
        assert!(!state.covers.is_empty());
        services.folders.lock().unwrap().push_back(second.clone());
        drive(&mut state, Message::SettingsLibraryBrowse, &services).await;
        assert_ne!(state.active_scan, Some(old_scan));
        assert!(state.selection.selected.is_empty());
        assert!(state.covers.is_empty());
        scan(&mut state, &services).await;
        assert_eq!(state.catalog.order.len(), 1);
        assert_eq!(state.catalog.entries[0].book.title, "Cyberiada");
        // Simulate a fresh process loading the same two saved configuration files.
        let restarted = RealServices::with_paths(config_path.clone(), ui_path.clone()).unwrap();
        let mut fresh = State::default();
        drive(&mut fresh, Message::LoadSettings, &restarted).await;
        drive(&mut fresh, Message::LoadLibrary, &restarted).await;
        scan(&mut fresh, &restarted).await;
        assert_eq!(fresh.settings, saved);
        assert_eq!(fresh.library, Some(second.clone()));
        assert_eq!(fresh.catalog.order.len(), 1);
        restarted.shutdown();
    });
    services.shutdown();
    assert_eq!(test_support::snapshot(&fixture.library), before);
    assert_eq!(test_support::snapshot(&second), before_second);
    assert_eq!(std::fs::read_dir(&export).unwrap().count(), 0);
}
