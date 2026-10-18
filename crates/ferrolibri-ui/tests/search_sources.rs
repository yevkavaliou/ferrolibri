//! Search sources and the results of a plugin (plan item 12): the real `view` and `update`
//! driven through the headless simulator, with a scripted `Services` standing in for the plugin
//! runtime, and a last group that runs a real plugin against a local catalog stand-in.
//!
//! The scripted service can hold an answer back until the test lets it go, which is how an
//! answer that arrives late (A → B → A) is made to arrive late.
use ferrolibri_core::{converter::CancellationToken, cover::CoverFile, jobs::JobRequest};
use ferrolibri_ui::{
    app::{self, Message, Screen, State},
    remote::{DetailsPhase, More, Msg, Phase, SearchSource},
    services::{
        RealServices, ServiceFuture, Services,
        jobs::{JobId, JobUpdate},
        plugins::{
            FieldKind, FieldValue, Health, PermissionSummary, PluginInfo, PluginRow, RemoteHit,
            RemoteOffer, RemotePage, SettingField, SourceError, SourceErrorKind, SourceFuture,
        },
    },
    settings::UiSettings,
};
use iced::{futures::StreamExt, widget::image::Handle};
use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::{Mutex, MutexGuard, PoisonError},
};
use tokio::sync::oneshot;

fn headless() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { std::env::set_var("ICED_TEST_BACKEND", "tiny-skia") });
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the scripted services -----------------------------------------------------------------

type SearchAnswer = Result<RemotePage, SourceError>;
type DetailsAnswer = Result<RemoteHit, SourceError>;

/// A call of `search_plugin`, with the means to answer it.
struct Search {
    plugin: String,
    text: String,
    page: u32,
    cursor: Option<String>,
    token: CancellationToken,
    gate: Option<oneshot::Sender<SearchAnswer>>,
}

struct Details {
    plugin: String,
    id: String,
    token: CancellationToken,
    gate: Option<oneshot::Sender<DetailsAnswer>>,
}

#[derive(Default)]
struct Script {
    rows: Vec<PluginRow>,
    /// Answers given at once; when there are none, the call waits for the test.
    search_answers: VecDeque<SearchAnswer>,
    details_answers: VecDeque<DetailsAnswer>,
    searches: Vec<Search>,
    details: Vec<Details>,
    covers: Vec<String>,
    bad_covers: BTreeSet<String>,
    saved: Vec<UiSettings>,
    opened: Vec<String>,
}

#[derive(Default)]
struct Fake {
    script: Mutex<Script>,
}

impl Fake {
    fn with_rows(rows: Vec<PluginRow>) -> Self {
        let fake = Self::default();
        lock(&fake.script).rows = rows;
        fake
    }
    fn answer_searches(&self, answers: impl IntoIterator<Item = SearchAnswer>) {
        lock(&self.script).search_answers.extend(answers);
    }
    fn answer_details(&self, answers: impl IntoIterator<Item = DetailsAnswer>) {
        lock(&self.script).details_answers.extend(answers);
    }
    fn searches(&self) -> Vec<(String, u32, Option<String>)> {
        lock(&self.script)
            .searches
            .iter()
            .map(|s| (s.text.clone(), s.page, s.cursor.clone()))
            .collect()
    }
    fn search_was_cancelled(&self, index: usize) -> bool {
        lock(&self.script).searches[index].token.is_cancelled()
    }
    /// Lets the call number `index` end with `answer`.
    fn release_search(&self, index: usize, answer: SearchAnswer) {
        let gate = lock(&self.script).searches[index]
            .gate
            .take()
            .expect("the call is waiting");
        let _ = gate.send(answer);
    }
    fn release_details(&self, index: usize, answer: DetailsAnswer) {
        let gate = lock(&self.script).details[index]
            .gate
            .take()
            .expect("the call is waiting");
        let _ = gate.send(answer);
    }
    fn detail_calls(&self) -> Vec<String> {
        lock(&self.script)
            .details
            .iter()
            .map(|d| d.id.clone())
            .collect()
    }
    fn covers(&self) -> Vec<String> {
        lock(&self.script).covers.clone()
    }
    fn saved_sources(&self) -> Vec<Option<String>> {
        lock(&self.script)
            .saved
            .iter()
            .map(|s| s.search_source.clone())
            .collect()
    }
    fn set_rows(&self, rows: Vec<PluginRow>) {
        lock(&self.script).rows = rows;
    }
}

impl Services for Fake {
    fn enqueue(&self, _: String, _: JobRequest) -> JobId {
        unreachable!("searching enqueues no jobs")
    }
    fn poll(&self) -> Vec<JobUpdate> {
        Vec::new()
    }
    fn cancel(&self, _: JobId) {}
    fn clear_finished(&self) {}
    fn shutdown(&self) {}
    fn load_settings(&self) -> ServiceFuture<UiSettings> {
        Box::pin(async { Ok(UiSettings::default()) })
    }
    fn save_settings(&self, settings: UiSettings) -> ServiceFuture<()> {
        lock(&self.script).saved.push(settings);
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
        id: String,
        text: String,
        page: u32,
        cursor: Option<String>,
        cancel: CancellationToken,
    ) -> SourceFuture<RemotePage> {
        let mut script = lock(&self.script);
        let ready = script.search_answers.pop_front();
        let (gate, receiver) = match ready {
            Some(_) => (None, None),
            None => {
                let (sender, receiver) = oneshot::channel();
                (Some(sender), Some(receiver))
            }
        };
        script.searches.push(Search {
            plugin: id,
            text,
            page,
            cursor,
            token: cancel,
            gate,
        });
        Box::pin(async move {
            match (ready, receiver) {
                (Some(answer), _) => answer,
                (None, Some(receiver)) => receiver.await.unwrap_or_else(|_| {
                    Err(SourceError::new(SourceErrorKind::Cancelled, "dropped"))
                }),
                (None, None) => unreachable!(),
            }
        })
    }
    fn plugin_details(
        &self,
        id: String,
        hit: RemoteHit,
        cancel: CancellationToken,
    ) -> SourceFuture<RemoteHit> {
        let mut script = lock(&self.script);
        let ready = script.details_answers.pop_front();
        let (gate, receiver) = match ready {
            Some(_) => (None, None),
            None => {
                let (sender, receiver) = oneshot::channel();
                (Some(sender), Some(receiver))
            }
        };
        script.details.push(Details {
            plugin: id,
            id: hit.id.clone(),
            token: cancel,
            gate,
        });
        Box::pin(async move {
            match (ready, receiver) {
                (Some(answer), _) => answer,
                (None, Some(receiver)) => receiver.await.unwrap_or_else(|_| {
                    Err(SourceError::new(SourceErrorKind::Cancelled, "dropped"))
                }),
                (None, None) => unreachable!(),
            }
        })
    }
    fn load_remote_cover(
        &self,
        _id: String,
        url: String,
        _cancel: CancellationToken,
    ) -> ServiceFuture<Handle> {
        let mut script = lock(&self.script);
        script.covers.push(url.clone());
        let bad = script.bad_covers.contains(&url);
        Box::pin(async move {
            if bad {
                Err("the server answered 404".into())
            } else {
                Ok(Handle::from_rgba(1, 1, vec![200, 30, 60, 255]))
            }
        })
    }
    fn open_url(&self, url: String) -> ServiceFuture<()> {
        lock(&self.script).opened.push(url);
        Box::pin(async { Ok(()) })
    }
}

