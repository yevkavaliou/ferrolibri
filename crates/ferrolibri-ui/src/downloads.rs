//! Downloading a book that a plugin found and adding it to the library (plan item 13, design
//! gap G6): what the download icon shows for each offer, the format menu, and the job that
//! `ferrolibri_plugins::DownloadAndAdd` runs.
//!
//! The icon is not stored: it is worked out from the job list each time it is drawn
//! ([`offer_icon`]), so it cannot disagree with Jobs. What is stored is which job belongs to
//! which offer, and how a finished one ended. The service builds the job
//! (`Services::prepare_download`: it loads the plugin and checks Calibre and the library); the
//! queue is the usual one (`Services::enqueue`), so the executor serialises writes to a library.
//!
//! A book that the library already seems to have (same author and title, compared without case,
//! punctuation or word order in the name) is only marked, with the icon of an existing book;
//! clicking it downloads as usual.
use crate::{
    app::{Message, State},
    remote,
    services::{
        Services,
        jobs::JobId,
        plugins::{DownloadError, DownloadRequest, RemoteHit, RemoteOffer},
    },
    widgets::toasts,
};
use ferrolibri_core::{
    calibre::AddOutcome,
    jobs::{ExternalJob, JobOutput, JobRequest, JobState, Stage},
};
use iced::{Point, Task};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

/// Words of the mockup, shared by the toast and the tooltip.
pub const NEEDS_CALIBRE: &str = "Calibre is needed to add downloaded books";
const NEEDS_CALIBRE_TOAST: &str =
    "Calibre is needed to add downloaded books. Install Calibre, then try again.";

/// One offer of one result of one plugin.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub plugin: String,
    pub hit: String,
    pub offer: String,
}

/// The states of the download icon (the mockup's `idle`, `nocalibre`, `none`, `queued`, `get`,
/// `adding`, `done`, `exists`, `failed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Idle,
    NeedsCalibre,
    NoFile,
    Queued,
    /// The share received, when the size is known; a spinner when it is not.
    Downloading(Option<u8>),
    Adding,
    Done,
    Exists,
    Failed,
}

impl Icon {
    /// The icon of a result that has several offers is the busiest of theirs.
    fn rank(self) -> u8 {
        match self {
            Icon::Downloading(_) | Icon::Adding => 5,
            Icon::Queued => 4,
            Icon::Failed => 3,
            Icon::Done | Icon::Exists => 2,
            Icon::Idle | Icon::NeedsCalibre | Icon::NoFile => 0,
        }
    }
    /// Something is going on: the clock has to run so that the spinner turns.
    pub fn is_busy(self) -> bool {
        matches!(self, Icon::Queued | Icon::Downloading(_) | Icon::Adding)
    }
    /// The tooltip, the same in lists, grids and the details pane.
    pub fn tip(self) -> String {
        match self {
            Icon::Idle => "Download and add to your library".into(),
            Icon::NeedsCalibre => NEEDS_CALIBRE.into(),
            Icon::NoFile => "This result has no downloadable file".into(),
            Icon::Queued => "Waiting in the queue".into(),
            Icon::Downloading(None) => "Downloading…".into(),
            Icon::Downloading(Some(percent)) => format!("Downloading {percent}%"),
            Icon::Adding => "Adding to your library…".into(),
            Icon::Done => "Added to your library".into(),
            Icon::Exists => "Already in your library".into(),
            Icon::Failed => "Download failed — click to try again".into(),
        }
    }
    /// The label next to the icon in the details pane.
    pub fn label(self) -> String {
        match self {
            Icon::Idle => "Download".into(),
            Icon::NeedsCalibre => "Needs Calibre".into(),
            Icon::NoFile => "No file".into(),
            Icon::Queued => "Waiting".into(),
            Icon::Downloading(None) => "Downloading".into(),
            Icon::Downloading(Some(percent)) => format!("{percent}%"),
            Icon::Adding => "Adding…".into(),
            Icon::Done => "Added".into(),
            Icon::Exists => "In library".into(),
            Icon::Failed => "Try again".into(),
        }
    }
}

#[derive(Clone, Debug)]
enum Slot {
    /// The service is building the job.
    Preparing,
    Job(JobId),
    Done,
    Exists,
    Failed,
}

