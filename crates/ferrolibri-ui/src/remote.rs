//! Search sources on the Search screen (plan item 12): the switch between the library and a
//! search plugin, a plugin's results, and the details of the selected result.
//!
//! Everything that touches a plugin or the network goes through [`Services`]; this module is
//! the state and the pure `update` step. The screens are drawn in `widgets::remote` after
//! `docs/design/mockup.html` (design gaps G4 and G5).
//!
//! * A search runs on Enter, not on every letter. Every search, "Show more", a cancel and a
//!   change of source bumps one [`GenerationGuard`], so an answer that arrives late (A → B → A)
//!   is dropped, and the call that is still running is cancelled.
//! * `details` runs once per selected result and is remembered for the rest of the session.
//! * Covers are fetched through the plugin's own network rules, a few at a time, only for rows
//!   that are on screen; a cover that fails is a placeholder and is not asked for again.
use crate::{
    app::{Message, Screen, State},
    keymap::Motion,
    response_guard::GenerationGuard,
    services::{
        Services,
        plugins::{PluginRow, RemoteHit, RemotePage, SourceError, SourceErrorKind},
    },
    widgets::toasts,
};
use ferrolibri_core::converter::CancellationToken;
use iced::{Task, widget::image::Handle};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How many covers of results are kept in memory; the oldest go first.
pub const COVER_CAPACITY: usize = 200;
/// Covers fetched at the same time. The HTTP layer serves one request per host anyway.
pub const COVER_CONCURRENCY: usize = 2;
/// Details of how many results are remembered.
const DETAILS_CAPACITY: usize = 500;

/// Where the Search screen looks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SearchSource {
    #[default]
    Library,
    Plugin(String),
}

/// One button of the source switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub source: SearchSource,
    pub name: String,
    /// A plugin that is off or not compatible is shown, but cannot be chosen.
    pub usable: bool,
    /// Why not, for the hint over the button.
    pub why: &'static str,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        if !self.usable {
            f.write_str(" (off)")?;
        }
        Ok(())
    }
}

/// What the results area shows.
#[derive(Debug)]
pub enum Phase {
    /// Nothing asked yet; the text replaces the usual invitation ("Search cancelled.").
    Idle(Option<String>),
    /// The plugin needs an address before it can search.
    NeedsAddress,
    Loading,
    Failed(SourceError),
    Empty,
    Results,
}

/// The "Show more" footer.
#[derive(Debug)]
pub enum More {
    Ready,
    Loading,
    Failed(SourceError),
}

/// The state of `details` for the selected result.
#[derive(Debug)]
pub enum DetailsPhase {
    Idle,
    Loading,
    Failed(SourceError),
    Ready,
}

/// One search of the chosen plugin and what came of it.
#[derive(Debug)]
pub struct Session {
    pub plugin: String,
    /// The text that was sent.
    pub query: String,
    pub phase: Phase,
    pub hits: Vec<RemoteHit>,
    /// What the plugin gave for the next page; none on the last one.
    pub next: Option<String>,
    pub page: u32,
    pub more: More,
    /// Results the plugin sent that broke a limit.
    pub dropped: usize,
}

#[derive(Default)]
pub struct RemoteCovers {
    entries: BTreeMap<String, Option<Handle>>,
    order: VecDeque<String>,
    running: BTreeSet<String>,
    token: CancellationToken,
}