// ---- builders --------------------------------------------------------------------------------

fn permissions(domains: &[&str]) -> PermissionSummary {
    PermissionSummary {
        domains: domains.iter().map(|d| (*d).to_owned()).collect(),
        user_address: false,
        call_timeout_s: 30,
        content: Vec::new(),
    }
}

fn plugin(id: &str, name: &str) -> PluginRow {
    PluginRow {
        id: id.into(),
        name: name.into(),
        version: "1.0.0".into(),
        author: "Jane Doe".into(),
        description: "Free ebooks".into(),
        enabled: true,
        health: Health::Ready,
        info: Some(PluginInfo {
            permissions: permissions(&["standardebooks.org"]),
            settings: Vec::new(),
            source: "Folder · D:\\Plugins".into(),
            installed: "17 Oct 2026".into(),
            search_hint: None,
        }),
    }
}

fn standard_ebooks() -> PluginRow {
    plugin("standard-ebooks", "Standard Ebooks")
}

fn with_hint(mut row: PluginRow, hint: &str) -> PluginRow {
    row.info.as_mut().unwrap().search_hint = Some(hint.into());
    row
}

fn address_field(saved: Option<&str>) -> SettingField {
    SettingField {
        key: "base_url".into(),
        label: "Catalog address".into(),
        kind: FieldKind::Url,
        network: true,
        value: FieldValue::Text(saved.unwrap_or_default().into()),
        placeholder: Some("https://example.org".into()),
        saved: saved.is_some(),
    }
}

fn with_address(mut row: PluginRow, saved: Option<&str>) -> PluginRow {
    let info = row.info.as_mut().unwrap();
    info.permissions.domains.clear();
    info.permissions.user_address = true;
    info.settings = vec![address_field(saved)];
    row
}

fn hit(id: &str, title: &str) -> RemoteHit {
    RemoteHit {
        authors: vec!["Jane Austen".into()],
        offers: vec![offer("o1", "EPUB", Some(812_000))],
        ..RemoteHit::titled(id, title)
    }
}

fn offer(id: &str, format: &str, size: Option<u64>) -> RemoteOffer {
    RemoteOffer {
        id: id.into(),
        format: format.into(),
        zip: false,
        size,
        label: None,
    }
}

fn page(hits: Vec<RemoteHit>, next: Option<&str>) -> SearchAnswer {
    Ok(RemotePage {
        hits,
        next_cursor: next.map(str::to_owned),
        dropped: 0,
    })
}

fn failure(kind: SourceErrorKind, message: &str) -> SearchAnswer {
    Err(SourceError::new(kind, message))
}

// ---- driving ---------------------------------------------------------------------------------

type Actions = iced::futures::stream::BoxStream<'static, iced_runtime::Action<Message>>;

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

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// Sends a message and everything it sets in motion. A call that waits for the test would wait
/// forever here: use [`launch`] and [`deliver`] for those.
fn send(state: &mut State, message: impl Into<Message>, services: &dyn Services) {
    block_on(async {
        let task = app::update(state, message.into(), services);
        run(state, task, services).await;
    });
}

fn remote(state: &mut State, msg: Msg, services: &dyn Services) {
    send(state, Message::Remote(msg), services);
}

/// Sends a message but does not wait for what it started; the work is returned.
fn launch(state: &mut State, message: impl Into<Message>, services: &dyn Services) -> Vec<Actions> {
    iced_runtime::task::into_stream(app::update(state, message.into(), services))
        .into_iter()
        .collect()
}

/// Runs work from [`launch`] to its end, in the order given, and handles what it produced.
fn deliver(state: &mut State, work: Vec<Actions>, services: &dyn Services) {
    block_on(async {
        for mut stream in work {
            while let Some(action) = stream.next().await {
                match action {
                    iced_runtime::Action::Output(message) => {
                        let task = app::update(state, message, services);
                        run(state, task, services).await;
                    }
                    iced_runtime::Action::Widget(_) => {}
                    _ => panic!("unexpected runtime effect"),
                }
            }
        }
    });
}

fn simulator<'a>(state: &'a State) -> iced_test::Simulator<'a, Message> {
    headless();
    iced_test::Simulator::with_size(
        iced::Settings::default(),
        iced::Size::new(1100.0, 1800.0),
        app::view(state),
    )
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

/// What clicking `label` sends; nothing for an inactive button.
fn clicked_messages(state: &State, label: &str) -> usize {
    let mut ui = simulator(state);
    ui.click(label)
        .unwrap_or_else(|e| panic!("cannot find {label:?}: {e:?}"));
    ui.into_messages().count()
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

fn search_screen(fake: &Fake) -> State {
    let mut state = State::default();
    state.window_width = 1100.0;
    state.viewport_width = 700.0;
    state.list_height = 500.0;
    send(&mut state, Message::Navigate(Screen::Search), fake);
    state
}

/// Search with the plugin chosen.
fn with_source(fake: &Fake, id: &str) -> State {
    let mut state = search_screen(fake);
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin(id.into())),
        fake,
    );
    assert_eq!(
        state.visible_settings().search_source.as_deref(),
        Some(id),
        "the plugin is chosen"
    );
    state
}

fn type_and_search(state: &mut State, text: &str, services: &dyn Services) {
    remote(state, Msg::QueryChanged(text.into()), services);
    remote(state, Msg::Submit, services);
}