/// What Jobs says about a download besides the job itself.
#[derive(Clone, Debug)]
struct Info {
    plugin: String,
    /// The result's title and authors: to find the book again once it is in the library.
    title: String,
    authors: Vec<String>,
}

/// The "Download as" menu of a result with several offers, opened at the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub hit: String,
    pub at: Point,
}

#[derive(Default)]
pub struct DownloadsState {
    slots: BTreeMap<Key, Slot>,
    keys: BTreeMap<JobId, Key>,
    infos: BTreeMap<JobId, Info>,
    /// Author and title of every book in the library, normalised (see [`book_key`]).
    library: HashSet<String>,
    /// Calibre is installed; `None` until it has been looked for.
    pub calibre: Option<bool>,
    checking: bool,
    pub menu: Option<Menu>,
    /// The book that was added last, to select once the library has been scanned again.
    highlight: Option<Vec<String>>,
    /// Turns the spinners; moves with the clock while a download is going.
    spin: u32,
}

impl DownloadsState {
    pub fn spin(&self) -> u32 {
        self.spin
    }
    /// The name of the plugin a download job came from.
    pub fn plugin_of(&self, job: JobId) -> Option<&str> {
        self.infos.get(&job).map(|info| info.plugin.as_str())
    }
    /// A job that left the history of finished jobs is forgotten here too.
    pub fn forget(&mut self, job: JobId) {
        self.infos.remove(&job);
        self.keys.remove(&job);
    }
    fn busy(&self) -> bool {
        self.slots
            .values()
            .any(|slot| matches!(slot, Slot::Preparing | Slot::Job(_)))
    }
}

// ---- which book is in the library -------------------------------------------------------------

/// Lower-case words of `text`, letters and digits only.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The words of an author's name in a fixed order, so that "Austen, Jane" is "Jane Austen".
fn author_words(author: &str) -> String {
    let mut words = words(author);
    words.sort();
    words.join(" ")
}

fn book_key(author: &str, title: &str) -> String {
    format!("{}\u{1}{}", author_words(author), words(title).join(" "))
}

/// Calibre writes several authors into one folder name ("A & B"): each counts.
fn author_parts(author: &str) -> impl Iterator<Item = &str> {
    author
        .split(['&', ';'])
        .map(str::trim)
        .filter(|a| !a.is_empty())
}

/// Reads the library again: called whenever a scan has produced a new catalog.
pub fn library_changed(state: &mut State) {
    let mut keys = HashSet::with_capacity(state.books.len() * 2);
    for book in &state.books {
        keys.insert(book_key(&book.author, &book.title));
        for part in author_parts(&book.author) {
            keys.insert(book_key(part, &book.title));
        }
    }
    state.downloads.library = keys;
}

/// The library has the book that `hit` describes, as far as author and title tell.
fn in_library(state: &State, hit: &RemoteHit) -> bool {
    !state.downloads.library.is_empty()
        && hit.authors.iter().any(|author| {
            state
                .downloads
                .library
                .contains(&book_key(author, &hit.title))
        })
}

/// After a rescan: selects the book that was just added, if the library shows it.
pub fn select_added(state: &mut State) {
    let Some(authors) = state.downloads.highlight.take() else {
        return;
    };
    // `highlight` is `[title, author…]`.
    let Some((title, authors)) = authors.split_first() else {
        return;
    };
    let wanted: HashSet<String> = authors.iter().map(|a| book_key(a, title)).collect();
    let found = state.catalog.entries.iter().find(|entry| {
        wanted.contains(&book_key(&entry.book.author, &entry.book.title))
            || author_parts(&entry.book.author)
                .any(|part| wanted.contains(&book_key(part, &entry.book.title)))
    });
    if let Some(id) = found.map(|entry| entry.book.id.clone()) {
        let order = state.catalog.order.clone();
        state
            .selection
            .click(id, iced::keyboard::Modifiers::default(), &order);
    }
}

// ---- the icon ---------------------------------------------------------------------------------

fn key_of(state: &State, hit: &RemoteHit, offer: &RemoteOffer) -> Option<Key> {
    Some(Key {
        plugin: remote::active_id(state)?,
        hit: hit.id.clone(),
        offer: offer.id.clone(),
    })
}

