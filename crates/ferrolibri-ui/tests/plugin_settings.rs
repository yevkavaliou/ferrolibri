//! Settings → Plugins and the install dialog (plan item 11): the real `view` and `update`
//! driven through the headless simulator, with a scripted `Services` standing in for the
//! plugin store and for GitHub. A second group of tests runs the real folder installation in
//! a temporary directory.
use ferrolibri_core::{converter::CancellationToken, cover::CoverFile, jobs::JobRequest};
use ferrolibri_ui::{
    app::{self, Message, Screen, State},
    plugins::{Mode, Msg},
    services::{
        RealServices, ServiceFuture, Services,
        jobs::{JobId, JobUpdate},
        plugins::{
            Candidate, Change, Checked, FieldKind, FieldValue, Health, InstallOptions, Installed,
            PermissionSummary, PluginFuture, PluginInfo, PluginRow, Problem, ProblemKind,
            ReleaseView, Releases, SettingField, SourceRequest,
        },
    },
    settings::UiSettings,
};
use iced::futures::StreamExt;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Mutex, MutexGuard, PoisonError},
};

fn headless() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { std::env::set_var("ICED_TEST_BACKEND", "tiny-skia") });
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the scripted services -----------------------------------------------------------------

#[derive(Default)]
struct Script {
    rows: Vec<PluginRow>,
    list_error: Option<String>,
    checks: VecDeque<Result<Checked, Problem>>,
    inspections: VecDeque<Result<Candidate, Problem>>,
    installs: VecDeque<Result<Installed, Problem>>,
    toggle_error: Option<String>,
    picked: VecDeque<PathBuf>,
    calls: Vec<String>,
    saved: Vec<(String, Vec<(String, FieldValue)>)>,
    installed_with: Vec<InstallOptions>,
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
    fn calls(&self) -> Vec<String> {
        lock(&self.script).calls.clone()
    }
    fn count(&self, call: &str) -> usize {
        self.calls().iter().filter(|c| c.as_str() == call).count()
    }
}

impl Services for Fake {
    fn enqueue(&self, _: String, _: JobRequest) -> JobId {
        unreachable!("plugin screens enqueue no jobs")
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
    fn save_settings(&self, _: UiSettings) -> ServiceFuture<()> {
        Box::pin(async { Ok(()) })
    }
    fn find_cover(&self, _: PathBuf) -> ServiceFuture<Option<CoverFile>> {
        Box::pin(async { Ok(None) })
    }
    fn pick_folder(&self) -> ServiceFuture<Option<PathBuf>> {
        let picked = lock(&self.script).picked.pop_front();
        Box::pin(async move { Ok(picked) })
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
        let mut script = lock(&self.script);
        script.calls.push("list".into());
        let result = match &script.list_error {
            Some(error) => Err(error.clone()),
            None => Ok(script.rows.clone()),
        };
        Box::pin(async move { result })
    }
    fn check_plugin_source(
        &self,
        request: SourceRequest,
        _cancel: CancellationToken,
    ) -> PluginFuture<Checked> {
        let mut script = lock(&self.script);
        script.calls.push(format!("check {request:?}"));
        let result = script.checks.pop_front().expect("a scripted check");
        Box::pin(async move { result })
    }
    fn inspect_plugin_release(
        &self,
        release: ReleaseView,
        _cancel: CancellationToken,
    ) -> PluginFuture<Candidate> {
        let mut script = lock(&self.script);
        script.calls.push(format!("release {}", release.tag));
        let result = script
            .inspections
            .pop_front()
            .expect("a scripted inspection");
        Box::pin(async move { result })
    }
    fn install_plugin(
        &self,
        candidate: Candidate,
        options: InstallOptions,
    ) -> PluginFuture<Installed> {
        let mut script = lock(&self.script);
        script.calls.push(format!("install {}", candidate.id));
        script.installed_with.push(options);
        let result = script.installs.pop_front().unwrap_or_else(|| {
            Ok(Installed {
                name: candidate.name.clone(),
                version: candidate.version.clone(),
                change: candidate.change.clone(),
                turned_on: candidate.change == Change::New && options.turn_on,
            })
        });
        Box::pin(async move { result })
    }
    fn set_plugin_enabled(&self, id: String, enabled: bool) -> ServiceFuture<()> {
        let mut script = lock(&self.script);
        script.calls.push(format!("enable {id} {enabled}"));
        let result = match script.toggle_error.clone() {
            Some(error) => Err(error),
            None => {
                if let Some(row) = script.rows.iter_mut().find(|r| r.id == id) {
                    row.enabled = enabled;
                }
                Ok(())
            }
        };
        Box::pin(async move { result })
    }
    fn remove_plugin(&self, id: String) -> ServiceFuture<()> {
        let mut script = lock(&self.script);
        script.calls.push(format!("remove {id}"));
        script.rows.retain(|r| r.id != id);
        Box::pin(async { Ok(()) })
    }
    fn save_plugin_settings(
        &self,
        id: String,
        values: Vec<(String, FieldValue)>,
    ) -> ServiceFuture<()> {
        let mut script = lock(&self.script);
        script.calls.push(format!("save {id}"));
        if let Some(row) = script.rows.iter_mut().find(|r| r.id == id)
            && let Some(info) = row.info.as_mut()
        {
            for (field, (_, value)) in info.settings.iter_mut().zip(&values) {
                field.value = value.clone();
                field.saved = true;
            }
        }
        script.saved.push((id, values));
        Box::pin(async { Ok(()) })
    }
}

// ---- builders --------------------------------------------------------------------------------

fn permissions() -> PermissionSummary {
    PermissionSummary {
        domains: vec!["standardebooks.org".into(), "*.standardebooks.org".into()],
        user_address: false,
        call_timeout_s: 30,
        content: vec!["metadata".into(), "public-domain".into()],
    }
}

fn plugin(id: &str, name: &str, version: &str) -> PluginRow {
    PluginRow {
        id: id.into(),
        name: name.into(),
        version: version.into(),
        author: "Jane Doe".into(),
        description: "Free ebooks".into(),
        enabled: false,
        health: Health::Ready,
        info: Some(PluginInfo {
            permissions: permissions(),
            settings: Vec::new(),
            source: "GitHub · janedoe/ferrolibri-standard-ebooks · v1.0.0".into(),
            installed: "17 Oct 2026".into(),
            search_hint: None,
        }),
    }
}

fn address_field(saved: bool) -> SettingField {
    SettingField {
        key: "base_url".into(),
        label: "Catalog address".into(),
        kind: FieldKind::Url,
        network: true,
        value: FieldValue::Text(if saved {
            "https://nas.local".into()
        } else {
            String::new()
        }),
        placeholder: Some("https://example.org".into()),
        saved,
    }
}

fn with_settings(mut row: PluginRow, settings: Vec<SettingField>) -> PluginRow {
    row.info.as_mut().unwrap().settings = settings;
    row
}

fn candidate(name: &str, version: &str, change: Change) -> Candidate {
    Candidate {
        id: "standard-ebooks".into(),
        name: name.into(),
        version: version.into(),
        author: "Jane Doe".into(),
        license: "MIT".into(),
        homepage: "https://example.org/standard-ebooks".into(),
        source: "Folder · D:\\Plugins\\standard-ebooks".into(),
        api: "1 (supported)".into(),
        checksum_in_manifest: false,
        compatible: true,
        reasons: Vec::new(),
        permissions: permissions(),
        change,
        handle: None,
    }
}

fn incompatible() -> Candidate {
    Candidate {
        compatible: false,
        reasons: vec![
            "Plugin API version 2 is not supported (supported: 1)".into(),
            "Requires FerroLibri 0.6.0 or newer (this is 0.1.2)".into(),
        ],
        ..candidate("Standard Ebooks", "2.0.0", Change::New)
    }
}

fn releases() -> Releases {
    let view = |tag: &str, date: &str, ok: bool, reason: &str| ReleaseView {
        tag: tag.into(),
        date: date.into(),
        compatible: ok,
        reason: reason.into(),
        handle: None,
    };
    Releases {
        repo: "janedoe/ferrolibri-standard-ebooks".into(),
        items: vec![
            view(
                "v2.0.0",
                "14 Oct 2026",
                false,
                "Plugin API version 2 is not supported",
            ),
            view("v1.2.0", "12 Oct 2026", true, ""),
            view("v1.1.0", "2 Sep 2026", true, ""),
        ],
    }
}

fn problem(title: &str, message: &str, kind: ProblemKind) -> Problem {
    Problem {
        kind,
        title: title.into(),
        message: message.into(),
    }
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

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// Sends a message and everything it sets in motion.
fn send(state: &mut State, message: impl Into<Message>, services: &dyn Services) {
    block_on(async {
        let task = app::update(state, message.into(), services);
        run(state, task, services).await;
    });
}

fn plugins(state: &mut State, msg: Msg, services: &dyn Services) {
    send(state, Message::Plugins(msg), services);
}

fn simulator<'a>(state: &'a State) -> iced_test::Simulator<'a, Message> {
    headless();
    iced_test::Simulator::with_size(
        iced::Settings::default(),
        iced::Size::new(1100.0, 1800.0),
        app::view(state),
    )
}