fn session(state: &State) -> &ferrolibri_ui::remote::Session {
    state.remote.session.as_ref().expect("a search was made")
}

fn titles(state: &State) -> Vec<String> {
    session(state)
        .hits
        .iter()
        .map(|h| h.title.clone())
        .collect()
}

fn key(named: iced::keyboard::key::Named, modifiers: iced::keyboard::Modifiers) -> Message {
    use iced::keyboard::{Event, Key, Location, key::Physical};
    Message::Keyboard(Event::KeyPressed {
        key: Key::Named(named),
        modified_key: Key::Named(named),
        physical_key: Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
        location: Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

// ---- the source switch (G4) ----------------------------------------------------------------

#[test]
fn the_switch_is_there_once_a_plugin_is_installed_and_gone_without_one() {
    let none = Fake::default();
    let state = search_screen(&none);
    assert_hides(&state, &["Standard Ebooks"]);
    assert!(!ferrolibri_ui::remote::switcher_visible(&state));

    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = search_screen(&fake);
    assert_shows(&state, &["Standard Ebooks"]);
    // The library is still the source until a plugin is chosen.
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Library
    );
    assert!(!ferrolibri_ui::remote::is_remote(&state));

    // Removing the plugin takes the switch away the next time Search is opened.
    fake.set_rows(Vec::new());
    send(&mut state, Message::Navigate(Screen::Settings), &fake);
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    assert_hides(&state, &["Standard Ebooks"]);
}

#[test]
fn a_plugin_that_is_off_or_not_compatible_is_shown_but_cannot_be_chosen() {
    // Library and two plugins: three sources, so they are buttons side by side.
    let off = PluginRow {
        enabled: false,
        ..plugin("quiet", "Quiet Library")
    };
    let fake = Fake::with_rows(vec![standard_ebooks(), off]);
    let mut state = search_screen(&fake);
    assert_shows(&state, &["Standard Ebooks", "Quiet Library"]);
    // Its button does nothing; the usable one chooses its plugin.
    assert_eq!(clicked_messages(&state, "Quiet Library"), 0);
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("quiet".into())),
        &fake,
    );
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Library
    );
    assert!(fake.saved_sources().is_empty(), "nothing was saved");
    click(&mut state, "Standard Ebooks", &fake);
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Plugin("standard-ebooks".into())
    );

    // The same for a plugin this FerroLibri cannot run, and the hint says why.
    let old = PluginRow {
        health: Health::Incompatible(vec!["Needs plugin API 2".into()]),
        ..plugin("old-site", "Old Site Search")
    };
    let fake = Fake::with_rows(vec![standard_ebooks(), old]);
    let mut state = search_screen(&fake);
    assert_shows(&state, &["Standard Ebooks", "Old Site Search"]);
    assert_eq!(clicked_messages(&state, "Old Site Search"), 0);
    let choices = ferrolibri_ui::remote::choices(&state);
    assert!(!choices[2].usable);
    assert_eq!(choices[2].why, "Not compatible with this FerroLibri");
    assert_eq!(choices[2].to_string(), "Old Site Search (off)");
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("old-site".into())),
        &fake,
    );
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Library
    );
}

#[test]
fn more_than_three_sources_become_a_drop_down() {
    let fake = Fake::with_rows(vec![
        standard_ebooks(),
        plugin("b", "Bee Books"),
        plugin("c", "Sea Shelf"),
    ]);
    let mut state = search_screen(&fake);
    // Library + three plugins: four sources.
    let choices = ferrolibri_ui::remote::choices(&state);
    assert_eq!(choices.len(), 4);
    assert_eq!(choices[1].to_string(), "Standard Ebooks");
    // A drop-down lists the sources only when opened: none of the plugins is a button.
    assert_hides(&state, &["Standard Ebooks", "Bee Books", "Sea Shelf"]);
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("b".into())),
        &fake,
    );
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Plugin("b".into())
    );
    assert_hides(&state, &["Standard Ebooks", "Sea Shelf"]);
}

#[test]
fn the_chosen_source_is_saved_and_a_vanished_one_falls_back_to_the_library() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    assert!(ferrolibri_ui::remote::is_remote(&state));
    assert_eq!(
        fake.saved_sources().last(),
        Some(&Some("standard-ebooks".to_owned()))
    );
    // Back to the library by hand. (The word is also the sidebar's, so the message is sent.)
    remote(&mut state, Msg::SourceChosen(SearchSource::Library), &fake);
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Library
    );
    assert_eq!(fake.saved_sources().last(), Some(&None));

    // Chosen again, then the plugin is removed in Settings.
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("standard-ebooks".into())),
        &fake,
    );
    fake.set_rows(Vec::new());
    send(&mut state, Message::Navigate(Screen::Settings), &fake);
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Library
    );
    assert!(!ferrolibri_ui::remote::is_remote(&state));
    assert_eq!(state.visible_settings().search_source, None);
    assert_eq!(
        fake.saved_sources().last(),
        Some(&None),
        "and it is remembered"
    );

    // The same when the plugin is only turned off.
    let off = PluginRow {
        enabled: false,
        ..standard_ebooks()
    };
    fake.set_rows(vec![standard_ebooks()]);
    send(&mut state, Message::Navigate(Screen::Settings), &fake);
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("standard-ebooks".into())),
        &fake,
    );
    fake.set_rows(vec![off]);
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    assert_eq!(state.visible_settings().search_source, None);
    assert_shows(&state, &["Standard Ebooks"]);
}

#[test]
fn a_saved_source_is_used_as_soon_as_the_plugins_are_known() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = State::default();
    state.settings.search_source = Some("standard-ebooks".into());
    // Before the list is read the library is shown: nothing is known about the plugin.
    assert!(!ferrolibri_ui::remote::is_remote(&state));
    let loaded = state.settings.clone();
    send(&mut state, Message::SettingsLoaded(Ok(loaded)), &fake);
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    assert!(ferrolibri_ui::remote::is_remote(&state));
    // A plugin that is gone from a saved choice is dropped at the first read.
    let mut state = State::default();
    state.settings.search_source = Some("removed".into());
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    assert!(!ferrolibri_ui::remote::is_remote(&state));
    assert_eq!(state.visible_settings().search_source, None);
}