fn from_job(state: &State, job: JobId) -> Icon {
    let Some(view) = state.jobs.get(&job) else {
        return Icon::Queued;
    };
    match view.state {
        JobState::Waiting => Icon::Queued,
        JobState::Running => match view.stage {
            Some(Stage::Adding) => Icon::Adding,
            _ => Icon::Downloading(crate::widgets::jobs::percentage(view).map(|p| p.round() as u8)),
        },
        JobState::Succeeded => Icon::Done,
        JobState::Failed => Icon::Failed,
        JobState::Cancelled => Icon::Idle,
    }
}

/// The icon of one offer.
pub fn offer_icon(state: &State, hit: &RemoteHit, offer: &RemoteOffer) -> Icon {
    let slot = key_of(state, hit, offer).and_then(|key| state.downloads.slots.get(&key));
    match slot {
        Some(Slot::Preparing) => Icon::Queued,
        Some(Slot::Job(job)) => from_job(state, *job),
        Some(Slot::Done) => Icon::Done,
        Some(Slot::Exists) => Icon::Exists,
        Some(Slot::Failed) => Icon::Failed,
        None if in_library(state, hit) => Icon::Exists,
        None if state.downloads.calibre == Some(false) => Icon::NeedsCalibre,
        None => Icon::Idle,
    }
}

/// The icon of a result: the busiest of its offers' icons.
pub fn hit_icon(state: &State, hit: &RemoteHit) -> Icon {
    hit.offers
        .iter()
        .map(|offer| offer_icon(state, hit, offer))
        .reduce(|best, next| {
            if next.rank() > best.rank() {
                next
            } else {
                best
            }
        })
        .unwrap_or(Icon::NoFile)
}

/// A download can be started: it was never started, or it failed.
pub fn can_start(state: &State, hit: &RemoteHit, offer: &RemoteOffer) -> bool {
    match key_of(state, hit, offer).and_then(|key| state.downloads.slots.get(&key)) {
        None | Some(Slot::Failed) => true,
        Some(Slot::Preparing | Slot::Job(_) | Slot::Done | Slot::Exists) => false,
    }
}

/// Something is downloading, so the clock keeps running for the spinner.
pub fn animating(state: &State) -> bool {
    state.downloads.busy()
}

pub fn tick(state: &mut State) {
    if state.downloads.busy() {
        state.downloads.spin = state.downloads.spin.wrapping_add(1);
    }
}

// ---- messages ---------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Msg {
    /// The icon was pressed: of a result (`offer` is `None`) or of one of its offers.
    Press {
        hit: String,
        offer: Option<String>,
    },
    CloseMenu,
    Prepared {
        key: Key,
        title: String,
        info_plugin: String,
        authors: Vec<String>,
        result: Result<Arc<dyn ExternalJob>, DownloadError>,
    },
    /// "Check again" on the Calibre card.
    CheckCalibre,
    CalibreChecked {
        found: bool,
        asked_again: bool,
    },
}

impl From<Msg> for Message {
    fn from(msg: Msg) -> Self {
        Message::Download(msg)
    }
}

/// Looks for Calibre once, when the first results of a plugin appear.
pub fn ensure_calibre(state: &mut State, services: &dyn Services) -> Task<Message> {
    if state.downloads.calibre.is_some() || state.downloads.checking {
        return Task::none();
    }
    ask_calibre(state, services, false)
}

fn ask_calibre(state: &mut State, services: &dyn Services, again: bool) -> Task<Message> {
    state.downloads.checking = true;
    Task::perform(services.calibre_available(again), move |result| {
        Msg::CalibreChecked {
            // A check that fails to run is treated as "not found": the card says what to do.
            found: result.unwrap_or(false),
            asked_again: again,
        }
        .into()
    })
}