/// Clicks the widget with this text in the real view, then handles what it sent.
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

fn shows(state: &State, text: &str) -> bool {
    simulator(state).find(text).is_ok()
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

fn settings_screen(fake: &Fake) -> State {
    let mut state = State::default();
    state.window_width = 1100.0;
    send(&mut state, Message::Navigate(Screen::Settings), fake);
    send(
        &mut state,
        Message::SetSettingsTab(ferrolibri_ui::app::SettingsTab::Plugins),
        fake,
    );
    state
}

fn toast_texts(state: &State) -> Vec<String> {
    state.toasts.iter().map(|t| t.message.clone()).collect()
}

// ---- Settings → Plugins ------------------------------------------------------------------------

#[test]
fn opening_settings_lists_plugins_or_says_there_are_none() {
    let none = Fake::default();
    let state = settings_screen(&none);
    assert_shows(
        &state,
        &[
            "PLUGINS",
            "Plugins add search sources to the Search screen.",
            "Install plugin…",
            "No plugins installed.",
        ],
    );
    let fake = Fake::with_rows(vec![
        plugin("standard-ebooks", "Standard Ebooks", "1.2.0"),
        PluginRow {
            enabled: true,
            ..plugin("oapen", "OAPEN", "0.3.0")
        },
    ]);
    let state = settings_screen(&fake);
    assert_shows(
        &state,
        &[
            "Standard Ebooks",
            "v1.2.0",
            "OAPEN",
            "Off",
            "On",
            "Free ebooks · by Jane Doe",
        ],
    );
    assert_hides(&state, &["No plugins installed.", "THIS PLUGIN CAN"]);
    // Every visit to the screen reads the list again.
    assert_eq!(fake.count("list"), 1);
    let mut state = state;
    send(&mut state, Message::Navigate(Screen::Jobs), &fake);
    send(&mut state, Message::Navigate(Screen::Settings), &fake);
    send(
        &mut state,
        Message::SetSettingsTab(ferrolibri_ui::app::SettingsTab::Plugins),
        &fake,
    );
    assert_eq!(fake.count("list"), 2);
}

#[test]
fn an_unreadable_list_is_explained_and_does_not_hide_the_install_button() {
    let fake = Fake::default();
    lock(&fake.script).list_error = Some("plugins.toml is damaged".into());
    let state = settings_screen(&fake);
    assert_shows(
        &state,
        &[
            "The plugins could not be listed",
            "plugins.toml is damaged",
            "Install plugin…",
        ],
    );
    assert_hides(&state, &["No plugins installed."]);
}

#[test]
fn details_show_what_a_plugin_can_and_cannot_do_and_where_it_came_from() {
    let fake = Fake::with_rows(vec![plugin("standard-ebooks", "Standard Ebooks", "1.2.0")]);
    let mut state = settings_screen(&fake);
    click(&mut state, "Details", &fake);
    assert_shows(
        &state,
        &[
            "Hide",
            "THIS PLUGIN CAN",
            "standardebooks.org",
            "*.standardebooks.org",
            "(https only)",
            "Run for up to 30 s per search or download",
            "Declare content: metadata, public-domain (stated by its author, not checked)",
            "THIS PLUGIN CANNOT",
            "Read your files, run programs or see your library",
            "Use the network by itself — FerroLibri makes every request",
            "SOURCE",
            "GitHub · janedoe/ferrolibri-standard-ebooks · v1.0.0 · installed 17 Oct 2026",
            "Update or reinstall…",
            "Remove…",
        ],
    );
    assert_hides(&state, &["SETTINGS", "Save settings"]);
    click(&mut state, "Hide", &fake);
    assert_hides(&state, &["THIS PLUGIN CAN"]);
}

#[test]
fn permissions_point_out_a_long_time_budget_an_entered_address_and_no_network() {
    let mut long = plugin("slow", "Slow", "1.0.0");
    {
        let info = long.info.as_mut().unwrap();
        info.permissions = PermissionSummary {
            domains: Vec::new(),
            user_address: true,
            call_timeout_s: 60,
            content: Vec::new(),
        };
    }
    let mut offline = plugin("offline", "Offline", "1.0.0");
    offline.info.as_mut().unwrap().permissions = PermissionSummary {
        call_timeout_s: 30,
        ..Default::default()
    };
    let fake = Fake::with_rows(vec![long]);
    let mut state = settings_screen(&fake);
    plugins(&mut state, Msg::ToggleDetails("slow".into()), &fake);
    assert_shows(
        &state,
        &[
            "Contact the address you enter in its settings, and that address’s subdomains — only after you save it",
            "Run for up to 60 s per search or download (longer than the usual 30 s)",
        ],
    );
    assert_hides(&state, &["Use no network at all", "Declare content"]);
    lock(&fake.script).rows = vec![offline];
    send(&mut state, Message::Navigate(Screen::Settings), &fake);
    send(
        &mut state,
        Message::SetSettingsTab(ferrolibri_ui::app::SettingsTab::Plugins),
        &fake,
    );
    plugins(&mut state, Msg::ToggleDetails("offline".into()), &fake);
    assert_shows(&state, &["Use no network at all"]);
}

#[test]
fn turning_a_plugin_on_and_off_asks_the_service_and_says_so() {
    let fake = Fake::with_rows(vec![plugin("standard-ebooks", "Standard Ebooks", "1.2.0")]);
    let mut state = settings_screen(&fake);
    plugins(
        &mut state,
        Msg::SetEnabled {
            id: "standard-ebooks".into(),
            on: true,
        },
        &fake,
    );
    assert_eq!(fake.count("enable standard-ebooks true"), 1);
    assert!(state.plugins.rows[0].enabled, "the list is read again");
    assert_eq!(
        toast_texts(&state),
        ["Standard Ebooks is on. It is now a source on the Search screen."]
    );
    plugins(
        &mut state,
        Msg::SetEnabled {
            id: "standard-ebooks".into(),
            on: false,
        },
        &fake,
    );
    assert!(!state.plugins.rows[0].enabled);
    assert_eq!(
        toast_texts(&state).last().unwrap(),
        "Standard Ebooks is off."
    );
    assert!(state.plugins.pending.is_empty());
}

#[test]
fn a_switch_that_fails_is_reported_and_the_list_shows_the_truth() {
    let fake = Fake::with_rows(vec![plugin("standard-ebooks", "Standard Ebooks", "1.2.0")]);
    lock(&fake.script).toggle_error = Some("plugins.toml is damaged".into());
    let mut state = settings_screen(&fake);
    plugins(
        &mut state,
        Msg::SetEnabled {
            id: "standard-ebooks".into(),
            on: true,
        },
        &fake,
    );
    assert!(!state.plugins.rows[0].enabled);
    assert!(state.toasts.last().unwrap().error);
    assert!(toast_texts(&state)[0].contains("plugins.toml is damaged"));
}

#[test]
fn an_incompatible_plugin_lists_its_reasons_and_cannot_be_turned_on() {
    let mut row = plugin("future", "Future", "2.0.0");
    row.health = Health::Incompatible(vec![
        "Plugin API version 2 is not supported (supported: 1)".into(),
        "Requires FerroLibri 0.6.0 or newer (this is 0.1.2)".into(),
    ]);
    row.enabled = true;
    let fake = Fake::with_rows(vec![row]);
    let state = settings_screen(&fake);
    assert_shows(
        &state,
        &[
            "Not compatible",
            "This plugin cannot be used:",
            "Plugin API version 2 is not supported (supported: 1)",
            "Requires FerroLibri 0.6.0 or newer (this is 0.1.2)",
        ],
    );
    assert_hides(&state, &["On"]);
    assert!(!state.plugins.rows[0].is_usable());
}

#[test]
fn a_plugin_that_cannot_be_read_is_listed_with_the_error_and_can_be_removed() {
    let broken = PluginRow {
        id: "oapen".into(),
        name: "oapen".into(),
        version: String::new(),
        author: String::new(),
        description: String::new(),
        enabled: false,
        health: Health::Broken("plugin.js was changed after installation".into()),
        info: None,
    };
    let fake = Fake::with_rows(vec![broken]);
    let mut state = settings_screen(&fake);
    assert_shows(
        &state,
        &["Cannot be read", "plugin.js was changed after installation"],
    );
    click(&mut state, "Details", &fake);
    assert_shows(&state, &["Remove…"]);
    assert_hides(&state, &["Update or reinstall…", "THIS PLUGIN CAN"]);
    click(&mut state, "Remove…", &fake);
    click(&mut state, "Remove", &fake);
    assert_eq!(fake.count("remove oapen"), 1);
    assert!(state.plugins.rows.is_empty());
}

#[test]
fn an_enabled_plugin_without_its_address_asks_for_it_until_the_address_is_saved() {
    let mut row = with_settings(plugin("opds", "OPDS", "1.0.0"), vec![address_field(false)]);
    row.enabled = true;
    let fake = Fake::with_rows(vec![row]);
    let mut state = settings_screen(&fake);
    assert_shows(
        &state,
        &["Enter the catalog address under Details to start searching with this plugin."],
    );
    click(&mut state, "Details", &fake);
    assert_shows(
        &state,
        &[
            "SETTINGS",
            "Catalog address",
            "FerroLibri lets this plugin contact the host of this address, and its subdomains, only once you save it here.",
            "Save settings",
        ],
    );
    // A bad address is refused on the spot and nothing is saved.
    plugins(
        &mut state,
        Msg::FieldEdited {
            id: "opds".into(),
            index: 0,
            value: FieldValue::Text("nas.local".into()),
        },
        &fake,
    );
    click(&mut state, "Save settings", &fake);
    assert_shows(
        &state,
        &["Enter a full address starting with http:// or https://"],
    );
    assert_eq!(fake.count("save opds"), 0);
    // Editing clears the error; a good address is saved, with a toast, and the hint goes.
    plugins(
        &mut state,
        Msg::FieldEdited {
            id: "opds".into(),
            index: 0,
            value: FieldValue::Text("  https://nas.local  ".into()),
        },
        &fake,
    );
    assert_hides(
        &state,
        &["Enter a full address starting with http:// or https://"],
    );
    click(&mut state, "Save settings", &fake);
    assert_eq!(fake.count("save opds"), 1);
    assert_eq!(
        lock(&fake.script).saved[0].1,
        [(
            "base_url".to_owned(),
            FieldValue::Text("  https://nas.local  ".into())
        )],
        "trimming is the store's job"
    );
    assert_eq!(toast_texts(&state), ["Settings for OPDS saved."]);
    assert_hides(
        &state,
        &["Enter the catalog address under Details to start searching with this plugin."],
    );
    assert!(
        state.plugins.forms.is_empty(),
        "saved edits are no longer edits"
    );
}

#[test]
fn an_address_without_tls_is_flagged_but_accepted() {
    let row = with_settings(plugin("opds", "OPDS", "1.0.0"), vec![address_field(false)]);
    let fake = Fake::with_rows(vec![row]);
    let mut state = settings_screen(&fake);
    plugins(&mut state, Msg::ToggleDetails("opds".into()), &fake);
    plugins(
        &mut state,
        Msg::FieldEdited {
            id: "opds".into(),
            index: 0,
            value: FieldValue::Text("http://192.168.1.5:8080/opds".into()),
        },
        &fake,
    );
    assert_shows(
        &state,
        &["This address does not use https: the connection is not encrypted."],
    );
    click(&mut state, "Save settings", &fake);
    assert_eq!(fake.count("save opds"), 1);
}

#[test]
fn edits_that_are_not_saved_survive_leaving_the_screen_and_a_reload() {
    let row = with_settings(
        plugin("opds", "OPDS", "1.0.0"),
        vec![
            address_field(true),
            SettingField {
                key: "covers".into(),
                label: "Show covers".into(),
                kind: FieldKind::Bool,
                network: false,
                value: FieldValue::Bool(false),
                placeholder: None,
                saved: false,
            },
            SettingField {
                key: "order".into(),
                label: "Order".into(),
                kind: FieldKind::Choice(vec!["relevance".into(), "newest".into()]),
                network: false,
                value: FieldValue::Text("relevance".into()),
                placeholder: None,
                saved: false,
            },
        ],
    );
    let fake = Fake::with_rows(vec![row]);
    let mut state = settings_screen(&fake);
    plugins(&mut state, Msg::ToggleDetails("opds".into()), &fake);
    assert_shows(&state, &["Show covers", "Order", "Catalog address"]);
    plugins(
        &mut state,
        Msg::FieldEdited {
            id: "opds".into(),
            index: 2,
            value: FieldValue::Text("newest".into()),
        },
        &fake,
    );
    plugins(
        &mut state,
        Msg::FieldEdited {
            id: "opds".into(),
            index: 1,
            value: FieldValue::Bool(true),
        },
        &fake,
    );
    send(&mut state, Message::Navigate(Screen::Jobs), &fake);
    send(&mut state, Message::Navigate(Screen::Settings), &fake);
    send(
        &mut state,
        Message::SetSettingsTab(ferrolibri_ui::app::SettingsTab::Plugins),
        &fake,
    );
    let form = &state.plugins.forms["opds"];
    assert_eq!(form.values[1], FieldValue::Bool(true));
    assert_eq!(form.values[2], FieldValue::Text("newest".into()));
    assert_eq!(state.plugins.expanded.as_deref(), Some("opds"));
    assert_eq!(fake.count("save opds"), 0);
    // Saving sends the whole form, in the order of the manifest.
    click(&mut state, "Save settings", &fake);
    let saved = lock(&fake.script).saved.clone();
    assert_eq!(
        saved[0].1,
        [
            (
                "base_url".to_owned(),
                FieldValue::Text("https://nas.local".into())
            ),
            ("covers".to_owned(), FieldValue::Bool(true)),
            ("order".to_owned(), FieldValue::Text("newest".into())),
        ]
    );
}

#[test]
fn removing_asks_first_and_keeping_changes_nothing() {
    let fake = Fake::with_rows(vec![plugin("standard-ebooks", "Standard Ebooks", "1.2.0")]);
    let mut state = settings_screen(&fake);
    click(&mut state, "Details", &fake);
    click(&mut state, "Remove…", &fake);
    assert_shows(
        &state,
        &[
            "Remove “Standard Ebooks”?",
            "The plugin and its saved settings are deleted. Books you added with it stay in your library.",
            "Keep it",
        ],
    );
    assert_hides(&state, &["Update or reinstall…"]);
    click(&mut state, "Keep it", &fake);
    assert_eq!(fake.count("remove standard-ebooks"), 0);
    assert_shows(&state, &["Remove…", "Update or reinstall…"]);
    click(&mut state, "Remove…", &fake);
    click(&mut state, "Remove", &fake);
    assert_eq!(fake.count("remove standard-ebooks"), 1);
    assert!(state.plugins.rows.is_empty());
    assert_eq!(toast_texts(&state), ["Removed Standard Ebooks."]);
    assert_shows(&state, &["No plugins installed."]);
}

// ---- the install dialog ------------------------------------------------------------------------

fn open_dialog(state: &mut State, fake: &Fake) {
    click(state, "Install plugin…", fake);
    assert!(state.plugins.install.is_some());
}

fn type_folder(state: &mut State, services: &dyn Services, path: &str) {
    plugins(state, Msg::FolderInput(path.into()), services);
}

#[test]
fn the_dialog_opens_on_the_source_step_and_checks_only_a_filled_field() {
    let fake = Fake::default();
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    assert_shows(
        &state,
        &[
            "Install plugin",
            "1. Source",
            "2. Check",
            "3. Confirm",
            "Folder",
            "Web address",
            "Plugin folder",
            "Browse…",
            "The folder must contain plugin.toml and the plugin file. Nothing is copied or run until you confirm.",
            "Cancel",
            "Check",
        ],
    );
    // Check does nothing while the field is empty.
    plugins(&mut state, Msg::Check, &fake);
    assert_eq!(state.plugins.install.as_ref().unwrap().step, 1);
    assert!(fake.calls().iter().all(|c| !c.starts_with("check")));
    click(&mut state, "Web address", &fake);
    assert_shows(
        &state,
        &[
            "A GitHub repository (its latest releases are listed) or a direct link to plugin.toml. Nothing is installed until you confirm.",
        ],
    );
    assert_hides(&state, &["Browse…"]);
    assert_eq!(state.plugins.install.as_ref().unwrap().mode, Mode::Address);
}

#[test]
fn browse_fills_the_folder_from_the_picker() {
    let fake = Fake::default();
    lock(&fake.script)
        .picked
        .push_back(PathBuf::from("D:\\Plugins\\standard-ebooks"));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    click(&mut state, "Browse…", &fake);
    assert_eq!(
        state.plugins.install.as_ref().unwrap().folder,
        "D:\\Plugins\\standard-ebooks"
    );
    // A cancelled picker changes nothing.
    click(&mut state, "Browse…", &fake);
    assert_eq!(
        state.plugins.install.as_ref().unwrap().folder,
        "D:\\Plugins\\standard-ebooks"
    );
}

#[test]
fn a_compatible_folder_goes_check_then_confirm_then_install_and_is_not_turned_on_unasked() {
    let fake = Fake::default();
    lock(&fake.script)
        .checks
        .push_back(Ok(Checked::Candidate(Box::new(candidate(
            "Standard Ebooks",
            "1.2.0",
            Change::New,
        )))));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\Plugins\\standard-ebooks");
    click(&mut state, "Check", &fake);
    assert_eq!(
        fake.calls().last().unwrap(),
        r#"check Folder("D:\\Plugins\\standard-ebooks")"#
    );
    assert_shows(
        &state,
        &[
            "✓ Source",
            "2. Check",
            "Standard Ebooks",
            "v1.2.0",
            "Compatible",
            "Author",
            "Jane Doe",
            "License",
            "MIT",
            "Homepage",
            "Plugin API",
            "1 (supported)",
            "Back",
            "Continue",
        ],
    );
    assert_hides(&state, &["Install"]);
    click(&mut state, "Continue", &fake);
    assert_shows(
        &state,
        &[
            "✓ Check",
            "3. Confirm",
            "by Jane Doe",
            "THIS PLUGIN CAN",
            "THIS PLUGIN CANNOT",
            "Turn on after installing",
            "Install",
        ],
    );
    // Nothing is installed before the button is pressed.
    assert_eq!(fake.count("install standard-ebooks"), 0);
    plugins(&mut state, Msg::TurnOn(false), &fake);
    lock(&fake.script).rows = vec![plugin("standard-ebooks", "Standard Ebooks", "1.2.0")];
    click(&mut state, "Install", &fake);
    assert_eq!(fake.count("install standard-ebooks"), 1);
    assert_eq!(
        lock(&fake.script).installed_with,
        [InstallOptions {
            turn_on: false,
            allow_downgrade: false
        }]
    );
    assert!(state.plugins.install.is_none(), "the dialog closes");
    assert_eq!(toast_texts(&state), ["Installed Standard Ebooks v1.2.0."]);
    assert_eq!(state.plugins.rows.len(), 1, "the list is read again");
}

#[test]
fn turning_on_after_installing_is_offered_checked_and_reported() {
    let fake = Fake::default();
    lock(&fake.script)
        .checks
        .push_back(Ok(Checked::Candidate(Box::new(candidate(
            "Standard Ebooks",
            "1.2.0",
            Change::New,
        )))));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\p");
    plugins(&mut state, Msg::Check, &fake);
    plugins(&mut state, Msg::Continue, &fake);
    assert!(
        state.plugins.install.as_ref().unwrap().turn_on,
        "the box starts checked"
    );
    click(&mut state, "Install", &fake);
    assert_eq!(
        toast_texts(&state),
        ["Installed Standard Ebooks v1.2.0 and turned it on."]
    );
}

#[test]
fn an_incompatible_plugin_shows_why_and_cannot_continue() {
    let fake = Fake::default();
    lock(&fake.script)
        .checks
        .push_back(Ok(Checked::Candidate(Box::new(incompatible()))));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\p");
    plugins(&mut state, Msg::Check, &fake);
    assert_shows(
        &state,
        &[
            "Not compatible",
            "This plugin cannot be installed",
            "Plugin API version 2 is not supported (supported: 1)",
            "Requires FerroLibri 0.6.0 or newer (this is 0.1.2)",
            "Look for a release that supports plugin API 1, or ask the author for one.",
            "Back",
        ],
    );
    assert_hides(&state, &["Continue"]);
    // Even a stray message cannot get past the check.
    plugins(&mut state, Msg::Continue, &fake);
    assert_eq!(state.plugins.install.as_ref().unwrap().step, 2);
    plugins(&mut state, Msg::Install, &fake);
    assert_eq!(fake.count("install standard-ebooks"), 0);
}

#[test]
fn a_github_address_lists_releases_and_only_a_compatible_one_can_be_picked() {
    let fake = Fake::default();
    {
        let mut script = lock(&fake.script);
        script.checks.push_back(Ok(Checked::Releases(releases())));
        script
            .inspections
            .push_back(Ok(candidate("Standard Ebooks", "1.2.0", Change::New)));
    }
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    plugins(&mut state, Msg::SetMode(Mode::Address), &fake);
    plugins(
        &mut state,
        Msg::AddressInput("https://github.com/janedoe/ferrolibri-standard-ebooks".into()),
        &fake,
    );
    plugins(&mut state, Msg::Check, &fake);
    assert_shows(
        &state,
        &[
            "janedoe/ferrolibri-standard-ebooks",
            "— the latest 5, pre-releases hidden. Choose one to check.",
            "v2.0.0",
            "14 Oct 2026",
            "Plugin API version 2 is not supported",
            "v1.2.0",
            "v1.1.0",
            "Back",
        ],
    );
    assert_hides(&state, &["No compatible release"]);
    // The incompatible release does nothing.
    plugins(&mut state, Msg::PickRelease("v2.0.0".into()), &fake);
    assert_eq!(fake.count("release v2.0.0"), 0);
    click(&mut state, "v1.2.0", &fake);
    assert_eq!(fake.count("release v1.2.0"), 1);
    assert_shows(&state, &["Standard Ebooks", "Compatible", "Continue"]);
    // Back from a checked release returns to the list, back from the list to the source.
    click(&mut state, "Back", &fake);
    assert_shows(
        &state,
        &[
            "v1.1.0",
            "— the latest 5, pre-releases hidden. Choose one to check.",
        ],
    );
    click(&mut state, "Back", &fake);
    assert_shows(&state, &["1. Source", "Web address"]);
    assert_eq!(
        state.plugins.install.as_ref().unwrap().address,
        "https://github.com/janedoe/ferrolibri-standard-ebooks",
        "what was typed stays"
    );
}

#[test]
fn a_repository_without_a_compatible_release_says_so() {
    let fake = Fake::default();
    let mut all_bad = releases();
    all_bad.items.retain(|r| !r.compatible);
    lock(&fake.script)
        .checks
        .push_back(Ok(Checked::Releases(all_bad)));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    plugins(&mut state, Msg::SetMode(Mode::Address), &fake);
    plugins(
        &mut state,
        Msg::AddressInput("https://github.com/a/b".into()),
        &fake,
    );
    plugins(&mut state, Msg::Check, &fake);
    assert_shows(
        &state,
        &[
            "No compatible release",
            "None of the latest releases works with this FerroLibri.",
        ],
    );
}

#[test]
fn a_checksum_failure_offers_another_release_and_nothing_is_installed() {
    let fake = Fake::default();
    {
        let mut script = lock(&fake.script);
        script.checks.push_back(Ok(Checked::Releases(releases())));
        script.inspections.push_back(Err(problem(
            "Checksum does not match",
            "plugin.js does not match the SHA-256 published by GitHub. Nothing was installed.",
            ProblemKind::Checksum,
        )));
    }
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    plugins(&mut state, Msg::SetMode(Mode::Address), &fake);
    plugins(
        &mut state,
        Msg::AddressInput("https://github.com/a/b".into()),
        &fake,
    );
    plugins(&mut state, Msg::Check, &fake);
    plugins(&mut state, Msg::PickRelease("v1.2.0".into()), &fake);
    assert_shows(
        &state,
        &[
            "Checksum does not match",
            "plugin.js does not match the SHA-256 published by GitHub. Nothing was installed.",
            "Choose another release",
        ],
    );
    assert_hides(&state, &["Try again", "Continue"]);
    click(&mut state, "Choose another release", &fake);
    assert_shows(&state, &["v1.2.0", "v1.1.0"]);
    assert_eq!(fake.count("install standard-ebooks"), 0);
}

#[test]
fn network_errors_can_be_tried_again_but_the_github_limit_cannot() {
    let fake = Fake::default();
    {
        let mut script = lock(&fake.script);
        script.checks.push_back(Err(problem(
            "Could not reach the address",
            "No connection to github.com (timeout: slow). Check your network and try again.",
            ProblemKind::Failed,
        )));
        script
            .checks
            .push_back(Ok(Checked::Candidate(Box::new(candidate(
                "Standard Ebooks",
                "1.2.0",
                Change::New,
            )))));
        script.checks.push_back(Err(problem(
            "GitHub request limit reached",
            "GitHub allows 60 requests an hour without signing in, and that limit is used up. Try again in about 38 minutes, install from a folder, or paste a direct link to plugin.toml.",
            ProblemKind::RateLimited,
        )));
    }
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    plugins(&mut state, Msg::SetMode(Mode::Address), &fake);
    plugins(
        &mut state,
        Msg::AddressInput("https://github.com/a/b".into()),
        &fake,
    );
    plugins(&mut state, Msg::Check, &fake);
    assert_shows(
        &state,
        &["Could not reach the address", "Back", "Try again"],
    );
    click(&mut state, "Try again", &fake);
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| c.starts_with("check"))
            .count(),
        2
    );
    assert_shows(&state, &["Standard Ebooks", "Continue"]);
    click(&mut state, "Back", &fake);
    assert_shows(&state, &["1. Source"]);
    plugins(&mut state, Msg::Check, &fake);
    assert_shows(&state, &["GitHub request limit reached", "Back"]);
    assert_hides(&state, &["Try again"]);
}

