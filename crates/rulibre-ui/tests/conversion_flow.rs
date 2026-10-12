//! Drive actual iced tasks, atomic config IO and scans against two fixture
//! libraries. Folder-picker responses are injected; no physical reader is used.
#[path = "../../rulibre-core/src/test_support.rs"]
mod test_support;
use iced::futures::StreamExt;
use rulibre_core::{cover::CoverFile, jobs::JobRequest};
use rulibre_ui::{
    app::{self, Message, ScanStatus, State},
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
#[ignore = "Requires installed ebook-convert; explicit developer verification"]
fn real_txt_to_epub_then_name_conflict_preserves_library_and_export() {
    use rulibre_core::book::Format;
    let fixture = test_support::Fixture::new();
    let book = fixture.library.join("Writer/Conversion fixture (100)");
    std::fs::create_dir_all(&book).unwrap();
    std::fs::write(
        book.join("Conversion fixture.txt"),
        b"A fixture book.\n\nChapter one.\nTest conversion through the UI coordinator.",
    )
    .unwrap();
    let before = test_support::snapshot(&fixture.library);
    let export = fixture.root.join("exports");
    std::fs::create_dir(&export).unwrap();
    let services = FixtureServices {
        real: RealServices::with_paths(
            fixture.root.join("config.toml"),
            fixture.root.join("ui.toml"),
        )
        .unwrap(),
        folders: Mutex::new(VecDeque::from([export.clone()])),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut state = State::default();
        drive(&mut state, Message::LoadLibrary, &services).await;
        drive(
            &mut state,
            Message::SetupPathChanged(fixture.library.display().to_string()),
            &services,
        )
        .await;
        drive(&mut state, Message::ContinueSetup, &services).await;
        scan(&mut state, &services).await;
        drive(&mut state, Message::RefreshBackends, &services).await;
        let deadline = Instant::now() + Duration::from_secs(60);
        while state.backend_job.is_some() {
            drive(&mut state, Message::Tick, &services).await;
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let id = state
            .catalog
            .order
            .iter()
            .find(|id| state.catalog.get(id).unwrap().book.path == book)
            .unwrap()
            .clone();
        drive(&mut state, Message::BookClicked(id.clone()), &services).await;
        assert!(
            rulibre_ui::widgets::details::batch(&state, Format::Epub).is_some(),
            "Ready TXT to EPUB backend required"
        );
        drive(&mut state, Message::Convert(Format::Epub), &services).await;
        assert!(state.export_prompt);
        drive(&mut state, Message::ExportBrowse, &services).await;
        assert!(state.conversion_export.is_some());
        drive(&mut state, Message::UseExport, &services).await;
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            drive(&mut state, Message::Tick, &services).await;
            if state
                .toasts
                .iter()
                .any(|t| t.message.starts_with("Converted"))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Conversion timeout: {:?}",
                state.job_messages
            );
            assert!(!state.toasts.iter().any(|t| t.error), "{:?}", state.toasts);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let result = state
            .results
            .values()
            .find_map(|r| match r.as_ref() {
                Ok(rulibre_core::jobs::JobOutput::Conversion(result)) => Some(result),
                _ => None,
            })
            .unwrap();
        let output = result.output_path.clone();
        let bytes = std::fs::read(&output).unwrap();
        assert!(bytes.starts_with(b"PK"));
        assert!(
            bytes
                .windows(b"application/epub+zip".len())
                .any(|part| part == b"application/epub+zip")
        );
        assert!(bytes.len() > 100);
        assert_eq!(
            state
                .toasts
                .iter()
                .find(|t| t.message.starts_with("Converted"))
                .unwrap()
                .folder
                .as_deref(),
            output.parent()
        );
        assert_eq!(before, test_support::snapshot(&fixture.library));
        assert_eq!(
            services.real.load_settings().await.unwrap().export_folder,
            state.settings.export_folder
        );
        println!(
            "Real TXT -> EPUB: {} bytes, backend {} {:?}",
            result.size,
            result.backend.kind.program(),
            result.backend.version
        );
        drive(&mut state, Message::Convert(Format::Epub), &services).await;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            drive(&mut state, Message::Tick, &services).await;
            if state.toasts.iter().any(|t| t.error) {
                break;
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            state
                .toasts
                .iter()
                .any(|t| t.error && t.message.contains("not overwritten")),
            "{:?}",
            state.toasts
        );
        assert_eq!(bytes, std::fs::read(output).unwrap());
        assert_eq!(before, test_support::snapshot(&fixture.library));
    });
    services.shutdown();
}