pub fn update(state: &mut State, msg: Msg, services: &dyn Services) -> Task<Message> {
    if state.closing {
        return Task::none();
    }
    match msg {
        Msg::Press { hit, offer } => press(state, &hit, offer.as_deref(), services),
        Msg::CloseMenu => {
            state.downloads.menu = None;
            Task::none()
        }
        Msg::CheckCalibre => ask_calibre(state, services, true),
        Msg::CalibreChecked { found, asked_again } => {
            state.downloads.checking = false;
            state.downloads.calibre = Some(found);
            if asked_again && !found {
                toasts::push(
                    state,
                    "Calibre was still not found. Install it, then try again.".into(),
                    true,
                );
            }
            Task::none()
        }
        Msg::Prepared {
            key,
            title,
            info_plugin,
            authors,
            result,
        } => prepared(state, key, title, info_plugin, authors, result, services),
    }
}

fn press(
    state: &mut State,
    hit_id: &str,
    offer_id: Option<&str>,
    services: &dyn Services,
) -> Task<Message> {
    let Some(hit) = state.remote.hit(hit_id).cloned() else {
        return Task::none();
    };
    if let Some(offer) = offer_id {
        return match hit.offers.iter().find(|o| o.id == offer) {
            Some(offer) => start(state, &hit, offer, services),
            None => Task::none(),
        };
    }
    match hit.offers.as_slice() {
        [] => Task::none(),
        [offer] => start(state, &hit, offer, services),
        _ => {
            if hit_icon(state, &hit) == Icon::NeedsCalibre {
                toasts::push(state, NEEDS_CALIBRE_TOAST.into(), true);
                return Task::none();
            }
            let same = state
                .downloads
                .menu
                .as_ref()
                .is_some_and(|m| m.hit == hit.id);
            state.downloads.menu = (!same).then(|| Menu {
                hit: hit.id.clone(),
                at: Point::new(state.pointer_x, state.pointer_y),
            });
            Task::none()
        }
    }
}

/// The label of an offer in a job's title: `EPUB`, `FB2 zip`.
pub fn offer_label(offer: &RemoteOffer) -> String {
    if offer.zip {
        format!("{} zip", offer.format)
    } else {
        offer.format.clone()
    }
}

fn start(
    state: &mut State,
    hit: &RemoteHit,
    offer: &RemoteOffer,
    services: &dyn Services,
) -> Task<Message> {
    state.downloads.menu = None;
    if !can_start(state, hit, offer) {
        return Task::none();
    }
    let Some(key) = key_of(state, hit, offer) else {
        return Task::none();
    };
    if state.downloads.calibre == Some(false) {
        toasts::push(state, NEEDS_CALIBRE_TOAST.into(), true);
        return Task::none();
    }
    let Some(library) = state.library.clone() else {
        toasts::push(
            state,
            "Open a library before downloading a book.".into(),
            true,
        );
        return Task::none();
    };
    let Some(handle) = hit.handle.clone() else {
        toasts::push(
            state,
            "This result cannot be downloaded: the plugin did not describe it fully.".into(),
            true,
        );
        return Task::none();
    };
    state.downloads.slots.insert(key.clone(), Slot::Preparing);
    let request = DownloadRequest {
        plugin: key.plugin.clone(),
        hit: handle,
        offer: offer.id.clone(),
        library,
        export_dir: state.visible_settings().export_folder.clone(),
    };
    let plugin_name = state
        .plugins
        .row(&key.plugin)
        .map_or_else(|| key.plugin.clone(), |row| row.name.clone());
    let title = format!("Download · {} ({})", hit.title, offer_label(offer));
    let authors = hit.authors.clone();
    let book = hit.title.clone();
    Task::perform(services.prepare_download(request), move |result| {
        Msg::Prepared {
            key: key.clone(),
            title: title.clone(),
            info_plugin: plugin_name.clone(),
            authors: std::iter::once(book.clone())
                .chain(authors.clone())
                .collect(),
            result,
        }
        .into()
    })
}

