//! Downloading a found book and adding it to the library : the real
//! `view` and `update` driven through the headless simulator, with a scripted `Services` that
//! stands in for the plugin runtime, the job queue and Calibre. The last group (`real`, ignored
//! unless asked for) runs the whole chain against a local catalog and the real `calibredb`.
//!
//! The scripted queue only does what a test tells it to: `report` says how far a job is, `finish`
//! ends it, and nothing moves by itself, so every state of the icon can be held and looked at.
use ferrolibri_core::{
    book::{Book, BookId, Format},
    calibre::AddOutcome,
    converter::CancellationToken,
    cover::CoverFile,
    jobs::{AddedBook, ExternalJob, JobOutput, JobRequest, JobState, Stage},
    scanner::ScanReport,
};
use ferrolibri_plugins::{Offer, SearchHit};
use ferrolibri_ui::{
    app::{self, Message, Screen, State},
    downloads::{self, Icon, Msg as DownloadMsg},
    remote::{Msg, SearchSource},
    services::{
        ServiceFuture, Services,
        jobs::{JobId, JobKind, JobUpdate, JobView},
        plugins::{
            DownloadError, DownloadFuture, DownloadRequest, Health, PermissionSummary, PluginInfo,
            PluginRow, RemoteHit, RemotePage, SourceError, SourceFuture,
        },
    },
    settings::UiSettings,
};
use iced::{futures::StreamExt, widget::image::Handle};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

fn headless() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { std::env::set_var("ICED_TEST_BACKEND", "tiny-skia") });
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the scripted services -----------------------------------------------------------------

#[derive(Debug)]
struct Stub;

impl ExternalJob for Stub {
    fn title(&self) -> String {
        "stub".into()
    }
    fn run(&self, _: &ferrolibri_core::jobs::Context) -> Result<JobOutput, ferrolibri_core::Error> {
        unreachable!("the scripted queue does not run jobs")
    }
}

/// What the screen asked the service to prepare.
#[derive(Clone, Debug)]
struct Prepared {
    plugin: String,
    offer: String,
    library: PathBuf,
    export_dir: Option<PathBuf>,
}