#[test]
fn ctrl_tab_walks_through_the_usable_sources() {
    let off = PluginRow {
        enabled: false,
        ..plugin("quiet", "Quiet Library")
    };
    let fake = Fake::with_rows(vec![standard_ebooks(), off, plugin("c", "Sea Shelf")]);
    let mut state = search_screen(&fake);
    let ctrl = iced::keyboard::Modifiers::CTRL;
    let named = iced::keyboard::key::Named::Tab;
    let current = |state: &State| ferrolibri_ui::remote::current(state);
    send(&mut state, key(named, ctrl), &fake);
    assert_eq!(
        current(&state),
        SearchSource::Plugin("standard-ebooks".into())
    );
    send(&mut state, key(named, ctrl), &fake);
    assert_eq!(
        current(&state),
        SearchSource::Plugin("c".into()),
        "the off one is skipped"
    );
    send(&mut state, key(named, ctrl), &fake);
    assert_eq!(
        current(&state),
        SearchSource::Library,
        "and it wraps around"
    );
    send(
        &mut state,
        key(named, ctrl | iced::keyboard::Modifiers::SHIFT),
        &fake,
    );
    assert_eq!(current(&state), SearchSource::Plugin("c".into()));
}

// ---- searching (G4) --------------------------------------------------------------------------

#[test]
fn a_plugin_is_searched_on_enter_not_on_every_letter() {
    let fake = Fake::with_rows(vec![with_hint(standard_ebooks(), "Search by title only")]);
    let mut state = with_source(&fake, "standard-ebooks");
    assert_shows(
        &state,
        &[
            "Search by title only",
            "Type a query and press Enter to search Standard Ebooks.",
        ],
    );
    fake.answer_searches([page(vec![hit("a", "Pride and Prejudice")], None)]);
    for typed in ["p", "pr", "pride  "] {
        remote(&mut state, Msg::QueryChanged(typed.into()), &fake);
    }
    assert!(fake.searches().is_empty(), "typing asks nobody");
    assert_eq!(state.remote.text("standard-ebooks"), "pride  ");
    remote(&mut state, Msg::Submit, &fake);
    assert_eq!(
        fake.searches(),
        [("pride".to_owned(), 1, None)],
        "trimmed, first page"
    );
    assert_eq!(lock(&fake.script).searches[0].plugin, "standard-ebooks");
    assert_eq!(titles(&state), ["Pride and Prejudice"]);
    assert_shows(
        &state,
        &[
            "Pride and Prejudice",
            "1 results for “pride” · Standard Ebooks",
        ],
    );
    // Nothing is sent for an empty field.
    remote(&mut state, Msg::QueryChanged("   ".into()), &fake);
    remote(&mut state, Msg::Submit, &fake);
    assert_eq!(fake.searches().len(), 1);
}

#[test]
fn each_source_keeps_its_own_text_and_the_library_its_own() {
    let fake = Fake::with_rows(vec![standard_ebooks(), plugin("c", "Sea Shelf")]);
    let mut state = with_source(&fake, "standard-ebooks");
    remote(&mut state, Msg::QueryChanged("austen".into()), &fake);
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("c".into())),
        &fake,
    );
    assert_eq!(state.remote.text("c"), "");
    remote(&mut state, Msg::QueryChanged("verne".into()), &fake);
    remote(
        &mut state,
        Msg::SourceChosen(SearchSource::Plugin("standard-ebooks".into())),
        &fake,
    );
    assert_eq!(state.remote.text("standard-ebooks"), "austen");
    assert_eq!(state.remote.text("c"), "verne");
    assert_eq!(state.search_query, "", "the library's text is its own");
}

#[test]
fn an_older_answer_never_replaces_a_newer_search_and_the_older_call_is_cancelled() {
    // A, then B, then A again: only the last one counts, whatever order the answers come in.
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    remote(&mut state, Msg::QueryChanged("a".into()), &fake);
    let first = launch(&mut state, Message::Remote(Msg::Submit), &fake);
    remote(&mut state, Msg::QueryChanged("b".into()), &fake);
    let second = launch(&mut state, Message::Remote(Msg::Submit), &fake);
    remote(&mut state, Msg::QueryChanged("a".into()), &fake);
    let third = launch(&mut state, Message::Remote(Msg::Submit), &fake);
    assert_eq!(fake.searches().len(), 3);
    assert!(fake.search_was_cancelled(0), "A is cancelled by B");
    assert!(
        fake.search_was_cancelled(1),
        "B is cancelled by the second A"
    );
    assert!(!fake.search_was_cancelled(2));
    assert!(matches!(session(&state).phase, Phase::Loading));

    fake.release_search(0, page(vec![hit("old-a", "Old A")], None));
    fake.release_search(1, page(vec![hit("old-b", "Old B")], None));
    fake.release_search(2, page(vec![hit("new-a", "New A")], None));
    // The oldest answer comes first: it must be dropped, and the screen keeps waiting.
    deliver(&mut state, first, &fake);
    assert!(matches!(session(&state).phase, Phase::Loading));
    assert!(titles(&state).is_empty());
    deliver(&mut state, third, &fake);
    assert_eq!(titles(&state), ["New A"]);
    // And an answer that is late after the new one has arrived is ignored too.
    deliver(&mut state, second, &fake);
    assert_eq!(titles(&state), ["New A"]);
    assert!(matches!(session(&state).phase, Phase::Results));
}

#[test]
fn changing_the_source_cancels_the_call_that_is_running() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    remote(&mut state, Msg::QueryChanged("a".into()), &fake);
    let running = launch(&mut state, Message::Remote(Msg::Submit), &fake);
    remote(&mut state, Msg::SourceChosen(SearchSource::Library), &fake);
    assert!(fake.search_was_cancelled(0));
    assert!(state.remote.session.is_none());
    // Its answer, when it comes, finds nothing to change.
    fake.release_search(0, page(vec![hit("a", "Late")], None));
    deliver(&mut state, running, &fake);
    assert!(state.remote.session.is_none());
    assert_eq!(
        ferrolibri_ui::remote::current(&state),
        SearchSource::Library
    );
}

#[test]
fn cancel_ends_the_wait_and_says_so() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    remote(&mut state, Msg::QueryChanged("slow".into()), &fake);
    let running = launch(&mut state, Message::Remote(Msg::Submit), &fake);
    assert_shows(&state, &["Searching Standard Ebooks for “slow”…", "Cancel"]);
    click(&mut state, "Cancel", &fake);
    assert!(fake.search_was_cancelled(0));
    assert_shows(&state, &["Search cancelled."]);
    assert_hides(&state, &["Searching Standard Ebooks for “slow”…"]);
    fake.release_search(0, failure(SourceErrorKind::Cancelled, ""));
    deliver(&mut state, running, &fake);
    assert_shows(&state, &["Search cancelled."]);
}