#[test]
fn cancel_works_at_every_step() {
    let fake = Fake::default();
    {
        let mut script = lock(&fake.script);
        for _ in 0..3 {
            script
                .checks
                .push_back(Ok(Checked::Candidate(Box::new(candidate(
                    "Standard Ebooks",
                    "1.2.0",
                    Change::New,
                )))));
        }
    }
    let mut state = settings_screen(&fake);
    // Step 1: Cancel closes the dialog.
    open_dialog(&mut state, &fake);
    click(&mut state, "Cancel", &fake);
    assert!(state.plugins.install.is_none());
    // Step 1: the X and Esc close it too.
    open_dialog(&mut state, &fake);
    plugins(&mut state, Msg::CloseInstall, &fake);
    assert!(state.plugins.install.is_none());
    open_dialog(&mut state, &fake);
    send(
        &mut state,
        Message::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::Escape),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        }),
        &fake,
    );
    assert!(state.plugins.install.is_none(), "Esc closes the dialog");
    // Step 2, waiting: Cancel goes back to the source and the late answer is ignored.
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\p");
    let pending = app::update(&mut state, Message::Plugins(Msg::Check), &fake);
    assert_shows(
        &state,
        &["Reading plugin.toml and checking compatibility…", "Cancel"],
    );
    click(&mut state, "Cancel", &fake);
    assert_shows(&state, &["1. Source", "Plugin folder"]);
    block_on(run(&mut state, pending, &fake));
    assert_eq!(state.plugins.install.as_ref().unwrap().step, 1);
    assert!(state.plugins.install.as_ref().unwrap().candidate.is_none());
    // Step 2, checked: Back returns to the source.
    plugins(&mut state, Msg::Check, &fake);
    assert!(state.plugins.install.as_ref().unwrap().candidate.is_some());
    click(&mut state, "Back", &fake);
    assert_eq!(state.plugins.install.as_ref().unwrap().step, 1);
    // Step 3: Back returns to the check, Esc closes without installing.
    plugins(&mut state, Msg::Check, &fake);
    plugins(&mut state, Msg::Continue, &fake);
    click(&mut state, "Back", &fake);
    assert_eq!(state.plugins.install.as_ref().unwrap().step, 2);
    plugins(&mut state, Msg::Continue, &fake);
    plugins(&mut state, Msg::CloseInstall, &fake);
    assert!(state.plugins.install.is_none());
    assert_eq!(fake.count("install standard-ebooks"), 0);
}