struct Script {
    rows: Vec<PluginRow>,
    page: Option<RemotePage>,
    /// `calibre_available` answers; the last one stays.
    calibre: VecDeque<bool>,
    calibre_asked: Vec<bool>,
    /// Answers to `prepare_download`; none left means success.
    prepare: VecDeque<Result<(), DownloadError>>,
    prepared: Vec<Prepared>,
    next_job: u64,
    /// Every job queued: id, title, whether it is a download.
    queued: Vec<(JobId, String, JobKind)>,
    cancelled: Vec<JobId>,
    updates: Vec<JobUpdate>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            page: None,
            calibre: VecDeque::from([true]),
            calibre_asked: Vec::new(),
            prepare: VecDeque::new(),
            prepared: Vec::new(),
            next_job: 0,
            queued: Vec::new(),
            cancelled: Vec::new(),
            updates: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Fake {
    script: Mutex<Script>,
}

impl Fake {
    fn new(hits: Vec<RemoteHit>) -> Self {
        let fake = Self::default();
        {
            let mut script = lock(&fake.script);
            script.rows = vec![plugin()];
            script.page = Some(RemotePage {
                hits,
                next_cursor: None,
                dropped: 0,
            });
        }
        fake
    }
    fn calibre_is(self, found: bool) -> Self {
        lock(&self.script).calibre = VecDeque::from([found]);
        self
    }
    fn answer_calibre(&self, found: bool) {
        lock(&self.script).calibre = VecDeque::from([found]);
    }
    fn answer_prepare(&self, answer: Result<(), DownloadError>) {
        lock(&self.script).prepare.push_back(answer);
    }
    fn prepared(&self) -> Vec<Prepared> {
        lock(&self.script).prepared.clone()
    }
    fn calibre_asked(&self) -> Vec<bool> {
        lock(&self.script).calibre_asked.clone()
    }
    fn downloads(&self) -> Vec<(JobId, String)> {
        lock(&self.script)
            .queued
            .iter()
            .filter(|(_, _, kind)| *kind == JobKind::Download)
            .map(|(id, title, _)| (*id, title.clone()))
            .collect()
    }
    fn scans(&self) -> Vec<JobId> {
        lock(&self.script)
            .queued
            .iter()
            .filter(|(_, _, kind)| *kind == JobKind::Scan)
            .map(|(id, _, _)| *id)
            .collect()
    }
    fn cancelled(&self) -> Vec<JobId> {
        lock(&self.script).cancelled.clone()
    }
    fn title_of(&self, id: JobId) -> (String, JobKind) {
        lock(&self.script)
            .queued
            .iter()
            .find(|(job, _, _)| *job == id)
            .map(|(_, title, kind)| (title.clone(), *kind))
            .expect("a queued job")
    }
    fn view(&self, id: JobId, state: JobState) -> JobView {
        let (title, kind) = self.title_of(id);
        JobView {
            id,
            title,
            kind,
            state,
            progress: None,
            received: None,
            stage: None,
            error: None,
        }
    }
    /// The job is at this point; `Services::poll` reports it at the next tick.
    fn report(
        &self,
        id: JobId,
        state: JobState,
        stage: Option<Stage>,
        received: Option<u64>,
        total: Option<u64>,
    ) {
        let mut view = self.view(id, state);
        view.stage = stage;
        view.received = received;
        view.progress = received.zip(total);
        lock(&self.script)
            .updates
            .push(JobUpdate { view, result: None });
    }
    fn finish(&self, id: JobId, state: JobState, result: Result<JobOutput, String>) {
        let mut view = self.view(id, state);
        view.error = result.as_ref().err().cloned();
        lock(&self.script).updates.push(JobUpdate {
            view,
            result: Some(Arc::new(result)),
        });
    }
}

impl Services for Fake {
    fn enqueue(&self, title: String, request: JobRequest) -> JobId {
        let mut script = lock(&self.script);
        script.next_job += 1;
        let id = JobId(script.next_job);
        let kind = match request {
            JobRequest::External(_) => JobKind::Download,
            JobRequest::Scan { .. } => JobKind::Scan,
            _ => JobKind::Metadata,
        };
        script.queued.push((id, title.clone(), kind));
        // The real queue reports every job that is waiting or running at each poll.
        script.updates.push(JobUpdate {
            view: JobView {
                id,
                title,
                kind,
                state: JobState::Waiting,
                progress: None,
                received: None,
                stage: None,
                error: None,
            },
            result: None,
        });
        id
    }
    fn poll(&self) -> Vec<JobUpdate> {
        std::mem::take(&mut lock(&self.script).updates)
    }
    fn cancel(&self, id: JobId) {
        let view = self.view(id, JobState::Cancelled);
        let mut script = lock(&self.script);
        script.cancelled.push(id);
        script.updates.push(JobUpdate {
            view,
            result: Some(Arc::new(Err("Cancelled".into()))),
        });
    }
    fn clear_finished(&self) {}
    fn shutdown(&self) {}
    fn load_settings(&self) -> ServiceFuture<UiSettings> {
        Box::pin(async { Ok(UiSettings::default()) })
    }
    fn save_settings(&self, _: UiSettings) -> ServiceFuture<()> {
        Box::pin(async { Ok(()) })
    }
    fn find_cover(&self, _: PathBuf) -> ServiceFuture<Option<CoverFile>> {
        Box::pin(async { Ok(None) })
    }
    fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>> {
        Box::pin(async { Ok(None) })
    }
    fn load_library(&self) -> ServiceFuture<Option<PathBuf>> {
        Box::pin(async { Ok(None) })
    }
    fn save_library(&self, path: PathBuf) -> ServiceFuture<PathBuf> {
        Box::pin(async move { Ok(path) })
    }
    fn validate_library(&self, _: String) -> ServiceFuture<PathBuf> {
        Box::pin(async { Err("no".into()) })
    }
    fn list_plugins(&self) -> ServiceFuture<Vec<PluginRow>> {
        let rows = lock(&self.script).rows.clone();
        Box::pin(async move { Ok(rows) })
    }
    fn search_plugin(
        &self,
        _: String,
        _: String,
        _: u32,
        _: Option<String>,
        _: CancellationToken,
    ) -> SourceFuture<RemotePage> {
        let page = lock(&self.script).page.clone().expect("a page to give");
        Box::pin(async move { Ok(page) })
    }
    fn plugin_details(
        &self,
        _: String,
        hit: RemoteHit,
        _: CancellationToken,
    ) -> SourceFuture<RemoteHit> {
        Box::pin(async move { Ok::<_, SourceError>(hit) })
    }
    fn load_remote_cover(
        &self,
        _: String,
        _: String,
        _: CancellationToken,
    ) -> ServiceFuture<Handle> {
        Box::pin(async { Err("no covers here".into()) })
    }
    fn calibre_available(&self, recheck: bool) -> ServiceFuture<bool> {
        let mut script = lock(&self.script);
        script.calibre_asked.push(recheck);
        let answer = if script.calibre.len() > 1 {
            script.calibre.pop_front()
        } else {
            script.calibre.front().copied()
        }
        .unwrap_or(true);
        Box::pin(async move { Ok(answer) })
    }
    fn prepare_download(&self, request: DownloadRequest) -> DownloadFuture {
        let mut script = lock(&self.script);
        script.prepared.push(Prepared {
            plugin: request.plugin,
            offer: request.offer,
            library: request.library,
            export_dir: request.export_dir,
        });
        let answer = script.prepare.pop_front().unwrap_or(Ok(()));
        Box::pin(async move { answer.map(|()| Arc::new(Stub) as Arc<dyn ExternalJob>) })
    }
}

// ---- builders --------------------------------------------------------------------------------

fn plugin() -> PluginRow {
    PluginRow {
        id: "standard-ebooks".into(),
        name: "Standard Ebooks".into(),
        version: "1.0.0".into(),
        author: "Jane Doe".into(),
        description: "Free ebooks".into(),
        enabled: true,
        health: Health::Ready,
        info: Some(PluginInfo {
            permissions: PermissionSummary {
                domains: vec!["standardebooks.org".into()],
                user_address: false,
                call_timeout_s: 30,
                content: Vec::new(),
            },
            settings: Vec::new(),
            source: "Folder · D:\\Plugins".into(),
            installed: "17 Oct 2026".into(),
            search_hint: None,
        }),
    }
}

fn offer(id: &str, format: Format, size: Option<u64>) -> Offer {
    Offer {
        id: id.into(),
        format,
        archive: None,
        size,
        label: None,
    }
}

fn hit(id: &str, title: &str, author: &str, offers: Vec<Offer>) -> RemoteHit {
    RemoteHit::from(SearchHit {
        id: id.into(),
        title: title.into(),
        authors: vec![author.into()],
        offers,
        ..SearchHit::default()
    })
}

fn emma() -> RemoteHit {
    hit(
        "emma",
        "Emma",
        "Jane Austen",
        vec![offer("emma-epub", Format::Epub, Some(3_400_000))],
    )
}

fn persuasion() -> RemoteHit {
    hit(
        "persuasion",
        "Persuasion",
        "Jane Austen",
        vec![
            offer("p-epub", Format::Epub, Some(900_000)),
            offer("p-azw3", Format::Azw3, None),
        ],
    )
}

fn library_dir() -> PathBuf {
    PathBuf::from("library")
}

// ---- driving ---------------------------------------------------------------------------------

async fn run(state: &mut State, task: iced::Task<Message>, services: &dyn Services) {
    let mut tasks = VecDeque::from([task]);
    while let Some(task) = tasks.pop_front() {
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(action) = stream.next().await {
                match action {
                    iced_runtime::Action::Output(message) => {
                        tasks.push_back(app::update(state, message, services))
                    }
                    iced_runtime::Action::Widget(_) => {}
                    _ => panic!("unexpected runtime effect"),
                }
            }
        }
    }
}