fn prepared(
    state: &mut State,
    key: Key,
    title: String,
    plugin_name: String,
    title_and_authors: Vec<String>,
    result: Result<Arc<dyn ExternalJob>, DownloadError>,
    services: &dyn Services,
) -> Task<Message> {
    // Something else may have replaced the entry meanwhile (the plugin was removed): only a
    // download that is still being prepared goes on.
    if !matches!(state.downloads.slots.get(&key), Some(Slot::Preparing)) {
        return Task::none();
    }
    match result {
        Ok(job) => {
            let id = services.enqueue(title, JobRequest::External(job));
            let mut parts = title_and_authors.into_iter();
            let book = parts.next().unwrap_or_default();
            state.downloads.slots.insert(key.clone(), Slot::Job(id));
            state.downloads.keys.insert(id, key);
            state.downloads.infos.insert(
                id,
                Info {
                    plugin: plugin_name,
                    title: book,
                    authors: parts.collect(),
                },
            );
            // A job that was refused is finished already; do not wait for the clock to say so.
            crate::app::poll_now(state, services);
        }
        Err(DownloadError::NeedsCalibre) => {
            state.downloads.slots.remove(&key);
            state.downloads.calibre = Some(false);
            toasts::push(state, NEEDS_CALIBRE_TOAST.into(), true);
        }
        Err(DownloadError::Failed(message)) => {
            state.downloads.slots.insert(key, Slot::Failed);
            toasts::push(state, message, true);
        }
    }
    Task::none()
}

/// A download job ended (called from the job poll, once per job). Sets the icon's final state,
/// the line Jobs shows, and tells the user.
pub fn finished(state: &mut State, id: JobId, result: &Result<JobOutput, String>, cancelled: bool) {
    let Some(key) = state.downloads.keys.get(&id).cloned() else {
        return;
    };
    let Some(info) = state.downloads.infos.get(&id).cloned() else {
        return;
    };
    match result {
        Ok(JobOutput::Added(book)) => match book.outcome {
            AddOutcome::Added { .. } => {
                state.downloads.slots.insert(key, Slot::Done);
                state
                    .job_messages
                    .insert(id, "Added to your library · rescanning".into());
                toasts::push(
                    state,
                    format!("Added to your library: {}", book.title),
                    false,
                );
                let mut remember = vec![info.title];
                remember.extend(info.authors);
                state.downloads.highlight = Some(remember);
            }
            AddOutcome::AlreadyExists => {
                state.downloads.slots.insert(key, Slot::Exists);
                state
                    .job_messages
                    .insert(id, "Already in your library · not added again".into());
                toasts::push(
                    state,
                    format!("Already in your library: {}", book.title),
                    false,
                );
            }
        },
        Err(_) if cancelled => {
            // Cancelling puts the icon back: the book can be asked for again.
            state.downloads.slots.remove(&key);
            let title = state
                .jobs
                .get(&id)
                .map_or_else(|| info.title.clone(), |job| job.title.clone());
            toasts::push(state, format!("Cancelled · {title}"), false);
        }
        Err(message) => {
            state.downloads.slots.insert(key, Slot::Failed);
            toasts::push(state, message.clone(), true);
        }
        Ok(_) => {
            state.downloads.slots.insert(key, Slot::Failed);
            toasts::push(
                state,
                "The download returned no book. Try again.".into(),
                true,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_and_title_match_without_case_punctuation_or_name_order() {
        assert_eq!(
            book_key("Austen, Jane", "Pride & Prejudice"),
            book_key("jane AUSTEN", "Pride  &  Prejudice")
        );
        assert_eq!(
            book_key("Austen, Jane", "Emma!"),
            book_key("Jane Austen", "emma")
        );
        assert_ne!(
            book_key("Jane Austen", "Emma"),
            book_key("Jane Austen", "Persuasion")
        );
        assert_ne!(
            book_key("Jane Austen", "Emma"),
            book_key("Emma Austen", "Jane")
        );
    }

    #[test]
    fn several_authors_in_one_folder_name_are_told_apart() {
        let parts: Vec<_> = author_parts("Ilf & Petrov; Someone Else").collect();
        assert_eq!(parts, ["Ilf", "Petrov", "Someone Else"]);
    }

    #[test]
    fn a_result_with_several_offers_shows_its_busiest_state() {
        let ranks = [
            Icon::Idle,
            Icon::Done,
            Icon::Failed,
            Icon::Queued,
            Icon::Adding,
        ];
        let best = ranks
            .into_iter()
            .reduce(|best, next| {
                if next.rank() > best.rank() {
                    next
                } else {
                    best
                }
            })
            .unwrap();
        assert_eq!(best, Icon::Adding);
        assert!(Icon::Downloading(None).is_busy());
        assert!(!Icon::Done.is_busy());
    }
}