#[test]
fn closing_while_a_check_runs_drops_its_answer() {
    let fake = Fake::default();
    lock(&fake.script)
        .checks
        .push_back(Ok(Checked::Candidate(Box::new(candidate(
            "Standard Ebooks",
            "1.2.0",
            Change::New,
        )))));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\p");
    let pending = app::update(&mut state, Message::Plugins(Msg::Check), &fake);
    plugins(&mut state, Msg::CloseInstall, &fake);
    open_dialog(&mut state, &fake);
    block_on(run(&mut state, pending, &fake));
    let install = state.plugins.install.as_ref().unwrap();
    assert!(
        install.candidate.is_none() && install.step == 1,
        "the new dialog is untouched"
    );
}

#[test]
fn updating_reinstalling_and_going_back_say_what_they_do() {
    for (change, button, note, toast) in [
        (
            Change::Upgrade {
                from: "1.0.0".into(),
                to: "1.2.0".into(),
            },
            "Update to v1.2.0",
            "Replaces your v1.0.0.",
            "Updated Standard Ebooks v1.2.0.",
        ),
        (
            Change::Reinstall {
                version: "1.2.0".into(),
            },
            "Reinstall v1.2.0",
            "Same version as the one installed (v1.2.0).",
            "Reinstalled Standard Ebooks v1.2.0.",
        ),
        (
            Change::Downgrade {
                from: "1.3.0".into(),
                to: "1.2.0".into(),
            },
            "Install older version",
            "Replaces your newer v1.3.0 with an older version.",
            "Installed older version of Standard Ebooks v1.2.0.",
        ),
    ] {
        let downgrade = matches!(change, Change::Downgrade { .. });
        let upgrade = matches!(change, Change::Upgrade { .. });
        let fake = Fake::with_rows(vec![plugin("standard-ebooks", "Standard Ebooks", "1.0.0")]);
        lock(&fake.script)
            .checks
            .push_back(Ok(Checked::Candidate(Box::new(candidate(
                "Standard Ebooks",
                "1.2.0",
                change,
            )))));
        let mut state = settings_screen(&fake);
        click(&mut state, "Details", &fake);
        click(&mut state, "Update or reinstall…", &fake);
        assert_eq!(
            state.plugins.install.as_ref().unwrap().mode,
            Mode::Address,
            "a plugin from GitHub is updated from an address"
        );
        plugins(&mut state, Msg::SetMode(Mode::Folder), &fake);
        type_folder(&mut state, &fake, "D:\\p");
        plugins(&mut state, Msg::Check, &fake);
        plugins(&mut state, Msg::Continue, &fake);
        assert_shows(&state, &[note, button]);
        assert_hides(&state, &["Turn on after installing"]);
        assert_eq!(
            shows(
                &state,
                "Settings you saved are kept. If the new version adds an address setting that can reach the network, enter it again."
            ),
            upgrade
        );
        if downgrade {
            assert_shows(
                &state,
                &["I want to go back to v1.2.0 even though v1.3.0 is installed"],
            );
            // The button is dead until the box is checked.
            plugins(&mut state, Msg::Install, &fake);
            assert_eq!(fake.count("install standard-ebooks"), 0);
            click(
                &mut state,
                "I want to go back to v1.2.0 even though v1.3.0 is installed",
                &fake,
            );
            assert!(state.plugins.install.as_ref().unwrap().allow_downgrade);
        }
        click(&mut state, button, &fake);
        assert_eq!(fake.count("install standard-ebooks"), 1, "{button}");
        assert_eq!(
            lock(&fake.script).installed_with[0].allow_downgrade,
            downgrade
        );
        assert_eq!(toast_texts(&state), [toast]);
    }
}

