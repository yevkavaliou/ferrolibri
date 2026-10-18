//! Plugin management as the interface sees it: plain data in, plain data out.
//!
//! The views never touch `ferrolibri-plugins` types, so a test can describe a plugin, a
//! release or a failure in a few lines. [`PluginBackend`] is the real thing: it reads the
//! store, asks GitHub or a web address for a plugin, and installs what the user confirmed.
use ferrolibri_core::{
    ErrorKind,
    calibre::BookAdder,
    config::{self, LibraryPathState},
    converter::CancellationToken,
    jobs::ExternalJob,
};
use ferrolibri_plugins::{
    Downgrade, DownloadAndAdd, Http, Identity, Inspection, InstallChange, InstallSource,
    InstalledPlugin, LoadedPlugin, Manifest, ManifestError, PluginFunction, PluginRuntime,
    PluginStatus, PluginStore, Release, ReleaseCheck, ReleaseOptions, RemoteError, RemoteInstaller,
    RemoteSource, SUPPORTED_API_VERSIONS, SearchHit, SearchQuery, SettingKind, SettingValue,
    parse_search_page,
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

/// How many releases the install dialog lists (the design shows the latest five).
pub const RELEASES_SHOWN: usize = 5;
/// The time budget most plugins need; a larger one is pointed out to the user.
pub const USUAL_CALL_TIMEOUT_S: u32 = 30;

pub type PluginFuture<T> = Pin<Box<dyn Future<Output = Result<T, Problem>> + Send>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    Failed,
    /// GitHub's hourly limit: trying again at once would not help.
    RateLimited,
    /// The download is not the file that was announced.
    Checksum,
    /// The user cancelled; nothing to show.
    Cancelled,
}

/// A failure of checking or installing, phrased for the install dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub title: String,
    pub message: String,
}