impl RemoteCovers {
    pub fn get(&self, url: &str) -> Option<&Handle> {
        self.entries.get(url).and_then(Option::as_ref)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn running(&self) -> usize {
        self.running.len()
    }
    fn insert(&mut self, url: String, handle: Option<Handle>) {
        if self.entries.insert(url.clone(), handle).is_none() {
            self.order.push_back(url);
        }
        while self.entries.len() > COVER_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
    /// The first of `wanted` that are neither here nor on their way, as many as there is room.
    fn next_requests(&mut self, wanted: &[String]) -> Vec<String> {
        let room = COVER_CONCURRENCY.saturating_sub(self.running.len());
        let picked: Vec<String> = wanted
            .iter()
            .filter(|url| !self.entries.contains_key(*url) && !self.running.contains(*url))
            .take(room)
            .cloned()
            .collect();
        self.running.extend(picked.iter().cloned());
        picked
    }
}

#[derive(Default)]
pub struct RemoteState {
    /// What was typed for each plugin; a plugin keeps its own text while another is chosen.
    pub texts: BTreeMap<String, String>,
    pub session: Option<Session>,
    /// `id` of the selected result.
    pub selected: Option<String>,
    pub details: Option<DetailsPhase>,
    /// Results as `details` completed them, by plugin and `id`.
    cache: BTreeMap<(String, String), RemoteHit>,
    pub covers: RemoteCovers,
    /// The cover grid instead of the list.
    pub grid: bool,
    guard: GenerationGuard,
    details_guard: GenerationGuard,
    token: Option<CancellationToken>,
    details_token: Option<CancellationToken>,
    /// What the plugin list looked like when the cache was filled.
    plugins_seen: Vec<(String, String, bool)>,
}

impl RemoteState {
    pub fn text(&self, plugin: &str) -> &str {
        self.texts.get(plugin).map_or("", String::as_str)
    }
    /// The results as the screen draws them, after the details that arrived.
    pub fn hit(&self, id: &str) -> Option<&RemoteHit> {
        let session = self.session.as_ref()?;
        let listed = session.hits.iter().find(|h| h.id == id)?;
        match self.cache.get(&(session.plugin.clone(), id.to_owned())) {
            Some(detailed) if detailed.title == listed.title => Some(detailed),
            _ => Some(listed),
        }
    }
    pub fn selected_hit(&self) -> Option<&RemoteHit> {
        self.hit(self.selected.as_deref()?)
    }
    pub fn busy(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| matches!(s.phase, Phase::Loading) || matches!(s.more, More::Loading))
            || matches!(self.details, Some(DetailsPhase::Loading))
    }
    /// A running call ends; whatever it answers will be ignored.
    fn cancel_search(&mut self) {
        self.guard.invalidate();
        if let Some(token) = self.token.take() {
            token.cancel();
        }
        self.covers.token.cancel();
        self.covers.token = CancellationToken::default();
    }
    fn cancel_details(&mut self) {
        self.details_guard.invalidate();
        if let Some(token) = self.details_token.take() {
            token.cancel();
        }
    }
    /// Back to the empty screen of a source: no results, nothing selected.
    fn reset(&mut self) {
        self.cancel_search();
        self.cancel_details();
        self.session = None;
        self.selected = None;
        self.details = None;
    }
}

#[derive(Clone, Debug)]
pub enum Msg {
    SourceChosen(SearchSource),
    /// The source switch with the next (or previous) usable source: the keyboard command.
    CycleSource {
        backwards: bool,
    },
    QueryChanged(String),
    Submit,
    Retry,
    Cancel,
    Found {
        generation: u64,
        result: Result<RemotePage, SourceError>,
    },
    ShowMore,
    MoreFound {
        generation: u64,
        result: Result<RemotePage, SourceError>,
    },
    Select(String),
    RetryDetails,
    DetailsLoaded {
        generation: u64,
        plugin: String,
        id: String,
        result: Result<Box<RemoteHit>, SourceError>,
    },
    CoverLoaded {
        url: String,
        generation: u64,
        result: Result<Handle, String>,
    },
    ShowList,
    ShowGrid,
    OpenPluginSettings(String),
    OpenPage(String),
    PageOpened(Result<(), String>),
}

impl From<Msg> for Message {
    fn from(msg: Msg) -> Self {
        Message::Remote(msg)
    }
}

// ---- which source is chosen ---------------------------------------------------------------

fn usable(row: &PluginRow) -> bool {
    row.enabled && row.is_usable()
}

/// The plugin that Search is using now: the one that was chosen, if it is still there, on and
/// compatible. Anything else is the library.
pub fn active(state: &State) -> Option<&PluginRow> {
    if !state.plugins.loaded {
        return None;
    }
    let id = state.visible_settings().search_source.as_deref()?;
    state.plugins.row(id).filter(|row| usable(row))
}

pub fn active_id(state: &State) -> Option<String> {
    active(state).map(|row| row.id.clone())
}

/// The Search screen is showing a plugin.
pub fn is_remote(state: &State) -> bool {
    state.screen == Screen::Search && active(state).is_some()
}

pub fn current(state: &State) -> SearchSource {
    active_id(state).map_or(SearchSource::Library, SearchSource::Plugin)
}

/// The switch is there once any plugin is installed (the mockup's rule), whether it is on or not.
pub fn switcher_visible(state: &State) -> bool {
    !state.plugins.rows.is_empty()
}

pub fn choices(state: &State) -> Vec<Choice> {
    let mut choices = vec![Choice {
        source: SearchSource::Library,
        name: "Library".into(),
        usable: true,
        why: "",
    }];
    choices.extend(state.plugins.rows.iter().map(|row| Choice {
        source: SearchSource::Plugin(row.id.clone()),
        name: row.name.clone(),
        usable: usable(row),
        why: match &row.health {
            crate::services::plugins::Health::Ready => "Turned off in Settings → Plugins",
            crate::services::plugins::Health::Incompatible(_) => {
                "Not compatible with this FerroLibri"
            }
            crate::services::plugins::Health::Broken(_) => "Cannot be read",
        },
    }));
    choices
}

/// The host a "could not connect" message names: the address the user saved for the plugin,
/// else the first site its manifest names, else the plugin itself.
pub fn host_of(row: &PluginRow) -> String {
    row.address_host().unwrap_or_else(|| row.name.clone())
}

// ---- update ---------------------------------------------------------------------------------

pub fn update(state: &mut State, msg: Msg, services: &dyn Services) -> Task<Message> {
    if state.closing {
        return Task::none();
    }
    match msg {
        Msg::SourceChosen(source) => choose(state, source, services),
        Msg::CycleSource { backwards } => {
            let usable: Vec<Choice> = choices(state).into_iter().filter(|c| c.usable).collect();
            let now = current(state);
            let at = usable.iter().position(|c| c.source == now).unwrap_or(0);
            let next = if backwards {
                (at + usable.len() - 1) % usable.len()
            } else {
                (at + 1) % usable.len()
            };
            match usable.get(next) {
                Some(choice) if choice.source != now => {
                    choose(state, choice.source.clone(), services)
                }
                _ => Task::none(),
            }
        }
        Msg::QueryChanged(text) => {
            if let Some(id) = active_id(state) {
                state.remote.texts.insert(id, text);
            }
            Task::none()
        }
        Msg::Submit => {
            let Some(id) = active_id(state) else {
                return Task::none();
            };
            let text = state.remote.text(&id).trim().to_owned();
            search(state, id, text, services)
        }
        Msg::Retry => {
            let Some(id) = active_id(state) else {
                return Task::none();
            };
            let query = state
                .remote
                .session
                .as_ref()
                .map(|s| s.query.clone())
                .unwrap_or_else(|| state.remote.text(&id).trim().to_owned());
            search(state, id, query, services)
        }
        Msg::Cancel => {
            if state
                .remote
                .session
                .as_ref()
                .is_some_and(|s| matches!(s.phase, Phase::Loading))
            {
                state.remote.reset();
                state.remote.session = Some(Session {
                    plugin: active_id(state).unwrap_or_default(),
                    query: String::new(),
                    phase: Phase::Idle(Some("Search cancelled.".into())),
                    hits: Vec::new(),
                    next: None,
                    page: 0,
                    more: More::Ready,
                    dropped: 0,
                });
            }
            Task::none()
        }
        Msg::Found { generation, result } => found(state, generation, result, services),
        Msg::ShowMore => show_more(state, services),
        Msg::MoreFound { generation, result } => more_found(state, generation, result, services),
        Msg::Select(id) => select(state, &id, false, services),
        Msg::RetryDetails => match state.remote.selected.clone() {
            Some(id) => select(state, &id, true, services),
            None => Task::none(),
        },
        Msg::DetailsLoaded {
            generation,
            plugin,
            id,
            result,
        } => details_loaded(state, generation, plugin, id, result, services),
        Msg::CoverLoaded {
            url,
            generation,
            result,
        } => {
            state.remote.covers.running.remove(&url);
            match result {
                Ok(handle) => state.remote.covers.insert(url, Some(handle)),
                Err(reason) if state.remote.guard.accepts(generation) => {
                    log::warn!("could not load the cover {url}: {reason}");
                    state.remote.covers.insert(url, None);
                }
                // Cancelled with its search: the cover may be wanted again.
                Err(_) => {}
            }
            request_covers(state, services)
        }
        Msg::ShowList => {
            state.remote.grid = false;
            state.list_offset = 0.0;
            Task::batch([
                crate::app::reset_list_scroll(),
                request_covers(state, services),
            ])
        }
        Msg::ShowGrid => {
            state.remote.grid = true;
            state.list_offset = 0.0;
            Task::batch([
                crate::app::reset_list_scroll(),
                request_covers(state, services),
            ])
        }
        Msg::OpenPluginSettings(id) => {
            state.plugins.expanded = Some(id);
            state.settings_tab = crate::app::SettingsTab::Plugins;
            crate::app::update(state, Message::Navigate(Screen::Settings), services)
        }
        Msg::OpenPage(url) => Task::perform(services.open_url(url), |r| Msg::PageOpened(r).into()),
        Msg::PageOpened(result) => {
            if let Err(error) = result {
                toasts::push(state, format!("Could not open the page. {error}"), true);
            }
            Task::none()
        }
    }
}

fn choose(state: &mut State, source: SearchSource, services: &dyn Services) -> Task<Message> {
    if source == current(state) {
        return Task::none();
    }
    let id = match &source {
        SearchSource::Library => None,
        SearchSource::Plugin(id) => match state.plugins.row(id) {
            Some(row) if usable(row) => Some(id.clone()),
            // Off or not compatible: the button is inactive, and a list entry does nothing.
            _ => return Task::none(),
        },
    };
    state.remote.reset();
    state.downloads.menu = None;
    state.list_offset = 0.0;
    let mut settings = state.visible_settings().clone();
    settings.search_source = id;
    Task::batch([
        crate::app::update(state, Message::SaveSettings(settings), services),
        crate::app::reset_list_scroll(),
        iced::widget::operation::focus(crate::app::search_id()),
    ])
}

/// Sends `text` to the plugin as the first page. A search that is still running is cancelled.
fn search(
    state: &mut State,
    plugin: String,
    text: String,
    services: &dyn Services,
) -> Task<Message> {
    if text.is_empty() {
        return Task::none();
    }
    let Some(row) = state.plugins.row(&plugin) else {
        return Task::none();
    };
    let needs_address = row.needs_setup();
    state.remote.reset();
    state.downloads.menu = None;
    state.list_offset = 0.0;
    let mut session = Session {
        plugin: plugin.clone(),
        query: text.clone(),
        phase: Phase::Loading,
        hits: Vec::new(),
        next: None,
        page: 1,
        more: More::Ready,
        dropped: 0,
    };
    if needs_address {
        session.phase = Phase::NeedsAddress;
        state.remote.session = Some(session);
        return crate::app::reset_list_scroll();
    }
    let generation = state.remote.guard.current();
    let token = CancellationToken::default();
    state.remote.token = Some(token.clone());
    state.remote.session = Some(session);
    Task::batch([
        crate::app::reset_list_scroll(),
        Task::perform(
            services.search_plugin(plugin, text, 1, None, token),
            move |result| Msg::Found { generation, result }.into(),
        ),
    ])
}

fn found(
    state: &mut State,
    generation: u64,
    result: Result<RemotePage, SourceError>,
    services: &dyn Services,
) -> Task<Message> {
    if !state.remote.guard.accepts(generation) {
        return Task::none();
    }
    state.remote.token = None;
    let Some(session) = state.remote.session.as_mut() else {
        return Task::none();
    };
    if !matches!(session.phase, Phase::Loading) {
        return Task::none();
    }
    match result {
        Err(error) if error.kind == SourceErrorKind::Cancelled => return Task::none(),
        Err(error) => session.phase = Phase::Failed(error),
        Ok(page) if page.hits.is_empty() => session.phase = Phase::Empty,
        Ok(page) => {
            session.dropped = page.dropped;
            session.next = page.next_cursor;
            session.hits = page.hits;
            session.phase = Phase::Results;
            // Results are there: the download icons need to know whether Calibre is.
            return Task::batch([
                crate::downloads::ensure_calibre(state, services),
                request_covers(state, services),
            ]);
        }
    }
    request_covers(state, services)
}

fn show_more(state: &mut State, services: &dyn Services) -> Task<Message> {
    let Some(plugin) = active_id(state) else {
        return Task::none();
    };
    let generation = state.remote.guard.current();
    let Some(session) = state.remote.session.as_mut() else {
        return Task::none();
    };
    if !matches!(session.phase, Phase::Results) || matches!(session.more, More::Loading) {
        return Task::none();
    }
    let Some(cursor) = session.next.clone() else {
        return Task::none();
    };
    session.more = More::Loading;
    let (text, page) = (session.query.clone(), session.page + 1);
    let token = CancellationToken::default();
    state.remote.token = Some(token.clone());
    Task::perform(
        services.search_plugin(plugin, text, page, Some(cursor), token),
        move |result| Msg::MoreFound { generation, result }.into(),
    )
}

fn more_found(
    state: &mut State,
    generation: u64,
    result: Result<RemotePage, SourceError>,
    services: &dyn Services,
) -> Task<Message> {
    if !state.remote.guard.accepts(generation) {
        return Task::none();
    }
    state.remote.token = None;
    let Some(session) = state.remote.session.as_mut() else {
        return Task::none();
    };
    if !matches!(session.more, More::Loading) {
        return Task::none();
    }
    match result {
        Err(error) if error.kind == SourceErrorKind::Cancelled => session.more = More::Ready,
        Err(error) => session.more = More::Failed(error),
        Ok(page) => {
            // The ids are unique within a page, not across pages: a repeat is left out.
            let known: BTreeSet<String> = session.hits.iter().map(|h| h.id.clone()).collect();
            session
                .hits
                .extend(page.hits.into_iter().filter(|h| !known.contains(&h.id)));
            session.dropped += page.dropped;
            session.next = page.next_cursor;
            session.page += 1;
            session.more = More::Ready;
        }
    }
    request_covers(state, services)
}

fn select(state: &mut State, id: &str, force: bool, services: &dyn Services) -> Task<Message> {
    let Some(plugin) = active_id(state) else {
        return Task::none();
    };
    let Some(listed) = state
        .remote
        .session
        .as_ref()
        .and_then(|s| s.hits.iter().find(|h| h.id == id))
        .cloned()
    else {
        return Task::none();
    };
    if !force && state.remote.selected.as_deref() == Some(id) {
        return Task::none();
    }
    state.remote.selected = Some(id.to_owned());
    state.remote.cancel_details();
    // A result is selected to be looked at: the pane that shows it opens (the mockup's rule).
    let mut tasks = Vec::new();
    if !state.visible_settings().details_visible {
        let mut settings = state.visible_settings().clone();
        settings.details_visible = true;
        tasks.push(crate::app::update(
            state,
            Message::SaveSettings(settings),
            services,
        ));
    }
    let known = !force
        && state
            .remote
            .cache
            .get(&(plugin.clone(), id.to_owned()))
            .is_some_and(|cached| cached.title == listed.title);
    if known {
        state.remote.details = Some(DetailsPhase::Ready);
    } else {
        state.remote.details = Some(DetailsPhase::Loading);
        let token = CancellationToken::default();
        state.remote.details_token = Some(token.clone());
        let generation = state.remote.details_guard.current();
        let owner = (plugin.clone(), id.to_owned());
        tasks.push(Task::perform(
            services.plugin_details(plugin, listed, token),
            move |result| {
                Msg::DetailsLoaded {
                    generation,
                    plugin: owner.0.clone(),
                    id: owner.1.clone(),
                    result: result.map(Box::new),
                }
                .into()
            },
        ));
    }
    tasks.push(request_covers(state, services));
    Task::batch(tasks)
}

fn details_loaded(
    state: &mut State,
    generation: u64,
    plugin: String,
    id: String,
    result: Result<Box<RemoteHit>, SourceError>,
    services: &dyn Services,
) -> Task<Message> {
    if !state.remote.details_guard.accepts(generation)
        || state.remote.selected.as_deref() != Some(id.as_str())
    {
        return Task::none();
    }
    state.remote.details_token = None;
    match result {
        Err(error) if error.kind == SourceErrorKind::Cancelled => return Task::none(),
        Err(error) => state.remote.details = Some(DetailsPhase::Failed(error)),
        Ok(hit) => {
            if state.remote.cache.len() >= DETAILS_CAPACITY {
                state.remote.cache.clear();
            }
            state.remote.cache.insert((plugin, id), *hit);
            state.remote.details = Some(DetailsPhase::Ready);
        }
    }
    request_covers(state, services)
}

// ---- covers -----------------------------------------------------------------------------------

/// The addresses of the covers to have now: the selected result's, then those of the rows
/// that are in view and a screenful around them.
fn wanted_covers(state: &State) -> Vec<String> {
    let mut wanted = Vec::new();
    if let Some(url) = state
        .remote
        .selected_hit()
        .and_then(|h| h.cover_url.clone())
    {
        wanted.push(url);
    }
    let Some(session) = state.remote.session.as_ref() else {
        return wanted;
    };
    let count = session.hits.len();
    let range = if state.remote.grid {
        crate::widgets::book_grid::Geometry::for_state(state).book_range(
            count,
            state.viewport_width,
            state.list_offset,
            state.list_height * 2.0,
        )
    } else {
        crate::catalog::visible_slice_with_height(
            count,
            state.list_offset,
            state.list_height * 2.0,
            state.row_height(),
        )
    };
    for hit in &session.hits[range] {
        if let Some(url) = &hit.cover_url
            && !wanted.contains(url)
        {
            wanted.push(url.clone());
        }
    }
    wanted
}

/// Starts the covers that are wanted and not here yet, up to [`COVER_CONCURRENCY`] at a time.
/// Called again whenever one ends, a list is scrolled or a result is selected.
pub fn request_covers(state: &mut State, services: &dyn Services) -> Task<Message> {
    let Some(plugin) = active_id(state) else {
        return Task::none();
    };
    let wanted = wanted_covers(state);
    let urls = state.remote.covers.next_requests(&wanted);
    let generation = state.remote.guard.current();
    let token = state.remote.covers.token.clone();
    Task::batch(urls.into_iter().map(|url| {
        let owner = url.clone();
        Task::perform(
            services.load_remote_cover(plugin.clone(), url, token.clone()),
            move |result| {
                Msg::CoverLoaded {
                    url: owner.clone(),
                    generation,
                    result,
                }
                .into()
            },
        )
    }))
}

// ---- the plugin list changed ------------------------------------------------------------------

/// After the list of plugins was read: a source that is gone, off or not compatible is dropped
/// (back to the library, and remembered as such), and what depends on a plugin that changed is
/// forgotten.
pub fn reconcile(state: &mut State, services: &dyn Services) -> Task<Message> {
    let seen: Vec<(String, String, bool)> = state
        .plugins
        .rows
        .iter()
        .map(|r| (r.id.clone(), r.version.clone(), usable(r)))
        .collect();
    if seen != state.remote.plugins_seen {
        // A new version may return other details, and a plugin that was off has no session.
        state.remote.cache.clear();
        state.remote.plugins_seen = seen;
    }
    if state.visible_settings().search_source.is_some() && active(state).is_none() {
        state.remote.reset();
        let mut settings = state.visible_settings().clone();
        settings.search_source = None;
        return crate::app::update(state, Message::SaveSettings(settings), services);
    }
    // An address that was missing has been entered since.
    let needs = active(state).is_some_and(PluginRow::needs_setup);
    if !needs
        && let Some(session) = state.remote.session.as_mut()
        && matches!(session.phase, Phase::NeedsAddress)
    {
        session.phase = Phase::Idle(None);
    }
    Task::none()
}

// ---- keyboard ---------------------------------------------------------------------------------

/// Arrow keys, Home, End and the page keys in the results: the selection moves and the
/// details of the new one load. A first press selects the first result.
pub fn key_move(state: &mut State, motion: Motion, services: &dyn Services) -> Task<Message> {
    let Some(session) = state.remote.session.as_ref() else {
        return Task::none();
    };
    let ids: Vec<String> = session.hits.iter().map(|h| h.id.clone()).collect();
    if ids.is_empty() {
        return Task::none();
    }
    let (grid, columns, row_height) = crate::app::layout_metrics(state);
    let page = ((state.list_height / row_height).floor() as isize).max(1) * columns as isize;
    let step = if grid { columns as isize } else { 1 };
    let delta = match motion {
        Motion::Up => -step,
        Motion::Down => step,
        Motion::Left if grid => -1,
        Motion::Right if grid => 1,
        Motion::PageUp => -page,
        Motion::PageDown => page,
        Motion::Home | Motion::End => 0,
        Motion::Left | Motion::Right => return Task::none(),
    };
    let current = state
        .remote
        .selected
        .as_ref()
        .and_then(|id| ids.iter().position(|i| i == id));
    let last = ids.len() - 1;
    let target = match (motion, current) {
        (Motion::Home, _) | (_, None) => 0,
        (Motion::End, _) => last,
        (_, Some(at)) => (at as isize + delta).clamp(0, last as isize) as usize,
    };
    if current == Some(target) {
        return Task::none();
    }
    state.keyboard_focus = true;
    Task::batch([
        select(state, &ids[target], false, services),
        crate::app::scroll_to_index(state, target),
    ])
}

/// Enter in the list (outside the text field): select the first result, or show the pane.
pub fn key_open(state: &mut State, services: &dyn Services) -> Task<Message> {
    if state.remote.selected.is_none() {
        return key_move(state, Motion::Home, services);
    }
    if !state.visible_settings().details_visible {
        let mut settings = state.visible_settings().clone();
        settings.details_visible = true;
        return crate::app::update(state, Message::SaveSettings(settings), services);
    }
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_requests_are_limited_and_never_repeated() {
        let mut covers = RemoteCovers::default();
        let wanted: Vec<String> = (0..5).map(|i| format!("https://x.test/{i}.jpg")).collect();
        let first = covers.next_requests(&wanted);
        assert_eq!(first, wanted[..COVER_CONCURRENCY]);
        assert!(covers.next_requests(&wanted).is_empty(), "no room left");
        covers.running.remove(&wanted[0]);
        covers.insert(wanted[0].clone(), None);
        // The one that failed is not asked for again; the next in line is.
        assert_eq!(covers.next_requests(&wanted), [wanted[2].clone()]);
        assert!(covers.get(&wanted[0]).is_none());
        assert_eq!(covers.len(), 1);
    }

    #[test]
    fn the_oldest_covers_are_dropped_beyond_the_capacity() {
        let mut covers = RemoteCovers::default();
        for i in 0..COVER_CAPACITY + 10 {
            covers.insert(format!("u{i}"), None);
        }
        assert_eq!(covers.len(), COVER_CAPACITY);
        assert!(!covers.entries.contains_key("u0"));
        assert!(covers.entries.contains_key("u209"));
    }

    #[test]
    fn a_result_shows_the_details_that_arrived_for_it() {
        let mut state = RemoteState::default();
        let listed = RemoteHit::titled("a", "Title");
        state.session = Some(Session {
            plugin: "p".into(),
            query: "q".into(),
            phase: Phase::Results,
            hits: vec![listed.clone()],
            next: None,
            page: 1,
            more: More::Ready,
            dropped: 0,
        });
        assert_eq!(state.hit("a").unwrap().description, None);
        let mut detailed = listed.clone();
        detailed.description = Some("More".into());
        state.cache.insert(("p".into(), "a".into()), detailed);
        assert_eq!(state.hit("a").unwrap().description.as_deref(), Some("More"));
        // The same id with another title is another book: the cache does not apply.
        state.session.as_mut().unwrap().hits[0].title = "Other".into();
        assert_eq!(state.hit("a").unwrap().description, None);
    }
}