fn send(state: &mut State, message: impl Into<Message>, services: &dyn Services) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let task = app::update(state, message.into(), services);
            run(state, task, services).await;
        });
}

fn press(state: &mut State, hit: &str, offer: Option<&str>, services: &dyn Services) {
    send(
        state,
        Message::Download(DownloadMsg::Press {
            hit: hit.into(),
            offer: offer.map(str::to_owned),
        }),
        services,
    );
}

/// Moves the clock on, so that what the queue reported is read.
fn tick(state: &mut State, services: &dyn Services) {
    send(state, Message::Tick, services);
}

fn simulator<'a>(state: &'a State) -> iced_test::Simulator<'a, Message> {
    headless();
    iced_test::Simulator::with_size(
        iced::Settings::default(),
        iced::Size::new(1100.0, 1800.0),
        app::view(state),
    )
}

fn assert_shows(state: &State, texts: &[&str]) {
    let mut ui = simulator(state);
    for text in texts {
        assert!(ui.find(*text).is_ok(), "{text:?} should be shown");
    }
}

fn assert_hides(state: &State, texts: &[&str]) {
    let mut ui = simulator(state);
    for text in texts {
        assert!(ui.find(*text).is_err(), "{text:?} must not be shown");
    }
}

fn click(state: &mut State, label: &str, services: &dyn Services) {
    let messages: Vec<Message> = {
        let mut ui = simulator(state);
        ui.click(label)
            .unwrap_or_else(|e| panic!("cannot click {label:?}: {e:?}"));
        ui.into_messages().collect()
    };
    assert!(!messages.is_empty(), "clicking {label:?} sent nothing");
    for message in messages {
        send(state, message, services);
    }
}

fn clicked_messages(state: &State, label: &str) -> usize {
    let mut ui = simulator(state);
    ui.click(label)
        .unwrap_or_else(|e| panic!("cannot find {label:?}: {e:?}"));
    ui.into_messages().count()
}

/// Search with the plugin chosen, a query sent and the first result selected.
fn results(fake: &Fake) -> State {
    let mut state = State::default();
    state.window_width = 1100.0;
    state.viewport_width = 700.0;
    state.list_height = 500.0;
    state.library = Some(library_dir());
    send(&mut state, Message::Navigate(Screen::Search), fake);
    send(
        &mut state,
        Message::Remote(Msg::SourceChosen(SearchSource::Plugin(
            "standard-ebooks".into(),
        ))),
        fake,
    );
    send(
        &mut state,
        Message::Remote(Msg::QueryChanged("austen".into())),
        fake,
    );
    send(&mut state, Message::Remote(Msg::Submit), fake);
    state
}

fn select(state: &mut State, id: &str, fake: &Fake) {
    send(state, Message::Remote(Msg::Select(id.into())), fake);
}

fn icon_of(state: &State, id: &str) -> Icon {
    let hit = state.remote.hit(id).expect("a result");
    downloads::hit_icon(state, hit)
}

fn offer_icon(state: &State, id: &str, offer: &str) -> Icon {
    let hit = state.remote.hit(id).expect("a result");
    let offer = hit.offers.iter().find(|o| o.id == offer).expect("an offer");
    downloads::offer_icon(state, hit, offer)
}

fn toast_texts(state: &State) -> Vec<(String, bool)> {
    state
        .toasts
        .iter()
        .map(|t| (t.message.clone(), t.error))
        .collect()
}

fn added(title: &str, id: u64) -> Result<JobOutput, String> {
    Ok(JobOutput::Added(AddedBook {
        library: library_dir(),
        title: title.into(),
        outcome: AddOutcome::Added { id },
    }))
}