#[test]
fn a_failed_installation_keeps_the_dialog_open_with_the_reason() {
    let fake = Fake::default();
    {
        let mut script = lock(&fake.script);
        script
            .checks
            .push_back(Ok(Checked::Candidate(Box::new(candidate(
                "Standard Ebooks",
                "1.2.0",
                Change::New,
            )))));
        script.installs.push_back(Err(problem(
            "Could not install",
            "Access is denied.",
            ProblemKind::Failed,
        )));
    }
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\p");
    plugins(&mut state, Msg::Check, &fake);
    plugins(&mut state, Msg::Continue, &fake);
    click(&mut state, "Install", &fake);
    assert!(
        state.plugins.install.is_some(),
        "the user can read it and go back"
    );
    assert_eq!(state.plugins.install.as_ref().unwrap().busy, None);
    assert_eq!(
        toast_texts(&state),
        ["Could not install. Access is denied."]
    );
    assert!(state.toasts[0].error);
}

#[test]
fn a_plugin_cannot_be_installed_twice_at_once() {
    let fake = Fake::default();
    lock(&fake.script)
        .checks
        .push_back(Ok(Checked::Candidate(Box::new(candidate(
            "Standard Ebooks",
            "1.2.0",
            Change::New,
        )))));
    let mut state = settings_screen(&fake);
    open_dialog(&mut state, &fake);
    type_folder(&mut state, &fake, "D:\\p");
    plugins(&mut state, Msg::Check, &fake);
    plugins(&mut state, Msg::Continue, &fake);
    let first = app::update(&mut state, Message::Plugins(Msg::Install), &fake);
    let second = app::update(&mut state, Message::Plugins(Msg::Install), &fake);
    block_on(async {
        run(&mut state, first, &fake).await;
        run(&mut state, second, &fake).await;
    });
    assert_eq!(fake.count("install standard-ebooks"), 1);
}