// ---- failures and empty answers (G4) ---------------------------------------------------------

#[test]
fn a_failed_search_says_what_the_plugin_said_and_can_be_tried_again() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([
        failure(
            SourceErrorKind::Plugin,
            "plugin error: the plugin returned an unexpected response (hits is missing)",
        ),
        page(vec![hit("a", "Pride and Prejudice")], None),
    ]);
    type_and_search(&mut state, "pride", &fake);
    assert_shows(
        &state,
        &[
            "Standard Ebooks: plugin error: the plugin returned an unexpected response (hits is missing)",
            "Try again",
        ],
    );
    assert_hides(&state, &["Open plugin settings"]);
    click(&mut state, "Try again", &fake);
    assert_eq!(
        fake.searches(),
        [("pride".to_owned(), 1, None), ("pride".to_owned(), 1, None)],
        "the same text again"
    );
    assert_eq!(titles(&state), ["Pride and Prejudice"]);
}

#[test]
fn a_connection_failure_names_the_host_and_leads_to_the_plugin_settings() {
    let row = with_address(standard_ebooks(), Some("https://mirror.example.org/opds"));
    let fake = Fake::with_rows(vec![row]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([failure(
        SourceErrorKind::Unreachable,
        "network error (timeout): the site did not answer",
    )]);
    type_and_search(&mut state, "pride", &fake);
    assert_shows(
        &state,
        &[
            "Standard Ebooks: Could not connect to mirror.example.org. Check the address in the plugin settings.",
            "Try again",
            "Open plugin settings",
        ],
    );
    click(&mut state, "Open plugin settings", &fake);
    assert_eq!(state.screen, Screen::Settings);
    assert_eq!(
        state.plugins.expanded.as_deref(),
        Some("standard-ebooks"),
        "the plugin's settings are open"
    );
    assert_shows(&state, &["Catalog address"]);
}

#[test]
fn a_plugin_without_an_address_names_its_first_site() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([failure(
        SourceErrorKind::Unreachable,
        "network error (dns): x",
    )]);
    type_and_search(&mut state, "pride", &fake);
    assert_shows(
        &state,
        &[
            "Standard Ebooks: Could not connect to standardebooks.org. Check the address in the plugin settings.",
        ],
    );
}

#[test]
fn a_server_error_is_called_one_and_offers_to_try_again() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([failure(
        SourceErrorKind::Server,
        "The catalog answered with status 503",
    )]);
    type_and_search(&mut state, "pride", &fake);
    assert_shows(
        &state,
        &[
            "Standard Ebooks: The server answered with an error. Try again later.",
            "Try again",
        ],
    );
    assert_hides(&state, &["Open plugin settings", "status 503"]);
}

#[test]
fn nothing_found_says_so() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(Vec::new(), None)]);
    type_and_search(&mut state, "qwertyuiop", &fake);
    assert_shows(
        &state,
        &[
            "No results for “qwertyuiop” in Standard Ebooks.",
            "Check the spelling or try fewer words.",
        ],
    );
    assert!(matches!(session(&state).phase, Phase::Empty));
}

#[test]
fn a_plugin_that_needs_an_address_sends_the_user_to_its_settings() {
    let fake = Fake::with_rows(vec![with_address(standard_ebooks(), None)]);
    let mut state = with_source(&fake, "standard-ebooks");
    remote(&mut state, Msg::QueryChanged("pride".into()), &fake);
    remote(&mut state, Msg::Submit, &fake);
    assert!(
        fake.searches().is_empty(),
        "the plugin is not called without an address"
    );
    assert_shows(
        &state,
        &[
            "Standard Ebooks needs an address first",
            "Enter the catalog address in Settings → Plugins before searching.",
            "Open plugin settings",
        ],
    );
    click(&mut state, "Open plugin settings", &fake);
    assert_eq!(state.screen, Screen::Settings);
    assert_eq!(state.plugins.expanded.as_deref(), Some("standard-ebooks"));
    // Once the address is saved, the next visit searches.
    fake.set_rows(vec![with_address(
        standard_ebooks(),
        Some("https://nas.local/opds"),
    )]);
    send(&mut state, Message::Navigate(Screen::Search), &fake);
    assert_hides(&state, &["Standard Ebooks needs an address first"]);
    assert_shows(
        &state,
        &["Type a query and press Enter to search Standard Ebooks."],
    );
}

// ---- results, more of them, details (G4, G5) -----------------------------------------------

#[test]
fn results_show_titles_authors_and_the_formats_on_offer() {
    let mut mars = hit("h4", "A Princess of Mars");
    mars.authors = vec!["Edgar Rice Burroughs".into()];
    mars.offers = vec![offer("o1", "EPUB", None), offer("o2", "AZW3", None)];
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(vec![hit("a", "Pride and Prejudice"), mars], None)]);
    type_and_search(&mut state, "p", &fake);
    assert_shows(
        &state,
        &[
            "Pride and Prejudice",
            "Jane Austen",
            "A Princess of Mars",
            "Edgar Rice Burroughs",
            "EPUB",
            "2 results for “p” · Standard Ebooks",
            "End of results",
        ],
    );
    assert_hides(&state, &["Show more", "so far"]);
}

#[test]
fn show_more_passes_the_cursor_back_and_ends_with_the_end_of_results() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([
        page(
            vec![hit("a", "First"), hit("b", "Second")],
            Some("cursor-2"),
        ),
        // The next page repeats an id of the first one: it is not listed twice.
        page(
            vec![hit("b", "Second"), hit("c", "Third")],
            Some("cursor-3"),
        ),
        page(vec![hit("d", "Fourth")], None),
    ]);
    type_and_search(&mut state, "p", &fake);
    assert_shows(
        &state,
        &["2 results so far for “p” · Standard Ebooks", "Show more"],
    );
    click(&mut state, "Show more", &fake);
    click(&mut state, "Show more", &fake);
    assert_eq!(
        fake.searches(),
        [
            ("p".to_owned(), 1, None),
            ("p".to_owned(), 2, Some("cursor-2".to_owned())),
            ("p".to_owned(), 3, Some("cursor-3".to_owned())),
        ]
    );
    assert_eq!(titles(&state), ["First", "Second", "Third", "Fourth"]);
    assert_shows(
        &state,
        &["4 results for “p” · Standard Ebooks", "End of results"],
    );
    assert_hides(&state, &["Show more"]);
}