impl Problem {
    pub fn failed(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: ProblemKind::Failed,
            title: title.into(),
            message: message.into(),
        }
    }
    pub fn unavailable() -> Self {
        Self::failed(
            "Plugins are unavailable",
            "This build cannot manage plugins.",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Ready,
    /// The plugin's files are fine but this FerroLibri cannot run it; one phrase per reason.
    Incompatible(Vec<String>),
    /// The folder cannot be read at all; it can still be removed.
    Broken(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionSummary {
    /// `network.domains` as declared (https only).
    pub domains: Vec<String>,
    /// A setting holds an address whose host the plugin may reach once the user saves it.
    pub user_address: bool,
    pub call_timeout_s: u32,
    /// What the author says the plugin finds. Not checked.
    pub content: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Url,
    Bool,
    Choice(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    Text(String),
    Bool(bool),
}

/// One entry of a plugin's `[[settings]]`, with what the form starts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingField {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    /// `url` only: the address, once saved, opens the network for its host (D4).
    pub network: bool,
    /// What the form shows: the saved value, else the manifest's `default`. The default of an
    /// address that opens the network is not shown as a value (it only counts when the user
    /// saves it), so the field starts empty and the default is the placeholder.
    pub value: FieldValue,
    pub placeholder: Option<String>,
    /// The user has saved a value for it.
    pub saved: bool,
}

impl SettingField {
    /// An address that lets the plugin use the network still has to be entered.
    pub fn needs_address(&self) -> bool {
        self.network && self.kind == FieldKind::Url && !self.saved
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    pub permissions: PermissionSummary,
    pub settings: Vec<SettingField>,
    /// "GitHub · owner/repo · v1.2.0", "Folder · D:\Plugins\x", "Web address · https://…".
    pub source: String,
    pub installed: String,
    /// `search_hint` of the manifest: the line under the Search field ("Search by title").
    pub search_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub enabled: bool,
    pub health: Health,
    /// `None` for a plugin whose folder cannot be read.
    pub info: Option<PluginInfo>,
}

impl PluginRow {
    pub fn is_usable(&self) -> bool {
        self.health == Health::Ready
    }
    /// Turned on but missing an address that it needs.
    pub fn needs_setup(&self) -> bool {
        self.enabled
            && self.is_usable()
            && self
                .info
                .as_ref()
                .is_some_and(|i| i.settings.iter().any(SettingField::needs_address))
    }
}

/// What installing a candidate would do to the plugin that is installed now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    New,
    Reinstall { version: String },
    Upgrade { from: String, to: String },
    Downgrade { from: String, to: String },
}

impl From<&InstallChange> for Change {
    fn from(change: &InstallChange) -> Self {
        match change {
            InstallChange::New => Self::New,
            InstallChange::Reinstall { version } => Self::Reinstall {
                version: version.clone(),
            },
            InstallChange::Upgrade { from, to } => Self::Upgrade {
                from: from.clone(),
                to: to.clone(),
            },
            InstallChange::Downgrade { from, to } => Self::Downgrade {
                from: from.clone(),
                to: to.clone(),
            },
        }
    }
}

/// A plugin that was found and checked, waiting for the user's confirmation. Nothing is
/// installed yet.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub license: String,
    pub homepage: String,
    pub source: String,
    /// "1 (supported)".
    pub api: String,
    /// `plugin.toml` carries the SHA-256 of the script, and the script matched it.
    pub checksum_in_manifest: bool,
    pub compatible: bool,
    pub reasons: Vec<String>,
    pub permissions: PermissionSummary,
    pub change: Change,
    /// What the service needs to install it. Empty for the fakes of tests.
    pub handle: Option<Arc<Inspection>>,
}

#[derive(Clone, Debug)]
pub struct ReleaseView {
    pub tag: String,
    pub date: String,
    pub compatible: bool,
    /// Why it cannot be installed; empty for a compatible release.
    pub reason: String,
    pub handle: Option<Release>,
}

#[derive(Clone, Debug)]
pub struct Releases {
    /// `owner/name`.
    pub repo: String,
    pub items: Vec<ReleaseView>,
}

#[derive(Clone, Debug)]
pub enum Checked {
    Releases(Releases),
    Candidate(Box<Candidate>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceRequest {
    Folder(PathBuf),
    Address(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallOptions {
    /// A plugin that is new here is turned on (otherwise left off).
    pub turn_on: bool,
    /// The user confirmed replacing the plugin with an older version.
    pub allow_downgrade: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub change: Change,
    /// The plugin is new and was turned on.
    pub turned_on: bool,
}

// ---- Search sources (plan item 12) -----------------------------------------------------------

pub type SourceFuture<T> = Pin<Box<dyn Future<Output = Result<T, SourceError>> + Send>>;

/// Why a call of a search plugin failed, as far as the screen words it differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceErrorKind {
    /// `connect`, `timeout`, `dns` or `tls`: the address, or the network, is the problem.
    Unreachable,
    /// The site answered with a 5xx status.
    Server,
    /// The plugin threw, ran out of time, or broke the contract.
    Plugin,
    /// The call was cancelled; nothing to show.
    Cancelled,
    /// The plugin cannot run any more: removed, turned off, or not compatible.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError {
    pub kind: SourceErrorKind,
    /// The plugin's own words, without its name.
    pub message: String,
}

impl SourceError {
    pub fn new(kind: SourceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn plugin(message: impl Into<String>) -> Self {
        Self::new(SourceErrorKind::Plugin, message)
    }
    pub fn cancelled() -> Self {
        Self::new(SourceErrorKind::Cancelled, "")
    }
    pub fn unavailable() -> Self {
        Self::new(
            SourceErrorKind::Unavailable,
            "This plugin is not available. Turn it on in Settings → Plugins.",
        )
    }
}

/// One thing the source offers to download for a result. Downloading is item 13.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteOffer {
    pub id: String,
    /// `EPUB`, `FB2`, …
    pub format: String,
    /// `FB2` inside a zip.
    pub zip: bool,
    pub size: Option<u64>,
    pub label: Option<String>,
}

/// A search result in the shape the screens draw. Everything in it has been checked against the
/// contract by the service; `handle` is what the service needs to call the plugin about it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteHit {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub year: Option<i32>,
    pub isbn: Option<String>,
    pub publisher: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub tags: Vec<String>,
    pub cover_url: Option<String>,
    pub page_url: Option<String>,
    pub offers: Vec<RemoteOffer>,
    /// The checked result itself. Empty for the fakes of tests.
    pub handle: Option<Arc<SearchHit>>,
}

impl RemoteHit {
    /// A result with a title and nothing else, for tests and for sources that give little.
    pub fn titled(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            authors: Vec::new(),
            description: None,
            language: None,
            year: None,
            isbn: None,
            publisher: None,
            series: None,
            series_index: None,
            tags: Vec::new(),
            cover_url: None,
            page_url: None,
            offers: Vec::new(),
            handle: None,
        }
    }
}

impl From<SearchHit> for RemoteHit {
    fn from(hit: SearchHit) -> Self {
        Self {
            id: hit.id.clone(),
            title: hit.title.clone(),
            authors: hit.authors.clone(),
            description: hit.description.clone(),
            language: hit.language.clone(),
            year: hit.year,
            isbn: hit.isbn.clone(),
            publisher: hit.publisher.clone(),
            series: hit.series.clone(),
            series_index: hit.series_index,
            tags: hit.tags.clone(),
            cover_url: hit.cover_url.clone(),
            page_url: hit.page_url.clone(),
            offers: hit
                .offers
                .iter()
                .map(|offer| RemoteOffer {
                    id: offer.id.clone(),
                    format: offer.format.label().to_owned(),
                    zip: offer.archive.is_some(),
                    size: offer.size,
                    label: offer.label.clone(),
                })
                .collect(),
            handle: Some(Arc::new(hit)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemotePage {
    pub hits: Vec<RemoteHit>,
    /// What to pass back for the next page; `None` on the last one.
    pub next_cursor: Option<String>,
    /// Results the plugin sent that broke a limit and were left out (they are in the log).
    pub dropped: usize,
}

// ---- Downloads (plan item 13) ----------------------------------------------------------------

/// One offer of one result, to be downloaded into one library.
#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub plugin: String,
    /// The checked result the offer belongs to (`RemoteHit::handle`).
    pub hit: Arc<SearchHit>,
    /// `RemoteOffer::id`.
    pub offer: String,
    pub library: PathBuf,
    /// Where a downloaded file is kept if Calibre cannot add it.
    pub export_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// `calibredb` was not found: the icon becomes "Needs Calibre".
    NeedsCalibre,
    /// Anything else that stops the job before it is queued, in words for a toast.
    Failed(String),
}

pub type DownloadFuture =
    Pin<Box<dyn Future<Output = Result<Arc<dyn ExternalJob>, DownloadError>> + Send>>;

/// Reads the words of a plugin failure. `host.fetch` failures that nobody caught arrive as
/// "network error (kind): …"; a plugin that meets a bad status usually says "status 503".
/// Neither is structured in the contract, so this is a reading of the text, and a plugin that
/// words it otherwise is shown its own message.
pub fn classify_plugin_message(message: &str) -> SourceErrorKind {
    let lower = message.to_ascii_lowercase();
    if ["connect", "timeout", "dns", "tls"]
        .iter()
        .any(|kind| lower.contains(&format!("network error ({kind})")))
    {
        return SourceErrorKind::Unreachable;
    }
    let mut previous = "";
    for word in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        if matches!(previous, "status" | "http" | "code")
            && word.len() == 3
            && word.starts_with('5')
            && word.bytes().all(|b| b.is_ascii_digit())
        {
            return SourceErrorKind::Server;
        }
        if !word.is_empty() {
            previous = word;
        }
    }
    SourceErrorKind::Plugin
}

fn source_error(error: ferrolibri_core::Error) -> SourceError {
    match error.kind {
        ErrorKind::Cancelled => SourceError::cancelled(),
        ErrorKind::Plugin { message, .. } => {
            SourceError::new(classify_plugin_message(&message), message)
        }
        _ => SourceError::plugin(error.to_string()),
    }
}

/// `17 Oct 2026` for seconds since 1970-01-01 UTC.
pub fn format_date(secs: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Days since the epoch to a calendar date (proleptic Gregorian).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// `14 Oct 2026` for GitHub's `2026-10-14T09:30:00Z`; anything else is shown as it came.
pub fn release_date(published_at: Option<&str>) -> String {
    let Some(text) = published_at else {
        return String::new();
    };
    let parse = || -> Option<u64> {
        let mut parts = text.get(..10)?.split('-');
        let year: i64 = parts.next()?.parse().ok()?;
        let month: i64 = parts.next()?.parse().ok()?;
        let day: i64 = parts.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1970 {
            return None;
        }
        // Civil date to days since the epoch.
        let y = year - i64::from(month <= 2);
        let era = y.div_euclid(400);
        let year_of_era = y.rem_euclid(400);
        let mp = (month + 9) % 12;
        let day_of_year = (153 * mp + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let days = era * 146_097 + day_of_era - 719_468;
        u64::try_from(days).ok().map(|d| d * 86_400)
    };
    parse().map_or_else(|| text.to_owned(), format_date)
}

pub fn source_text(source: &InstallSource) -> String {
    match source {
        InstallSource::Local { path } => format!("Folder · {path}"),
        InstallSource::Github { repo, tag } => format!("GitHub · {repo} · {tag}"),
        InstallSource::Url { url } => format!("Web address · {url}"),
        _ => "Unknown source".into(),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn permission_summary(manifest: &Manifest) -> PermissionSummary {
    PermissionSummary {
        domains: manifest.network.domains.clone(),
        user_address: manifest
            .settings
            .iter()
            .any(|s| s.kind == SettingKind::Url && s.network),
        call_timeout_s: u32::try_from(manifest.call_timeout().as_secs())
            .unwrap_or(USUAL_CALL_TIMEOUT_S),
        content: manifest.content.clone(),
    }
}

fn setting_fields(
    manifest: &Manifest,
    saved: &std::collections::BTreeMap<String, SettingValue>,
) -> Vec<SettingField> {
    manifest
        .settings
        .iter()
        .map(|setting| {
            let stored = saved.get(&setting.key);
            let default_text = match &setting.default {
                Some(SettingValue::Text(t)) => Some(t.clone()),
                _ => None,
            };
            let kind = match setting.kind {
                SettingKind::String => FieldKind::Text,
                SettingKind::Url => FieldKind::Url,
                SettingKind::Bool => FieldKind::Bool,
                SettingKind::Choice => FieldKind::Choice(setting.options.clone()),
            };
            let opens_network = setting.network && setting.kind == SettingKind::Url;
            let value = match (setting.kind, stored) {
                (SettingKind::Bool, Some(SettingValue::Bool(b))) => FieldValue::Bool(*b),
                (SettingKind::Bool, _) => {
                    FieldValue::Bool(matches!(setting.default, Some(SettingValue::Bool(true))))
                }
                (_, Some(SettingValue::Text(t))) => FieldValue::Text(t.clone()),
                (SettingKind::Choice, _) => FieldValue::Text(
                    default_text
                        .clone()
                        .or_else(|| setting.options.first().cloned())
                        .unwrap_or_default(),
                ),
                (_, _) if opens_network => FieldValue::Text(String::new()),
                (_, _) => FieldValue::Text(default_text.clone().unwrap_or_default()),
            };
            SettingField {
                key: setting.key.clone(),
                label: setting.label.clone(),
                kind,
                network: opens_network,
                value,
                placeholder: if opens_network { default_text } else { None },
                saved: stored.is_some(),
            }
        })
        .collect()
}

fn reasons(compatibility: &ferrolibri_plugins::Compatibility) -> Vec<String> {
    compatibility
        .reasons()
        .iter()
        .map(|r| capitalised(&r.to_string()))
        .collect()
}

fn row(store: &PluginStore, plugin: InstalledPlugin) -> PluginRow {
    match plugin.status {
        PluginStatus::Ready {
            manifest,
            compatibility,
            install,
        } => {
            let saved = store.settings(&plugin.id).unwrap_or_default();
            PluginRow {
                id: plugin.id,
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                author: manifest
                    .author
                    .clone()
                    .unwrap_or_else(|| "unknown author".into()),
                description: manifest.description.clone().unwrap_or_default(),
                enabled: plugin.enabled,
                health: if compatibility.is_compatible() {
                    Health::Ready
                } else {
                    Health::Incompatible(reasons(&compatibility))
                },
                info: Some(PluginInfo {
                    permissions: permission_summary(&manifest),
                    settings: setting_fields(&manifest, &saved),
                    source: source_text(&install.source),
                    installed: format_date(install.installed_at),
                    search_hint: manifest.search_hint.clone(),
                }),
            }
        }
        PluginStatus::Broken { message } => PluginRow {
            name: plugin.id.clone(),
            id: plugin.id,
            version: String::new(),
            author: String::new(),
            description: String::new(),
            enabled: plugin.enabled,
            health: Health::Broken(message),
            info: None,
        },
    }
}

fn candidate(inspection: Inspection) -> Candidate {
    let manifest = &inspection.manifest;
    let supported = SUPPORTED_API_VERSIONS.contains(&manifest.api_version);
    Candidate {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        author: manifest.author.clone().unwrap_or_else(|| "Unknown".into()),
        license: manifest
            .license
            .clone()
            .unwrap_or_else(|| "Not stated".into()),
        homepage: manifest
            .homepage
            .clone()
            .unwrap_or_else(|| "Not stated".into()),
        source: source_text(&inspection.source),
        api: format!(
            "{} ({})",
            manifest.api_version,
            if supported {
                "supported"
            } else {
                "not supported"
            }
        ),
        checksum_in_manifest: manifest.sha256.is_some(),
        compatible: inspection.compatibility.is_compatible(),
        reasons: reasons(&inspection.compatibility),
        permissions: permission_summary(manifest),
        change: Change::from(&inspection.change),
        handle: Some(Arc::new(inspection)),
    }
}

fn release_view(release: Release) -> ReleaseView {
    let (compatible, reason) = match &release.check {
        ReleaseCheck::Ready { compatibility, .. } if compatibility.is_compatible() => {
            (true, String::new())
        }
        ReleaseCheck::Ready { compatibility, .. } => (false, reasons(compatibility).join("; ")),
        ReleaseCheck::Failed { message } => (false, capitalised(message)),
    };
    ReleaseView {
        tag: release.tag.clone(),
        date: release_date(release.published_at.as_deref()),
        compatible,
        reason,
        handle: Some(release),
    }
}

/// The step-1 error of a manifest the app cannot read at all.
fn unsupported_api(path: &Path) -> Option<String> {
    let manifest = path.join("plugin.toml");
    let meta = std::fs::symlink_metadata(&manifest).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    let text = std::fs::read_to_string(&manifest).ok()?;
    match Manifest::parse(&text) {
        Err(error @ ManifestError::UnsupportedApiVersion { .. }) => {
            Some(capitalised(&error.to_string()))
        }
        _ => None,
    }
}

fn folder_problem(path: &Path, error: ferrolibri_core::Error) -> Problem {
    if let Some(reason) = unsupported_api(path) {
        return Problem::failed(
            "This plugin cannot be installed",
            format!(
                "{reason}. Look for a release that supports plugin API {}, or ask the author for one.",
                SUPPORTED_API_VERSIONS
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    Problem::failed(
        "This folder is not a plugin",
        format!("{error}. Choose the folder that contains plugin.toml and the plugin file."),
    )
}

fn minutes_until(reset_at: Option<u64>) -> Option<u64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let left = reset_at?.checked_sub(now)?;
    Some(left.div_ceil(60).max(1))
}

/// A failure of fetching from the network, as the install dialog words it. `host` is where
/// the user pointed, for the "could not reach" sentence.
pub fn remote_problem(error: RemoteError, host: &str, release: bool) -> Problem {
    if error.is_cancelled() {
        return Problem {
            kind: ProblemKind::Cancelled,
            title: String::new(),
            message: String::new(),
        };
    }
    match error {
        RemoteError::RateLimited { reset_at } => {
            let wait = match minutes_until(reset_at) {
                Some(1) => "Try again in about a minute".to_owned(),
                Some(n) => format!("Try again in about {n} minutes"),
                None => "Try again later".to_owned(),
            };
            Problem {
                kind: ProblemKind::RateLimited,
                title: "GitHub request limit reached".into(),
                message: format!(
                    "GitHub allows 60 requests an hour without signing in, and that limit is \
                     used up. {wait}, install from a folder, or paste a direct link to plugin.toml."
                ),
            }
        }
        RemoteError::NotFound { .. } => Problem::failed(
            "No plugin found",
            "This repository has no published releases, or the address is not a plugin. \
             FerroLibri installs from releases, not from a branch.",
        ),
        RemoteError::Status { status, what } => Problem::failed(
            "Could not reach the address",
            format!("{what}: the server answered {status}. Check the address and try again."),
        ),
        RemoteError::Network(e) => Problem::failed(
            "Could not reach the address",
            format!("No connection to {host} ({e}). Check your network and try again."),
        ),
        RemoteError::Invalid { what, message } => {
            Problem::failed("No plugin found", format!("{what}: {message}"))
        }
        RemoteError::HashMismatch { file, announced_by } => Problem {
            kind: if release {
                ProblemKind::Checksum
            } else {
                ProblemKind::Failed
            },
            title: "Checksum does not match".into(),
            message: format!(
                "{file} does not match the SHA-256 published by {announced_by}. \
                 Nothing was installed."
            ),
        },
        RemoteError::Local(e) => Problem::failed("Could not check the plugin", e.to_string()),
        other => Problem::failed("Could not check the plugin", other.to_string()),
    }
}

/// `nas:8080` of `http://nas:8080/opds`.
fn host_part(address: &str) -> Option<String> {
    address
        .trim()
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
}

fn host_of(address: &str) -> String {
    host_part(address).unwrap_or_else(|| "the address".to_owned())
}

impl PluginRow {
    /// Where the plugin is pointed: the host of the address the user saved for it, else the
    /// first site its manifest names without a wildcard. `None` for a plugin that names none.
    pub fn address_host(&self) -> Option<String> {
        let info = self.info.as_ref()?;
        let saved = info
            .settings
            .iter()
            .find(|f| f.network && f.kind == FieldKind::Url && f.saved);
        if let Some(FieldValue::Text(address)) = saved.map(|f| &f.value)
            && let Some(host) = host_part(address)
        {
            return Some(host);
        }
        info.permissions
            .domains
            .iter()
            .find(|domain| !domain.starts_with("*."))
            .cloned()
    }
}

/// `calibredb` as it was found: `None` = not looked for yet, `Some(None)` = not installed.
type CalibreLookup = Option<Option<Arc<dyn BookAdder>>>;

/// The real plugin operations. Every method blocks; the caller runs them off the UI thread.
#[derive(Clone)]
pub struct PluginBackend {
    store: PluginStore,
    installer: RemoteInstaller,
    /// Runs `search` and `details`. It shares its HTTP client, and so the per-host queues, with
    /// the installer and with the covers.
    runtime: PluginRuntime,
    /// What the last look for `calibredb` found.
    calibre: Arc<Mutex<CalibreLookup>>,
}

impl PluginBackend {
    pub fn new(store: PluginStore) -> Self {
        Self::with_http(store, Http::new())
    }

    /// With the given HTTP client: tests point it at a local server that stands in for a site.
    pub fn with_http(store: PluginStore, http: Http) -> Self {
        Self {
            store,
            installer: RemoteInstaller::new(http.clone()),
            runtime: PluginRuntime::new(http),
            calibre: Arc::default(),
        }
    }

    /// `calibredb` as an adder. Runs the program to find out, so it blocks; a result that says
    /// "not installed" is looked up again each time, because Calibre may have been installed
    /// since (that is what "Check again" is for). `recheck` also forgets a positive answer.
    pub fn calibre(&self, recheck: bool) -> Option<Arc<dyn BookAdder>> {
        let mut cache = self.calibre.lock().unwrap_or_else(PoisonError::into_inner);
        if recheck || cache.as_ref().is_none_or(Option::is_none) {
            *cache = Some(DownloadAndAdd::detect_calibre());
        }
        cache.clone().flatten()
    }

    /// Builds the job for one offer: the plugin is loaded, Calibre and the library are checked.
    /// Blocks (it runs `calibredb --version` the first time).
    pub fn prepare_download(
        &self,
        request: &DownloadRequest,
    ) -> Result<Arc<dyn ExternalJob>, DownloadError> {
        let adder = self.calibre(false).ok_or(DownloadError::NeedsCalibre)?;
        match config::inspect_library_path(&request.library) {
            Ok(LibraryPathState::Ready) => {}
            Ok(state) => {
                return Err(DownloadError::Failed(super::saved_library_message(
                    &request.library,
                    state,
                )));
            }
            Err(error) => return Err(DownloadError::Failed(error.to_string())),
        }
        let plugin = self
            .store
            .load(&request.plugin)
            .map_err(|error| DownloadError::Failed(error.to_string()))?;
        let offer = request
            .hit
            .offers
            .iter()
            .find(|offer| offer.id == request.offer)
            .cloned()
            .ok_or_else(|| DownloadError::Failed("This download is no longer offered.".into()))?;
        let mut job = DownloadAndAdd::new(
            self.runtime.clone(),
            plugin,
            request.hit.as_ref().clone(),
            offer,
            request.library.clone(),
            Some(adder),
        );
        if let Some(dir) = &request.export_dir {
            job = job.with_export_dir(dir.clone());
        }
        Ok(Arc::new(job))
    }

    pub fn store(&self) -> &PluginStore {
        &self.store
    }

    pub fn list(&self) -> Result<Vec<PluginRow>, String> {
        let plugins = self.store.list().map_err(|e| e.to_string())?;
        Ok(plugins.into_iter().map(|p| row(&self.store, p)).collect())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        if enabled {
            self.store.enable(id)
        } else {
            self.store.disable(id)
        }
        .map_err(|e| e.to_string())
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        self.store.remove(id).map_err(|e| e.to_string())
    }

    pub fn save_settings(&self, id: &str, values: &[(String, FieldValue)]) -> Result<(), String> {
        let values = values
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    FieldValue::Text(t) => SettingValue::Text(t.clone()),
                    FieldValue::Bool(b) => SettingValue::Bool(*b),
                };
                (key.clone(), value)
            })
            .collect();
        self.store
            .save_settings(id, &values)
            .map_err(|e| e.to_string())
    }

    pub fn check(
        &self,
        request: &SourceRequest,
        cancel: &CancellationToken,
    ) -> Result<Checked, Problem> {
        match request {
            SourceRequest::Folder(path) => {
                let inspection = self
                    .store
                    .inspect_local(path)
                    .map_err(|e| folder_problem(path, e))?;
                Ok(Checked::Candidate(Box::new(candidate(inspection))))
            }
            SourceRequest::Address(text) => {
                let host = host_of(text);
                let source = RemoteSource::parse(text)
                    .map_err(|e| Problem::failed("No plugin found", e.to_string()))?;
                match source {
                    RemoteSource::Github(repo) => {
                        let list = self
                            .installer
                            .releases(&self.store, &repo, ReleaseOptions::default(), cancel)
                            .map_err(|e| remote_problem(e, "github.com", false))?;
                        Ok(Checked::Releases(Releases {
                            repo: repo.to_string(),
                            items: list
                                .releases
                                .into_iter()
                                .take(RELEASES_SHOWN)
                                .map(release_view)
                                .collect(),
                        }))
                    }
                    RemoteSource::Manifest(url) => {
                        let inspection = self
                            .installer
                            .inspect_manifest_url(&self.store, &url, cancel)
                            .map_err(|e| remote_problem(e, &host, false))?;
                        Ok(Checked::Candidate(Box::new(candidate(inspection))))
                    }
                }
            }
        }
    }

    pub fn inspect_release(
        &self,
        release: &ReleaseView,
        cancel: &CancellationToken,
    ) -> Result<Candidate, Problem> {
        let handle = release
            .handle
            .as_ref()
            .ok_or_else(|| Problem::failed("No plugin found", "The release is not available."))?;
        let inspection = self
            .installer
            .inspect_release(&self.store, handle, cancel)
            .map_err(|e| remote_problem(e, "github.com", true))?;
        Ok(candidate(inspection))
    }

    pub fn install(
        &self,
        candidate: &Candidate,
        options: InstallOptions,
    ) -> Result<Installed, Problem> {
        let inspection = candidate.handle.as_ref().ok_or_else(Problem::unavailable)?;
        let fail = |e: ferrolibri_core::Error| Problem::failed("Could not install", e.to_string());
        let downgrade = if options.allow_downgrade {
            Downgrade::Allow
        } else {
            Downgrade::Refuse
        };
        self.store.install(inspection, downgrade).map_err(fail)?;
        // The store turns a new plugin on; the dialog's box decides.
        let is_new = candidate.change == Change::New;
        if is_new {
            if options.turn_on {
                self.store.enable(&candidate.id)
            } else {
                self.store.disable(&candidate.id)
            }
            .map_err(fail)?;
        }
        Ok(Installed {
            name: candidate.name.clone(),
            version: candidate.version.clone(),
            change: candidate.change.clone(),
            turned_on: is_new && options.turn_on,
        })
    }

    fn loaded(&self, id: &str) -> Result<LoadedPlugin, SourceError> {
        self.store.load(id).map_err(|error| match error.kind {
            ErrorKind::Plugin { message, .. } => {
                SourceError::new(SourceErrorKind::Unavailable, capitalised(&message))
            }
            _ => SourceError::new(SourceErrorKind::Unavailable, error.to_string()),
        })
    }

    /// One page of `search(query, ctx)`, checked against the contract. Blocks.
    pub fn search(
        &self,
        id: &str,
        query: &SearchQuery,
        cancel: &CancellationToken,
    ) -> Result<RemotePage, SourceError> {
        let plugin = self.loaded(id)?;
        let arg = serde_json::to_value(query).expect("a query always serializes");
        let answer = self
            .runtime
            .call(&plugin, PluginFunction::Search, &arg, cancel)
            .map_err(source_error)?;
        let allow = plugin
            .allow_list()
            .map_err(|e| SourceError::plugin(e.to_string()))?;
        let parsed = parse_search_page(&answer.to_string(), &|host| allow.allows_host(host))
            .map_err(|e| unexpected_response(&e))?;
        for warning in &parsed.warnings {
            log::warn!(target: "plugin", "[{id}] {warning}");
        }
        Ok(RemotePage {
            dropped: parsed.warnings.len(),
            next_cursor: parsed.page.next_cursor,
            hits: parsed.page.hits.into_iter().map(RemoteHit::from).collect(),
        })
    }

    /// `details(hit, ctx)` merged into the result; a plugin without `details` changes nothing.
    pub fn details(
        &self,
        id: &str,
        hit: &RemoteHit,
        cancel: &CancellationToken,
    ) -> Result<RemoteHit, SourceError> {
        let handle = hit
            .handle
            .as_ref()
            .ok_or_else(|| SourceError::plugin("the result cannot be passed to the plugin"))?;
        let plugin = self.loaded(id)?;
        let arg = serde_json::to_value(handle.as_ref()).expect("a result always serializes");
        let answer = self
            .runtime
            .call(&plugin, PluginFunction::Details, &arg, cancel)
            .map_err(source_error)?;
        let allow = plugin
            .allow_list()
            .map_err(|e| SourceError::plugin(e.to_string()))?;
        let merged = handle
            .with_details(&answer.to_string(), &|host| allow.allows_host(host))
            .map_err(|e| unexpected_response(&e))?;
        Ok(RemoteHit::from(merged))
    }

    /// The cover at `url` as bytes, fetched the way a plugin's own request would be: only
    /// hosts the plugin may reach, at most [`MAX_COVER_BYTES`], any status but 2xx refused.
    pub fn fetch_cover(
        &self,
        id: &str,
        url: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, String> {
        let plugin = self.loaded(id).map_err(|e| e.message)?;
        let allow = plugin.allow_list().map_err(|e| e.to_string())?;
        let session = self
            .runtime
            .http()
            .session(Identity::Plugin(id.to_owned()), allow, cancel.clone())
            .with_request_budget(1);
        let mut stream = session
            .open(url, &[("Accept", "image/*")], MAX_COVER_BYTES)
            .map_err(|e| e.to_string())?;
        let status = stream.head().status;
        if !stream.head().is_success() {
            return Err(format!("the server answered {status}"));
        }
        stream.read_to_end().map_err(|e| e.to_string())
    }
}

/// Largest cover that is downloaded for a result.
pub const MAX_COVER_BYTES: u64 = 2 * 1024 * 1024;

fn unexpected_response(error: &ferrolibri_plugins::ResponseError) -> SourceError {
    SourceError::plugin(format!(
        "plugin error: the plugin returned an unexpected response ({error})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_shown_the_way_the_design_has_them() {
        assert_eq!(format_date(0), "1 Jan 1970");
        assert_eq!(format_date(1_792_195_200), "17 Oct 2026");
        assert_eq!(format_date(1_709_164_800), "29 Feb 2024");
        assert_eq!(release_date(Some("2026-10-14T09:30:00Z")), "14 Oct 2026");
        assert_eq!(release_date(Some("2026-07-14")), "14 Jul 2026");
        assert_eq!(release_date(Some("not a date")), "not a date");
        assert_eq!(
            release_date(Some("2026-13-40T00:00:00Z")),
            "2026-13-40T00:00:00Z"
        );
        assert_eq!(release_date(None), "");
    }

    #[test]
    fn network_failures_get_the_titles_of_the_design() {
        let limited = remote_problem(
            RemoteError::RateLimited { reset_at: None },
            "github.com",
            false,
        );
        assert_eq!(limited.kind, ProblemKind::RateLimited);
        assert_eq!(limited.title, "GitHub request limit reached");
        assert!(limited.message.contains("60 requests an hour"));
        let soon = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 38 * 60
            - 5;
        let limited = remote_problem(
            RemoteError::RateLimited {
                reset_at: Some(soon),
            },
            "x",
            false,
        );
        assert!(
            limited.message.contains("about 38 minutes"),
            "{}",
            limited.message
        );
        let missing = remote_problem(
            RemoteError::NotFound {
                what: "repo".into(),
            },
            "x",
            false,
        );
        assert_eq!(
            (missing.title.as_str(), missing.kind),
            ("No plugin found", ProblemKind::Failed)
        );
        let offline = remote_problem(
            RemoteError::Network(ferrolibri_plugins::HostError::new(
                ferrolibri_plugins::HostErrorKind::Timeout,
                "slow",
            )),
            "github.com",
            false,
        );
        assert_eq!(offline.title, "Could not reach the address");
        assert!(offline.message.contains("No connection to github.com"));
        let cancelled = remote_problem(
            RemoteError::Network(ferrolibri_plugins::HostError::new(
                ferrolibri_plugins::HostErrorKind::Cancelled,
                "stop",
            )),
            "x",
            false,
        );
        assert_eq!(cancelled.kind, ProblemKind::Cancelled);
        let from_release = RemoteError::HashMismatch {
            file: "plugin.js".into(),
            announced_by: "GitHub",
        };
        assert_eq!(
            remote_problem(from_release, "x", true).kind,
            ProblemKind::Checksum
        );
    }

    #[test]
    fn plugin_failures_are_told_apart_by_their_words() {
        use SourceErrorKind::{Plugin, Server, Unreachable};
        for kind in ["connect", "timeout", "dns", "tls"] {
            let message = format!("Network error ({kind}): no answer");
            assert_eq!(classify_plugin_message(&message), Unreachable, "{message}");
        }
        for message in [
            "The catalog answered with status 503",
            "HTTP 502 from the mirror",
            "bad status: 500",
        ] {
            assert_eq!(classify_plugin_message(message), Server, "{message}");
        }
        for message in [
            "The catalog answered with status 404",
            "network error (blocked_domain): host is not allowed",
            "invoice 503 not found",
            "plugin error: hits is missing",
        ] {
            assert_eq!(classify_plugin_message(message), Plugin, "{message}");
        }
    }

    #[test]
    fn a_row_names_the_host_it_is_pointed_at() {
        let mut row = PluginRow {
            id: "p".into(),
            name: "P".into(),
            version: "1".into(),
            author: String::new(),
            description: String::new(),
            enabled: true,
            health: Health::Ready,
            info: Some(PluginInfo {
                permissions: PermissionSummary {
                    domains: vec!["*.cdn.test".into(), "books.test".into()],
                    ..Default::default()
                },
                settings: vec![SettingField {
                    key: "base".into(),
                    label: "Address".into(),
                    kind: FieldKind::Url,
                    network: true,
                    value: FieldValue::Text("http://nas:8080/opds".into()),
                    placeholder: None,
                    saved: false,
                }],
                source: String::new(),
                installed: String::new(),
                search_hint: None,
            }),
        };
        // Not saved yet: the first plain site of the manifest.
        assert_eq!(row.address_host().as_deref(), Some("books.test"));
        row.info.as_mut().unwrap().settings[0].saved = true;
        assert_eq!(row.address_host().as_deref(), Some("nas:8080"));
        row.info = None;
        assert_eq!(row.address_host(), None);
    }

    #[test]
    fn the_host_in_a_message_is_the_one_the_user_typed() {
        assert_eq!(host_of("https://example.org/a/plugin.toml"), "example.org");
        assert_eq!(host_of("  http://nas:8080/x "), "nas:8080");
        assert_eq!(host_of("nonsense"), "the address");
    }

    #[test]
    fn an_address_that_opens_the_network_starts_empty_and_needs_saving() {
        let manifest = Manifest::parse(
            r#"
id = "demo"
name = "Demo"
version = "1.0.0"
api_version = 1
provides = ["search"]
[[settings]]
key = "base"
type = "url"
label = "Address"
network = true
default = "https://example.org"
[[settings]]
key = "order"
type = "choice"
label = "Order"
options = ["a", "b"]
default = "b"
"#,
        )
        .unwrap();
        let fields = setting_fields(&manifest, &Default::default());
        assert_eq!(fields[0].value, FieldValue::Text(String::new()));
        assert_eq!(
            fields[0].placeholder.as_deref(),
            Some("https://example.org")
        );
        assert!(fields[0].needs_address());
        assert_eq!(fields[1].value, FieldValue::Text("b".into()));
        let saved = [(
            "base".to_owned(),
            SettingValue::Text("https://mirror.test".into()),
        )]
        .into_iter()
        .collect();
        let fields = setting_fields(&manifest, &saved);
        assert_eq!(
            fields[0].value,
            FieldValue::Text("https://mirror.test".into())
        );
        assert!(!fields[0].needs_address());
    }
}