// ---- the real folder installation --------------------------------------------------------------

fn write_plugin(dir: &std::path::Path, manifest: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    std::fs::write(
        dir.join("plugin.js"),
        "export function search(query, ctx) { return { hits: [] }; }\n",
    )
    .unwrap();
}

const MANIFEST: &str = r#"
id = "demo-books"
name = "Demo Books"
version = "1.0.0"
api_version = 1
author = "Jane Doe"
license = "MIT"
description = "A demo"
provides = ["search"]
content = ["metadata"]

[network]
domains = ["books.example.org"]

[[settings]]
key = "base_url"
type = "url"
label = "Catalog address"
network = true
default = "https://opds.example.org"

[[settings]]
key = "covers"
type = "bool"
label = "Show covers"
default = true
"#;

struct Real {
    _root: tempfile::TempDir,
    services: RealServices,
    source: PathBuf,
}

fn real() -> Real {
    let root = tempfile::tempdir().unwrap();
    let services =
        RealServices::with_paths(root.path().join("config.toml"), root.path().join("ui.toml"))
            .unwrap();
    let source = root.path().join("demo-books-src");
    write_plugin(&source, MANIFEST);
    Real {
        _root: root,
        services,
        source,
    }
}

#[test]
fn the_real_folder_install_lists_configures_updates_and_removes_a_plugin() {
    let real = real();
    let services = &real.services;
    let mut state = settings_screen_with(services);
    assert!(state.plugins.rows.is_empty());
    assert_shows(&state, &["No plugins installed."]);

    // Source → Check → Confirm → Install, not turned on.
    click(&mut state, "Install plugin…", services);
    type_folder(&mut state, services, &real.source.display().to_string());
    click(&mut state, "Check", services);
    assert_shows(
        &state,
        &[
            "Demo Books",
            "v1.0.0",
            "Compatible",
            "Jane Doe",
            "MIT",
            "1 (supported)",
        ],
    );
    click(&mut state, "Continue", services);
    assert_shows(
        &state,
        &[
            "books.example.org",
            "Contact the address you enter in its settings, and that address’s subdomains — only after you save it",
        ],
    );
    plugins(&mut state, Msg::TurnOn(false), services);
    click(&mut state, "Install", services);
    assert_eq!(toast_texts(&state), ["Installed Demo Books v1.0.0."]);
    assert_eq!(state.plugins.rows.len(), 1);
    let row = state.plugins.rows[0].clone();
    assert!(
        !row.enabled && row.is_usable(),
        "installing does not turn it on"
    );
    let info = row.info.unwrap();
    assert!(info.source.starts_with("Folder · "));
    assert_eq!(info.settings.len(), 2);
    assert_eq!(info.settings[0].value, FieldValue::Text(String::new()));
    assert_eq!(
        info.settings[0].placeholder.as_deref(),
        Some("https://opds.example.org")
    );
    assert_eq!(info.settings[1].value, FieldValue::Bool(true));

    // On, which now asks for the address; saving it clears the hint.
    plugins(
        &mut state,
        Msg::SetEnabled {
            id: "demo-books".into(),
            on: true,
        },
        services,
    );
    assert!(state.plugins.rows[0].enabled);
    assert_shows(
        &state,
        &["Enter the catalog address under Details to start searching with this plugin."],
    );
    plugins(
        &mut state,
        Msg::ToggleDetails("demo-books".into()),
        services,
    );
    plugins(
        &mut state,
        Msg::FieldEdited {
            id: "demo-books".into(),
            index: 0,
            value: FieldValue::Text("  https://nas.example.org/opds  ".into()),
        },
        services,
    );
    click(&mut state, "Save settings", services);
    let saved = &state.plugins.rows[0].info.as_ref().unwrap().settings[0];
    assert_eq!(
        saved.value,
        FieldValue::Text("https://nas.example.org/opds".into())
    );
    assert!(saved.saved);
    assert_hides(
        &state,
        &["Enter the catalog address under Details to start searching with this plugin."],
    );

    // Installing the same folder again is a reinstall that keeps the settings and the switch.
    click(&mut state, "Update or reinstall…", services);
    type_folder(&mut state, services, &real.source.display().to_string());
    click(&mut state, "Check", services);
    assert_shows(&state, &["Compatible"]);
    click(&mut state, "Continue", services);
    assert_shows(
        &state,
        &[
            "Same version as the one installed (v1.0.0).",
            "Reinstall v1.0.0",
        ],
    );
    click(&mut state, "Reinstall v1.0.0", services);
    assert_eq!(
        toast_texts(&state).last().unwrap(),
        "Reinstalled Demo Books v1.0.0."
    );
    let row = &state.plugins.rows[0];
    assert!(row.enabled);
    assert!(row.info.as_ref().unwrap().settings[0].saved);

    // Off, then remove.
    plugins(
        &mut state,
        Msg::SetEnabled {
            id: "demo-books".into(),
            on: false,
        },
        services,
    );
    assert!(!state.plugins.rows[0].enabled);
    click(&mut state, "Remove…", services);
    click(&mut state, "Remove", services);
    assert!(state.plugins.rows.is_empty());
    assert_shows(&state, &["No plugins installed."]);
}