#[test]
fn a_failed_show_more_keeps_the_results_and_can_be_tried_again() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([
        page(vec![hit("a", "First")], Some("cursor-2")),
        failure(
            SourceErrorKind::Server,
            "The catalog answered with status 502",
        ),
        page(vec![hit("b", "Second")], None),
    ]);
    type_and_search(&mut state, "p", &fake);
    click(&mut state, "Show more", &fake);
    assert!(matches!(session(&state).more, More::Failed(_)));
    assert_shows(
        &state,
        &[
            "Could not load more results: Standard Ebooks: The server answered with an error. Try again later.",
            "First",
        ],
    );
    click(&mut state, "Try again", &fake);
    assert_eq!(titles(&state), ["First", "Second"]);
    assert_eq!(
        fake.searches()[2].2.as_deref(),
        Some("cursor-2"),
        "the same cursor"
    );
}

#[test]
fn a_new_search_while_more_is_loading_drops_the_late_page() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(vec![hit("a", "First")], Some("cursor-2"))]);
    type_and_search(&mut state, "p", &fake);
    let more = launch(&mut state, Message::Remote(Msg::ShowMore), &fake);
    assert!(matches!(session(&state).more, More::Loading));
    fake.answer_searches([page(vec![hit("z", "Other")], None)]);
    type_and_search(&mut state, "q", &fake);
    assert!(
        fake.search_was_cancelled(1),
        "the page being loaded is cancelled"
    );
    fake.release_search(1, page(vec![hit("late", "Late")], None));
    deliver(&mut state, more, &fake);
    assert_eq!(titles(&state), ["Other"]);
}

#[test]
fn selecting_a_result_asks_for_its_details_once_and_shows_what_is_known_meanwhile() {
    let mut first = hit("a", "Pride and Prejudice");
    first.year = Some(1813);
    first.publisher = Some("Standard Ebooks".into());
    first.tags = vec!["Fiction".into()];
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(vec![first.clone(), hit("b", "Emma")], None)]);
    type_and_search(&mut state, "p", &fake);
    assert_shows(
        &state,
        &["Select a result to see its details and download options."],
    );

    let waiting = launch(&mut state, Message::Remote(Msg::Select("a".into())), &fake);
    assert_eq!(fake.detail_calls(), ["a"]);
    assert_eq!(lock(&fake.script).details[0].plugin, "standard-ebooks");
    assert_shows(
        &state,
        &[
            "From Standard Ebooks",
            "Loading details…",
            "Jane Austen",
            "1813",
            "Fiction",
            "DOWNLOADS",
            "EPUB",
            "812 KB",
        ],
    );
    assert_hides(&state, &["A witty novel of manners"]);
    let mut detailed = first.clone();
    detailed.description = Some("A witty novel of manners.".into());
    fake.release_details(0, Ok(detailed));
    deliver(&mut state, waiting, &fake);
    assert_shows(&state, &["A witty novel of manners."]);
    assert_hides(&state, &["Loading details…"]);
    assert!(matches!(state.remote.details, Some(DetailsPhase::Ready)));

    // Another result, then back: the details of the first are remembered.
    fake.answer_details([Ok(hit("b", "Emma"))]);
    remote(&mut state, Msg::Select("b".into()), &fake);
    remote(&mut state, Msg::Select("a".into()), &fake);
    assert_eq!(
        fake.detail_calls(),
        ["a", "b"],
        "no second call for the first one"
    );
    assert_shows(&state, &["A witty novel of manners."]);
    // The list itself still shows what the search returned.
    assert_eq!(session(&state).hits[0].description, None);
}

#[test]
fn a_late_details_answer_after_the_selection_moved_is_dropped() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(vec![hit("a", "First"), hit("b", "Second")], None)]);
    type_and_search(&mut state, "p", &fake);
    let first = launch(&mut state, Message::Remote(Msg::Select("a".into())), &fake);
    let second = launch(&mut state, Message::Remote(Msg::Select("b".into())), &fake);
    assert!(lock(&fake.script).details[0].token.is_cancelled());
    let mut for_a = hit("a", "First");
    for_a.description = Some("Belongs to the first.".into());
    let mut for_b = hit("b", "Second");
    for_b.description = Some("Belongs to the second.".into());
    fake.release_details(0, Ok(for_a));
    fake.release_details(1, Ok(for_b));
    deliver(&mut state, first, &fake);
    assert_hides(&state, &["Belongs to the first."]);
    assert_shows(&state, &["Loading details…"]);
    deliver(&mut state, second, &fake);
    assert_shows(&state, &["Belongs to the second."]);
    assert_hides(&state, &["Belongs to the first.", "Loading details…"]);
    assert_eq!(state.remote.selected.as_deref(), Some("b"));
}

#[test]
fn failed_details_keep_the_known_fields_and_can_be_tried_again() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(vec![hit("a", "First")], None)]);
    type_and_search(&mut state, "p", &fake);
    fake.answer_details([
        Err(SourceError::new(
            SourceErrorKind::Unreachable,
            "network error (timeout): slow",
        )),
        Ok(RemoteHit {
            description: Some("Now it worked.".into()),
            ..hit("a", "First")
        }),
    ]);
    remote(&mut state, Msg::Select("a".into()), &fake);
    assert_shows(
        &state,
        &[
            "Could not load more details",
            "Jane Austen",
            "Showing only what the search returned.",
            "Try again",
        ],
    );
    click(&mut state, "Try again", &fake);
    assert_shows(&state, &["Now it worked."]);
    assert_hides(&state, &["Could not load more details"]);
    assert_eq!(fake.detail_calls(), ["a", "a"]);
}