/// A download of `emma` that was started and is at the front of the queue.
fn started(fake: &Fake) -> (State, JobId) {
    let mut state = results(fake);
    press(&mut state, "emma", None, fake);
    tick(&mut state, fake);
    let (id, _) = fake.downloads().remove(0);
    (state, id)
}

// ---- the states of the icon ------------------------------------------------------------------

#[test]
fn a_result_with_a_file_offers_a_download_and_one_without_says_so() {
    let nothing = hit("nothing", "Lost Book", "Nobody", Vec::new());
    let fake = Fake::new(vec![emma(), nothing]);
    let mut state = results(&fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Idle);
    assert_eq!(icon_of(&state, "nothing"), Icon::NoFile);
    assert_eq!(Icon::Idle.tip(), "Download and add to your library");
    assert_eq!(Icon::NoFile.tip(), "This result has no downloadable file");

    // The details pane has the labelled button of the offer.
    select(&mut state, "emma", &fake);
    assert_shows(&state, &["DOWNLOADS", "Download", "EPUB"]);
    select(&mut state, "nothing", &fake);
    assert_shows(
        &state,
        &["This source offers no downloadable file for this book."],
    );
    // The icon of a result without a file does nothing.
    press(&mut state, "nothing", None, &fake);
    assert!(fake.prepared().is_empty());
}

#[test]
fn one_offer_downloads_on_a_click_and_the_icon_follows_the_job() {
    let fake = Fake::new(vec![emma()]);
    let mut state = results(&fake);
    select(&mut state, "emma", &fake);

    // Clicking the labelled button of the offer asks the service for the job, then queues it.
    click(&mut state, "Download", &fake);
    let prepared = fake.prepared();
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].plugin, "standard-ebooks");
    assert_eq!(prepared[0].offer, "emma-epub");
    assert_eq!(prepared[0].library, library_dir());
    assert_eq!(prepared[0].export_dir, None);
    let (job, title) = fake.downloads().remove(0);
    assert_eq!(title, "Download · Emma (EPUB)");
    assert_eq!(icon_of(&state, "emma"), Icon::Queued);
    assert_shows(&state, &["Waiting"]);

    // Waiting in the queue, then the link, then bytes with a known total.
    fake.report(job, JobState::Waiting, None, None, None);
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Queued);
    fake.report(job, JobState::Running, Some(Stage::Downloading), None, None);
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Downloading(None));
    fake.report(
        job,
        JobState::Running,
        Some(Stage::Downloading),
        Some(1_530_000),
        Some(3_400_000),
    );
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Downloading(Some(45)));
    assert_eq!(Icon::Downloading(Some(45)).tip(), "Downloading 45%");
    assert_shows(&state, &["45%"]);

    // Calibre has the file: no cancelling, and the icon says so.
    fake.report(
        job,
        JobState::Running,
        Some(Stage::Adding),
        Some(3_400_000),
        Some(3_400_000),
    );
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Adding);
    assert_shows(&state, &["Adding…"]);

    // Added: the icon, a toast, and the library is read again.
    fake.finish(job, JobState::Succeeded, added("Emma", 7));
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Done);
    assert_shows(&state, &["Added"]);
    assert_eq!(
        toast_texts(&state),
        [("Added to your library: Emma".to_owned(), false)]
    );
    assert_eq!(fake.scans().len(), 1, "the library is scanned again");
    // A finished download cannot be started a second time from the same icon.
    press(&mut state, "emma", None, &fake);
    assert_eq!(fake.prepared().len(), 1);
}

#[test]
fn an_offer_without_a_known_size_spins_and_a_known_one_fills() {
    let fake = Fake::new(vec![persuasion()]);
    let mut state = results(&fake);
    select(&mut state, "persuasion", &fake);
    press(&mut state, "persuasion", Some("p-azw3"), &fake);
    press(&mut state, "persuasion", Some("p-epub"), &fake);
    tick(&mut state, &fake);
    let jobs = fake.downloads();
    assert_eq!(jobs.len(), 2);
    fake.report(
        jobs[0].0,
        JobState::Running,
        Some(Stage::Downloading),
        Some(500_000),
        None,
    );
    fake.report(
        jobs[1].0,
        JobState::Running,
        Some(Stage::Downloading),
        Some(450_000),
        Some(900_000),
    );
    tick(&mut state, &fake);
    assert_eq!(
        offer_icon(&state, "persuasion", "p-azw3"),
        Icon::Downloading(None)
    );
    assert_eq!(
        offer_icon(&state, "persuasion", "p-epub"),
        Icon::Downloading(Some(50))
    );
}

// ---- several offers ---------------------------------------------------------------------------