#[test]
fn the_real_install_can_turn_a_new_plugin_on() {
    let real = real();
    let services = &real.services;
    let mut state = settings_screen_with(services);
    click(&mut state, "Install plugin…", services);
    type_folder(&mut state, services, &real.source.display().to_string());
    plugins(&mut state, Msg::Check, services);
    plugins(&mut state, Msg::Continue, services);
    click(&mut state, "Install", services);
    assert_eq!(
        toast_texts(&state),
        ["Installed Demo Books v1.0.0 and turned it on."]
    );
    assert!(state.plugins.rows[0].enabled);
}

#[test]
fn the_real_check_explains_a_folder_that_is_not_a_plugin_and_an_unsupported_api() {
    let real = real();
    let services = &real.services;
    let mut state = settings_screen_with(services);
    let empty = real.source.parent().unwrap().join("empty");
    std::fs::create_dir(&empty).unwrap();
    click(&mut state, "Install plugin…", services);
    type_folder(&mut state, services, &empty.display().to_string());
    click(&mut state, "Check", services);
    assert_shows(
        &state,
        &["This folder is not a plugin", "Back", "Try again"],
    );
    click(&mut state, "Back", services);

    let future = real.source.parent().unwrap().join("future");
    write_plugin(
        &future,
        &MANIFEST.replace("api_version = 1", "api_version = 2"),
    );
    type_folder(&mut state, services, &future.display().to_string());
    click(&mut state, "Check", services);
    assert_shows(&state, &["This plugin cannot be installed"]);
    let message = &state
        .plugins
        .install
        .as_ref()
        .unwrap()
        .problem
        .as_ref()
        .unwrap()
        .message;
    assert!(
        message.contains("Plugin API version 2 is not supported"),
        "{message}"
    );
    assert!(
        message
            .ends_with("Look for a release that supports plugin API 1, or ask the author for one."),
        "{message}"
    );
    click(&mut state, "Back", services);

    let missing = real.source.parent().unwrap().join("nowhere");
    type_folder(&mut state, services, &missing.display().to_string());
    click(&mut state, "Check", services);
    assert_shows(&state, &["This folder is not a plugin"]);
    // Nothing was installed by any of this.
    assert!(
        std::fs::read_dir(real.source.parent().unwrap().join("plugins"))
            .map(|entries| entries.count() == 0)
            .unwrap_or(true)
    );
}