#[test]
fn the_pane_lists_every_offer_and_says_when_a_result_has_none() {
    let mut book = hit("a", "Crime and Punishment");
    book.offers = vec![
        RemoteOffer {
            zip: true,
            label: Some("fb2.zip".into()),
            ..offer("o1", "FB2", Some(700_000))
        },
        offer("o2", "EPUB", None),
    ];
    book.series = Some("Barsoom".into());
    book.series_index = Some(1.0);
    book.page_url = Some("https://standardebooks.org/ebooks/a".into());
    let pamphlet = RemoteHit {
        offers: Vec::new(),
        ..hit("b", "Pamphlet")
    };
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_details([Ok(book.clone()), Ok(pamphlet.clone())]);
    fake.answer_searches([page(vec![book, pamphlet], None)]);
    type_and_search(&mut state, "p", &fake);
    remote(&mut state, Msg::Select("a".into()), &fake);
    assert_shows(
        &state,
        &[
            "FB2 zip",
            "fb2.zip · 700 KB",
            "EPUB",
            "size unknown",
            "Barsoom · #1",
            "Open page",
        ],
    );
    click(&mut state, "Open page", &fake);
    assert_eq!(
        lock(&fake.script).opened,
        ["https://standardebooks.org/ebooks/a"]
    );
    remote(&mut state, Msg::Select("b".into()), &fake);
    assert_shows(
        &state,
        &["This source offers no downloadable file for this book."],
    );
    assert_hides(&state, &["Open page"]);
}

#[test]
fn selecting_a_result_opens_the_details_pane_and_the_toggle_hides_it() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    assert!(ferrolibri_ui::widgets::shell::Layout::for_state(&state).details_available);
    fake.answer_searches([page(vec![hit("a", "First")], None)]);
    fake.answer_details([Ok(hit("a", "First"))]);
    type_and_search(&mut state, "p", &fake);
    state.settings.details_visible = false;
    remote(&mut state, Msg::Select("a".into()), &fake);
    assert!(
        state.visible_settings().details_visible,
        "selecting shows the pane"
    );
    send(&mut state, Message::ToggleDetails, &fake);
    assert!(!state.visible_settings().details_visible);
    // The library search has no such pane.
    remote(&mut state, Msg::SourceChosen(SearchSource::Library), &fake);
    assert!(!ferrolibri_ui::widgets::shell::Layout::for_state(&state).details_available);
}

#[test]
fn the_list_and_the_grid_show_the_same_results() {
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(vec![hit("a", "First"), hit("b", "Second")], None)]);
    type_and_search(&mut state, "p", &fake);
    assert!(!state.remote.grid);
    remote(&mut state, Msg::ShowGrid, &fake);
    assert!(state.remote.grid);
    assert_shows(&state, &["First", "Second", "End of results"]);
    let mut ui = simulator(&state);
    ui.click("Second").unwrap();
    let messages: Vec<Message> = ui.into_messages().collect();
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Remote(Msg::Select(id)) if id == "b")),
        "a card selects its result"
    );
    remote(&mut state, Msg::ShowList, &fake);
    assert!(!state.remote.grid);
}

// ---- covers -------------------------------------------------------------------------------------

#[test]
fn covers_are_fetched_for_the_rows_on_screen_and_a_failure_is_a_placeholder() {
    let mut covered: Vec<RemoteHit> = (0..6)
        .map(|i| RemoteHit {
            cover_url: Some(format!("https://standardebooks.org/c/{i}.jpg")),
            ..hit(&format!("h{i}"), &format!("Book {i}"))
        })
        .collect();
    covered[1].cover_url = Some("https://standardebooks.org/c/missing.jpg".into());
    covered.push(hit("plain", "No cover at all"));
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    lock(&fake.script)
        .bad_covers
        .insert("https://standardebooks.org/c/missing.jpg".into());
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(covered, None)]);
    type_and_search(&mut state, "p", &fake);
    let asked = fake.covers();
    assert_eq!(
        asked.len(),
        6,
        "every cover on screen once, none for a result without: {asked:?}"
    );
    assert!(
        state
            .remote
            .covers
            .get("https://standardebooks.org/c/0.jpg")
            .is_some()
    );
    assert!(
        state
            .remote
            .covers
            .get("https://standardebooks.org/c/missing.jpg")
            .is_none(),
        "the placeholder stays"
    );
    // Scrolling or redrawing does not ask again, not even for the one that failed.
    send(
        &mut state,
        Message::ListScrolled {
            offset: 0.0,
            height: 500.0,
        },
        &fake,
    );
    assert_eq!(fake.covers().len(), 6);
    assert_shows(&state, &["Book 1", "No cover at all"]);
}

#[test]
fn covers_are_asked_only_for_what_is_near_the_viewport_and_never_more_than_two_at_once() {
    let many: Vec<RemoteHit> = (0..100)
        .map(|i| RemoteHit {
            cover_url: Some(format!("https://standardebooks.org/c/{i}.jpg")),
            ..hit(&format!("h{i}"), &format!("Book {i}"))
        })
        .collect();
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(many, None)]);
    type_and_search(&mut state, "p", &fake);
    let asked = fake.covers().len();
    assert!(
        (1..100).contains(&asked) && asked <= 40,
        "only the first screens: {asked}"
    );
    // Far down the list, others are asked for.
    let row = state.row_height();
    send(
        &mut state,
        Message::ListScrolled {
            offset: row * 80.0,
            height: 500.0,
        },
        &fake,
    );
    assert!(fake.covers().iter().any(|url| url.ends_with("/c/85.jpg")));
    assert_eq!(ferrolibri_ui::remote::COVER_CONCURRENCY, 2);
    assert!(state.remote.covers.running() <= 2);
}

// ---- keyboard ---------------------------------------------------------------------------------

#[test]
fn arrow_keys_move_through_the_results_and_load_the_details_of_each() {
    use iced::keyboard::{Modifiers, key::Named};
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    fake.answer_searches([page(
        vec![hit("a", "First"), hit("b", "Second"), hit("c", "Third")],
        None,
    )]);
    fake.answer_details([
        Ok(hit("a", "First")),
        Ok(hit("b", "Second")),
        Ok(hit("c", "Third")),
    ]);
    type_and_search(&mut state, "p", &fake);
    let none = Modifiers::empty();
    send(&mut state, key(Named::ArrowDown, none), &fake);
    assert_eq!(
        state.remote.selected.as_deref(),
        Some("a"),
        "the first press selects the first"
    );
    send(&mut state, key(Named::ArrowDown, none), &fake);
    send(&mut state, key(Named::ArrowDown, none), &fake);
    send(&mut state, key(Named::ArrowDown, none), &fake);
    assert_eq!(
        state.remote.selected.as_deref(),
        Some("c"),
        "and it stops at the end"
    );
    send(&mut state, key(Named::ArrowUp, none), &fake);
    assert_eq!(state.remote.selected.as_deref(), Some("b"));
    send(&mut state, key(Named::Home, none), &fake);
    assert_eq!(state.remote.selected.as_deref(), Some("a"));
    send(&mut state, key(Named::End, none), &fake);
    assert_eq!(state.remote.selected.as_deref(), Some("c"));
    // Going back over a result whose details are known asks nobody again.
    assert_eq!(fake.detail_calls(), ["a", "b", "c"]);
    // Keys of the library mean nothing here.
    let before = state.remote.selected.clone();
    send(
        &mut state,
        Message::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Character("a".into()),
            modified_key: iced::keyboard::Key::Character("a".into()),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::KeyA),
            location: iced::keyboard::Location::Standard,
            modifiers: Modifiers::CTRL,
            text: None,
            repeat: false,
        }),
        &fake,
    );
    assert_eq!(state.remote.selected, before);
    assert!(state.selection.selected.is_empty());
}