#[test]
fn several_offers_open_a_menu_of_formats_and_a_choice_starts_that_one() {
    let fake = Fake::new(vec![persuasion()]);
    let mut state = results(&fake);
    state.pointer_x = 600.0;
    state.pointer_y = 200.0;

    press(&mut state, "persuasion", None, &fake);
    assert!(fake.prepared().is_empty(), "the icon only opens the menu");
    let menu = state.downloads.menu.clone().expect("the menu is open");
    assert_eq!(menu.hit, "persuasion");
    assert_eq!((menu.at.x, menu.at.y), (600.0, 200.0));
    assert_shows(&state, &["DOWNLOAD AS", "EPUB", "AZW3", "size unknown"]);

    // The same icon again closes it; so does Esc.
    press(&mut state, "persuasion", None, &fake);
    assert!(state.downloads.menu.is_none());
    press(&mut state, "persuasion", None, &fake);
    assert!(state.downloads.menu.is_some());
    let esc = {
        use iced::keyboard::{Event, Key, Location, key::Named, key::Physical};
        Message::Keyboard(Event::KeyPressed {
            key: Key::Named(Named::Escape),
            modified_key: Key::Named(Named::Escape),
            physical_key: Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        })
    };
    send(&mut state, esc, &fake);
    assert!(state.downloads.menu.is_none(), "Esc closes the menu");
    assert!(state.remote.session.is_some(), "and nothing else");

    // Choosing a format downloads that one and closes the menu.
    press(&mut state, "persuasion", None, &fake);
    press(&mut state, "persuasion", Some("p-azw3"), &fake);
    assert!(state.downloads.menu.is_none());
    assert_eq!(fake.prepared()[0].offer, "p-azw3");
    assert_eq!(fake.downloads()[0].1, "Download · Persuasion (AZW3)");
    assert_eq!(offer_icon(&state, "persuasion", "p-azw3"), Icon::Queued);
    assert_eq!(offer_icon(&state, "persuasion", "p-epub"), Icon::Idle);
    // The result shows the busiest of its offers.
    assert_eq!(icon_of(&state, "persuasion"), Icon::Queued);

    // A format that is under way is greyed in the menu: it cannot be started twice.
    press(&mut state, "persuasion", None, &fake);
    press(&mut state, "persuasion", Some("p-azw3"), &fake);
    assert_eq!(fake.prepared().len(), 1);
}

#[test]
fn the_menu_goes_with_a_new_search() {
    let fake = Fake::new(vec![persuasion()]);
    let mut state = results(&fake);
    press(&mut state, "persuasion", None, &fake);
    assert!(state.downloads.menu.is_some());
    send(&mut state, Message::Remote(Msg::Submit), &fake);
    assert!(state.downloads.menu.is_none());
}

// ---- cancel, failure, already there ------------------------------------------------------------

#[test]
fn cancelling_a_download_puts_the_icon_back() {
    let fake = Fake::new(vec![emma()]);
    let (mut state, job) = started(&fake);
    fake.report(
        job,
        JobState::Running,
        Some(Stage::Downloading),
        Some(10),
        Some(100),
    );
    tick(&mut state, &fake);
    send(&mut state, Message::Cancel(job), &fake);
    assert_eq!(fake.cancelled(), [job]);
    assert_eq!(icon_of(&state, "emma"), Icon::Idle);
    assert_eq!(
        toast_texts(&state),
        [("Cancelled · Download · Emma (EPUB)".to_owned(), false)]
    );
    // It can be asked for again.
    press(&mut state, "emma", None, &fake);
    assert_eq!(fake.prepared().len(), 2);
}

#[test]
fn a_failed_download_keeps_the_reason_and_the_kept_file_and_can_be_tried_again() {
    let fake = Fake::new(vec![emma()]);
    let (mut state, job) = started(&fake);
    let reason = "Could not add the book to the library. calibredb exited with code 1. \
                  The downloaded file was saved to D:\\Exports\\Emma.epub";
    fake.finish(job, JobState::Failed, Err(reason.into()));
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Failed);
    assert_eq!(Icon::Failed.tip(), "Download failed — click to try again");
    assert_eq!(Icon::Failed.label(), "Try again");
    // The toast stays until it is dismissed, and names the file that was kept.
    assert_eq!(toast_texts(&state), [(reason.to_owned(), true)]);
    assert_eq!(fake.scans().len(), 0, "nothing was added: no rescan");

    // Jobs lists it with the same words.
    send(&mut state, Message::Navigate(Screen::Jobs), &fake);
    assert_shows(
        &state,
        &[
            "Download · Emma (EPUB)",
            "Standard Ebooks · Could not add the book to the library. calibredb exited with code 1. The downloaded file was saved to D:\\Exports\\Emma.epub",
        ],
    );

    // A click starts it again.
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    press(&mut state, "emma", None, &fake);
    assert_eq!(fake.prepared().len(), 2);
    assert_eq!(icon_of(&state, "emma"), Icon::Queued);
}

#[test]
fn a_book_calibre_already_has_is_not_an_error() {
    let fake = Fake::new(vec![emma()]);
    let (mut state, job) = started(&fake);
    fake.finish(
        job,
        JobState::Succeeded,
        Ok(JobOutput::Added(AddedBook {
            library: library_dir(),
            title: "Emma".into(),
            outcome: AddOutcome::AlreadyExists,
        })),
    );
    tick(&mut state, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Exists);
    assert_eq!(Icon::Exists.tip(), "Already in your library");
    assert_eq!(
        toast_texts(&state),
        [("Already in your library: Emma".to_owned(), false)],
        "a notice, not an error"
    );
    assert_eq!(fake.scans().len(), 0, "a duplicate changed nothing");
    assert_eq!(
        state.job_messages[&job],
        "Already in your library · not added again"
    );
    // The mark is final: a click does not download it again.
    press(&mut state, "emma", None, &fake);
    assert_eq!(fake.prepared().len(), 1);
}