#[test]
fn the_real_service_refuses_a_downgrade_unless_confirmed() {
    let real = real();
    let services = &real.services;
    let mut state = settings_screen_with(services);
    let newer = real.source.parent().unwrap().join("newer");
    write_plugin(
        &newer,
        &MANIFEST.replace("version = \"1.0.0\"", "version = \"1.3.0\""),
    );
    for folder in [&newer, &real.source] {
        click(&mut state, "Install plugin…", services);
        type_folder(&mut state, services, &folder.display().to_string());
        plugins(&mut state, Msg::Check, services);
        plugins(&mut state, Msg::Continue, services);
        if folder == &real.source {
            assert_shows(
                &state,
                &[
                    "Install older version",
                    "Replaces your newer v1.3.0 with an older version.",
                ],
            );
            plugins(&mut state, Msg::Install, services);
            assert_eq!(
                state.plugins.rows[0].version, "1.3.0",
                "refused without the box"
            );
            plugins(&mut state, Msg::AllowDowngrade(true), services);
        }
        click(
            &mut state,
            if folder == &real.source {
                "Install older version"
            } else {
                "Install"
            },
            services,
        );
    }
    assert_eq!(state.plugins.rows[0].version, "1.0.0");
}

fn settings_screen_with(services: &dyn Services) -> State {
    let mut state = State::default();
    state.window_width = 1100.0;
    send(&mut state, Message::Navigate(Screen::Settings), services);
    send(
        &mut state,
        Message::SetSettingsTab(ferrolibri_ui::app::SettingsTab::Plugins),
        services,
    );
    state
}