#[test]
fn escape_leaves_search_from_a_plugin_too() {
    use iced::keyboard::{Modifiers, key::Named};
    let fake = Fake::with_rows(vec![standard_ebooks()]);
    let mut state = with_source(&fake, "standard-ebooks");
    send(&mut state, key(Named::Escape, Modifiers::empty()), &fake);
    assert_ne!(state.screen, Screen::Search);
}

// ---- a real plugin ---------------------------------------------------------------------------

mod real {
    use super::*;
    use ferrolibri_plugins::{
        Downgrade, PluginStore, SettingValue,
        testing::{TestServer, opds},
    };

    /// The sample plugin of the plugin crate, installed in a temporary store, pointed at a local
    /// stand-in of the OPDS catalog the plugin was written for. No real site is involved.
    struct Setup {
        _root: tempfile::TempDir,
        services: RealServices,
        server: TestServer,
    }

    fn setup() -> Setup {
        let root = tempfile::tempdir().unwrap();
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
                &[(
                    "base_url".to_owned(),
                    SettingValue::Text(server.ip_url("/")),
                )]
                .into_iter()
                .collect(),
            )
            .unwrap();
        Setup {
            _root: root,
            services,
            server,
        }
    }

    #[test]
    fn the_sample_plugin_is_searched_through_the_real_runtime_page_by_page() {
        let setup = setup();
        let services = &setup.services;
        let mut state = search_screen_with(services);
        assert!(
            ferrolibri_ui::remote::switcher_visible(&state),
            "the installed plugin is a source"
        );
        assert_eq!(
            ferrolibri_ui::remote::choices(&state)[1].name,
            "Sample OPDS catalog"
        );
        remote(
            &mut state,
            Msg::SourceChosen(SearchSource::Plugin("sample-opds".into())),
            services,
        );
        assert!(ferrolibri_ui::remote::is_remote(&state));
        assert_shows(&state, &["Search by title only"]);

        type_and_search(&mut state, "и", services);
        assert_eq!(session(&state).hits.len(), 2, "{:?}", session(&state).phase);
        assert!(session(&state).next.is_some());
        assert_shows(
            &state,
            &[
                "2 results so far for “и” · Sample OPDS catalog",
                "Show more",
            ],
        );
        // The offer is a zip around FB2, as the plugin says.
        assert_shows(&state, &["FB2"]);
        remote(&mut state, Msg::ShowMore, services);
        remote(&mut state, Msg::ShowMore, services);
        assert_eq!(session(&state).hits.len(), 5);
        assert!(session(&state).next.is_none());
        assert_shows(
            &state,
            &["5 results for “и” · Sample OPDS catalog", "End of results"],
        );

        // The details come from the feed through the same sandbox.
        let id = session(&state).hits[0].id.clone();
        remote(&mut state, Msg::Select(id), services);
        assert!(matches!(state.remote.details, Some(DetailsPhase::Ready)));
        assert_shows(&state, &["From the feed “Books”."]);
        let selected = state.remote.selected_hit().unwrap();
        assert_eq!(selected.offers.len(), 1);
        assert!(selected.offers[0].zip);
        assert!(!setup.server.requests().is_empty());
    }

    #[test]
    fn the_real_service_explains_a_catalog_that_does_not_answer() {
        let setup = setup();
        let services = &setup.services;
        // The address is changed to one nobody listens on.
        let store = PluginStore::new(
            setup._root.path().join("plugins"),
            setup._root.path().join("plugins.toml"),
        );
        store
            .save_settings(
                "sample-opds",
                &[(
                    "base_url".to_owned(),
                    SettingValue::Text("http://127.0.0.1:9/".into()),
                )]
                .into_iter()
                .collect(),
            )
            .unwrap();
        let mut state = search_screen_with(services);
        remote(
            &mut state,
            Msg::SourceChosen(SearchSource::Plugin("sample-opds".into())),
            services,
        );
        type_and_search(&mut state, "и", services);
        let Phase::Failed(error) = &session(&state).phase else {
            panic!("{:?}", session(&state).phase)
        };
        assert_eq!(error.kind, SourceErrorKind::Plugin, "{error:?}");
        assert!(
            error
                .message
                .contains("Check its address in the plugin settings")
        );
        assert_shows(&state, &["Try again"]);
    }

    #[test]
    fn a_plugin_that_is_turned_off_cannot_be_called_any_more() {
        let setup = setup();
        let services = &setup.services;
        let mut state = search_screen_with(services);
        remote(
            &mut state,
            Msg::SourceChosen(SearchSource::Plugin("sample-opds".into())),
            services,
        );
        let store = PluginStore::new(
            setup._root.path().join("plugins"),
            setup._root.path().join("plugins.toml"),
        );
        store.disable("sample-opds").unwrap();
        // A search that was already on its way finds the plugin off, and says so.
        type_and_search(&mut state, "и", services);
        let Phase::Failed(error) = &session(&state).phase else {
            panic!("{:?}", session(&state).phase)
        };
        assert_eq!(error.kind, SourceErrorKind::Unavailable, "{error:?}");
        assert!(
            error.message.to_lowercase().contains("disabled"),
            "{error:?}"
        );
    }

    fn search_screen_with(services: &dyn Services) -> State {
        let mut state = State::default();
        state.window_width = 1100.0;
        state.viewport_width = 700.0;
        state.list_height = 500.0;
        send(&mut state, Message::Navigate(Screen::Search), services);
        state
    }
}