#[test]
fn the_service_can_refuse_the_job_and_say_why() {
    let fake = Fake::new(vec![emma()]);
    fake.answer_prepare(Err(DownloadError::Failed(
        "The saved library library is unavailable. Open it again or choose another folder.".into(),
    )));
    let mut state = results(&fake);
    press(&mut state, "emma", None, &fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Failed);
    assert_eq!(
        toast_texts(&state),
        [(
            "The saved library library is unavailable. Open it again or choose another folder."
                .to_owned(),
            true
        )]
    );
    assert!(fake.downloads().is_empty(), "no job was queued");
}

#[test]
fn without_an_open_library_nothing_is_started() {
    let fake = Fake::new(vec![emma()]);
    let mut state = results(&fake);
    state.library = None;
    press(&mut state, "emma", None, &fake);
    assert!(fake.prepared().is_empty());
    assert_eq!(
        toast_texts(&state),
        [("Open a library before downloading a book.".to_owned(), true)]
    );
}

#[test]
fn the_export_folder_of_the_settings_goes_with_the_job() {
    let fake = Fake::new(vec![emma()]);
    let mut state = results(&fake);
    let mut settings = state.visible_settings().clone();
    settings.export_folder = Some(PathBuf::from("D:\\Exports"));
    send(&mut state, Message::SaveSettings(settings), &fake);
    press(&mut state, "emma", None, &fake);
    assert_eq!(
        fake.prepared()[0].export_dir,
        Some(PathBuf::from("D:\\Exports"))
    );
}

// ---- Calibre ---------------------------------------------------------------------------------

#[test]
fn without_calibre_the_icon_is_dim_and_explains_what_to_install() {
    let fake = Fake::new(vec![emma()]).calibre_is(false);
    let mut state = results(&fake);
    assert_eq!(state.downloads.calibre, Some(false));
    assert_eq!(icon_of(&state, "emma"), Icon::NeedsCalibre);
    assert_eq!(
        Icon::NeedsCalibre.tip(),
        "Calibre is needed to add downloaded books"
    );
    assert_eq!(Icon::NeedsCalibre.label(), "Needs Calibre");

    // A click says why, in a toast, and starts nothing.
    press(&mut state, "emma", None, &fake);
    assert!(fake.prepared().is_empty());
    assert_eq!(
        toast_texts(&state),
        [(
            "Calibre is needed to add downloaded books. Install Calibre, then try again."
                .to_owned(),
            true
        )]
    );

    // The details pane has the card with the command.
    select(&mut state, "emma", &fake);
    assert_shows(
        &state,
        &[
            "Calibre is needed to add downloaded books",
            "winget install --id calibre.calibre",
            "Check again",
            "Needs Calibre",
        ],
    );
}

#[test]
fn check_again_finds_calibre_once_it_is_installed() {
    let fake = Fake::new(vec![emma()]).calibre_is(false);
    let mut state = results(&fake);
    select(&mut state, "emma", &fake);

    // Still missing: said so, and the card stays.
    click(&mut state, "Check again", &fake);
    assert_eq!(
        fake.calibre_asked(),
        [false, true],
        "the second look is a recheck"
    );
    assert_eq!(state.downloads.calibre, Some(false));
    assert!(toast_texts(&state).iter().any(|(text, error)| text
        == "Calibre was still not found. Install it, then try again."
        && *error));

    // Installed since: the card goes, the icon is the usual one.
    fake.answer_calibre(true);
    click(&mut state, "Check again", &fake);
    assert_eq!(state.downloads.calibre, Some(true));
    assert_eq!(icon_of(&state, "emma"), Icon::Idle);
    assert_hides(
        &state,
        &["winget install --id calibre.calibre", "Check again"],
    );
}

#[test]
fn calibre_that_goes_missing_while_downloading_is_noticed() {
    let fake = Fake::new(vec![emma()]);
    fake.answer_prepare(Err(DownloadError::NeedsCalibre));
    let mut state = results(&fake);
    assert_eq!(icon_of(&state, "emma"), Icon::Idle);
    press(&mut state, "emma", None, &fake);
    assert_eq!(state.downloads.calibre, Some(false));
    assert_eq!(icon_of(&state, "emma"), Icon::NeedsCalibre);
    assert!(fake.downloads().is_empty());
}

// ---- the library already has the book -------------------------------------------------------

fn library_book(id: u64, author: &str, title: &str) -> Book {
    Book {
        id: BookId::Calibre(id),
        author: author.into(),
        title: title.into(),
        files: Vec::new(),
        path: PathBuf::from(format!("library/{author}/{title} ({id})")),
    }
}

#[test]
fn a_book_the_library_seems_to_have_is_marked_but_can_still_be_downloaded() {
    let fake = Fake::new(vec![emma(), persuasion()]);
    let mut state = results(&fake);
    state.books = vec![library_book(1, "Austen, Jane", "EMMA!")];
    downloads::library_changed(&mut state);
    // Same author (in another order and case) and title (without punctuation): marked.
    assert_eq!(icon_of(&state, "emma"), Icon::Exists);
    assert_eq!(icon_of(&state, "persuasion"), Icon::Idle);
    assert_eq!(Icon::Exists.label(), "In library");

    // Only a mark: the click downloads, as it would for any result.
    press(&mut state, "emma", None, &fake);
    assert_eq!(fake.prepared().len(), 1);
    assert_eq!(icon_of(&state, "emma"), Icon::Queued);
}

#[test]
fn a_book_that_was_added_is_selected_once_the_library_is_scanned_again() {
    let fake = Fake::new(vec![emma()]);
    let (mut state, job) = started(&fake);
    fake.finish(job, JobState::Succeeded, added("Emma", 7));
    tick(&mut state, &fake);
    let scan = fake.scans().remove(0);
    assert_eq!(state.active_scan, Some(scan));

    let books = vec![
        library_book(3, "Charlotte Brontë", "Villette"),
        library_book(7, "Austen, Jane", "Emma"),
    ];
    fake.finish(
        scan,
        JobState::Succeeded,
        Ok(JobOutput::Scan(ScanReport {
            books,
            warnings: Vec::new(),
        })),
    );
    tick(&mut state, &fake);
    assert_eq!(
        state.selection.selected.iter().collect::<Vec<_>>(),
        [&BookId::Calibre(7)],
        "the new book is selected in the library"
    );
    // And the search result now carries the mark.
    assert_eq!(icon_of(&state, "emma"), Icon::Done);
}

// ---- the Jobs screen --------------------------------------------------------------------------

#[test]
fn jobs_lists_a_download_with_its_stages_and_a_cancel_that_stops_working_at_the_end() {
    let fake = Fake::new(vec![emma()]);
    let (mut state, job) = started(&fake);
    send(&mut state, Message::Navigate(Screen::Jobs), &fake);

    assert_shows(
        &state,
        &["Download · Emma (EPUB)", "Standard Ebooks · Waiting"],
    );
    assert_eq!(clicked_messages(&state, "Cancel"), 1);

    fake.report(job, JobState::Running, Some(Stage::Downloading), None, None);
    tick(&mut state, &fake);
    assert_shows(&state, &["Standard Ebooks · Getting link…"]);

    fake.report(
        job,
        JobState::Running,
        Some(Stage::Downloading),
        Some(1_200_000),
        Some(3_400_000),
    );
    tick(&mut state, &fake);
    assert_shows(
        &state,
        &["Standard Ebooks · Downloading · 1.2 MB of 3.4 MB"],
    );

    fake.report(
        job,
        JobState::Running,
        Some(Stage::Downloading),
        Some(1_200_000),
        None,
    );
    tick(&mut state, &fake);
    assert_shows(&state, &["Standard Ebooks · Downloading · 1.2 MB received"]);
    assert_eq!(clicked_messages(&state, "Cancel"), 1);

    // Calibre is adding the book: Cancel stays but does nothing.
    fake.report(
        job,
        JobState::Running,
        Some(Stage::Adding),
        Some(3_400_000),
        Some(3_400_000),
    );
    tick(&mut state, &fake);
    assert_shows(&state, &["Standard Ebooks · Adding to library…"]);
    assert_eq!(clicked_messages(&state, "Cancel"), 0, "Cancel is inactive");

    fake.finish(job, JobState::Succeeded, added("Emma", 7));
    tick(&mut state, &fake);
    assert_shows(
        &state,
        &["Standard Ebooks · Added to your library · rescanning"],
    );
    // The rescan is a job too; once it is done nothing in the list can be cancelled.
    let scan = fake.scans().remove(0);
    fake.finish(
        scan,
        JobState::Succeeded,
        Ok(JobOutput::Scan(ScanReport {
            books: Vec::new(),
            warnings: Vec::new(),
        })),
    );
    tick(&mut state, &fake);
    assert_hides(&state, &["Cancel"]);
}

#[test]
fn jobs_says_what_went_wrong_when_the_site_sent_a_page() {
    let fake = Fake::new(vec![emma()]);
    let (mut state, job) = started(&fake);
    let reason = "The site did not return a book: it sent HTML, 150 bytes.";
    fake.finish(job, JobState::Failed, Err(reason.into()));
    tick(&mut state, &fake);
    send(&mut state, Message::Navigate(Screen::Jobs), &fake);
    assert_shows(&state, &[&format!("Standard Ebooks · {reason}")]);
    assert_eq!(icon_of(&state, "emma"), Icon::Failed);
}

// ---- the views --------------------------------------------------------------------------------

#[test]
fn every_state_of_the_icon_draws_in_the_list_the_grid_and_the_details() {
    let fake = Fake::new(vec![emma(), persuasion()]);
    let mut state = results(&fake);
    select(&mut state, "emma", &fake);
    // The list, then the cover grid.
    assert_shows(&state, &["Emma", "Persuasion"]);
    send(&mut state, Message::Remote(Msg::ShowGrid), &fake);
    assert_shows(&state, &["Emma", "Persuasion"]);
    press(&mut state, "emma", None, &fake);
    tick(&mut state, &fake);
    let (job, _) = fake.downloads().remove(0);
    for (stage, received, total) in [
        (None, None, None),
        (Some(Stage::Downloading), None, None),
        (Some(Stage::Downloading), Some(10), Some(100)),
        (Some(Stage::Downloading), Some(10), None),
        (Some(Stage::Adding), Some(100), Some(100)),
    ] {
        fake.report(job, JobState::Running, stage, received, total);
        tick(&mut state, &fake);
        send(&mut state, Message::Remote(Msg::ShowList), &fake);
        assert_shows(&state, &["Emma"]);
        send(&mut state, Message::Remote(Msg::ShowGrid), &fake);
        assert_shows(&state, &["Emma"]);
    }
    // The spinner turns with the clock while a download is going.
    let before = state.downloads.spin();
    tick(&mut state, &fake);
    send(&mut state, Message::Clock(std::time::Instant::now()), &fake);
    assert!(state.downloads.spin() > before);
    assert!(downloads::animating(&state));
}

// ---- the whole chain, with the real runtime and the real calibredb ----------------------------

/// The sample plugin searches a local stand-in of an OPDS catalog, a book is downloaded from it
/// and added to a library by the real `calibredb`, and the library is scanned again. Only the
/// site is a stand-in. It stays out of the usual run, like the other tests that need Calibre:
/// `cargo test -p ferrolibri-ui --test downloads -- --ignored`
mod real {
    use super::*;
    use ferrolibri_core::{calibre, scanner};
    use ferrolibri_plugins::{
        Downgrade, PluginStore, SettingValue,
        testing::{TestServer, opds},
    };
    use ferrolibri_ui::services::RealServices;
    use std::time::{Duration, Instant};

    fn wait_for(
        state: &mut State,
        services: &dyn Services,
        what: &str,
        mut done: impl FnMut(&State) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(90);
        while !done(state) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
            send(state, Message::Tick, services);
        }
    }

    #[test]
    #[ignore = "needs Calibre: runs the real calibredb"]
    fn a_found_book_is_downloaded_added_by_calibre_and_found_in_the_library() {
        let cli = calibre::require().expect("install Calibre to run this test");
        // A short path: Calibre refuses library paths over 88 characters on Windows.
        let root = tempfile::Builder::new().prefix("fl").tempdir().unwrap();
        let library = root.path().join("lib");
        std::fs::create_dir(&library).unwrap();
        cli.create_library(&library, &CancellationToken::default())
            .unwrap();

        let services =
            RealServices::with_paths(root.path().join("config.toml"), root.path().join("ui.toml"))
                .unwrap();
        let server = TestServer::start(opds::handle);
        let store = PluginStore::new(
            root.path().join("plugins"),
            root.path().join("plugins.toml"),
        );
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ferrolibri-plugins/tests/fixtures/sample-opds");
        let inspection = store.inspect_local(&fixture).unwrap();
        store.install(&inspection, Downgrade::Refuse).unwrap();
        store
            .save_settings(
                "sample-opds",
                &[("base_url".to_owned(), SettingValue::Text(server.url("/")))]
                    .into_iter()
                    .collect(),
            )
            .unwrap();
        let services = services.with_plugin_http(store, server.http());

        let mut state = State::default();
        state.window_width = 1100.0;
        state.viewport_width = 700.0;
        state.list_height = 500.0;
        state.library = Some(library.clone());
        send(&mut state, Message::Navigate(Screen::Search), &services);
        send(
            &mut state,
            Message::Remote(Msg::SourceChosen(SearchSource::Plugin(
                "sample-opds".into(),
            ))),
            &services,
        );
        send(
            &mut state,
            Message::Remote(Msg::QueryChanged("и".into())),
            &services,
        );
        send(&mut state, Message::Remote(Msg::Submit), &services);
        wait_for(&mut state, &services, "Calibre to be found", |s| {
            s.downloads.calibre.is_some()
        });
        assert_eq!(state.downloads.calibre, Some(true));

        let hit = state
            .remote
            .session
            .as_ref()
            .unwrap()
            .hits
            .iter()
            .find(|hit| !hit.offers.is_empty())
            .expect("a result with an offer")
            .clone();
        assert_eq!(downloads::hit_icon(&state, &hit), Icon::Idle);
        press(&mut state, &hit.id, None, &services);
        // The job is queued, and may already be under way.
        assert!(
            downloads::hit_icon(&state, &hit).is_busy(),
            "{:?}",
            downloads::hit_icon(&state, &hit)
        );
        wait_for(&mut state, &services, "the download to end", |s| {
            matches!(
                downloads::hit_icon(s, &hit),
                Icon::Done | Icon::Exists | Icon::Failed
            )
        });
        assert_eq!(
            downloads::hit_icon(&state, &hit),
            Icon::Done,
            "{:?}",
            toast_texts(&state)
        );
        assert!(
            toast_texts(&state)
                .iter()
                .any(|(text, error)| text.starts_with("Added to your library") && !error)
        );

        // The app scanned the library again by itself, and the book is in it and selected.
        wait_for(&mut state, &services, "the rescan", |s| {
            s.active_scan.is_none()
        });
        let report = scanner::scan_library(&library).unwrap();
        assert_eq!(report.books.len(), 1, "{:?}", report.books);
        assert_eq!(state.books.len(), 1);
        assert_eq!(
            state.selection.selected.len(),
            1,
            "the new book is selected"
        );

        drop(server);
    }
}
