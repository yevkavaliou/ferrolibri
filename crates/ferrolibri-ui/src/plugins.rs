//! Settings → Plugins and the install dialog: their state and the pure `update` step.
//!
//! Everything that touches the disk or the network goes through [`Services`]. Screens are
//! drawn in `widgets::plugins` after `docs/design/mockup.html` (design gaps G2 and G3).
use crate::{
    app::{Message, State},
    services::{
        Services,
        plugins::{
            Candidate, Change, Checked, FieldKind, FieldValue, InstallOptions, PluginRow, Problem,
            ProblemKind, Releases, SourceRequest,
        },
    },
    widgets::toasts,
};
use ferrolibri_core::converter::CancellationToken;
use iced::Task;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Folder,
    Address,
}

/// Unsaved edits of one plugin's settings form, one entry per declared setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub values: Vec<FieldValue>,
    pub errors: Vec<Option<String>>,
    pub saving: bool,
}

impl Form {
    fn of(row: &PluginRow) -> Self {
        let fields = row.info.as_ref().map_or(&[][..], |i| &i.settings[..]);
        Self {
            values: fields.iter().map(|f| f.value.clone()).collect(),
            errors: vec![None; fields.len()],
            saving: false,
        }
    }
}

/// The install dialog: Source → Check → Confirm.
#[derive(Debug)]
pub struct Install {
    /// 1 = Source, 2 = Check, 3 = Confirm.
    pub step: u8,
    pub mode: Mode,
    pub folder: String,
    pub address: String,
    /// What is being waited for; the dialog shows it with a Cancel button.
    pub busy: Option<String>,
    pub problem: Option<Problem>,
    pub releases: Option<Releases>,
    pub candidate: Option<Candidate>,
    pub turn_on: bool,
    pub allow_downgrade: bool,
    token: Option<CancellationToken>,
    installing: bool,
}

impl Install {
    fn new(mode: Mode) -> Self {
        Self {
            step: 1,
            mode,
            folder: String::new(),
            address: String::new(),
            busy: None,
            problem: None,
            releases: None,
            candidate: None,
            turn_on: true,
            allow_downgrade: false,
            token: None,
            installing: false,
        }
    }
    pub fn input(&self) -> &str {
        match self.mode {
            Mode::Folder => &self.folder,
            Mode::Address => &self.address,
        }
    }
    pub fn can_check(&self) -> bool {
        !self.input().trim().is_empty()
    }
}

#[derive(Default)]
pub struct PluginsState {
    pub rows: Vec<PluginRow>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// The plugin whose Details are open.
    pub expanded: Option<String>,
    /// The plugin whose removal waits for "Remove".
    pub removing: Option<String>,
    /// Edits that are not saved yet; they survive leaving the screen.
    pub forms: BTreeMap<String, Form>,
    /// Plugins with a change in flight; their controls wait.
    pub pending: BTreeSet<String>,
    pub install: Option<Install>,
    list_generation: u64,
    install_generation: u64,
}

impl PluginsState {
    pub fn row(&self, id: &str) -> Option<&PluginRow> {
        self.rows.iter().find(|r| r.id == id)
    }
    /// The value a field shows: the edit if there is one, else what is saved.
    pub fn value<'a>(&'a self, row: &'a PluginRow, index: usize) -> Option<&'a FieldValue> {
        self.forms
            .get(&row.id)
            .and_then(|f| f.values.get(index))
            .or_else(|| row.info.as_ref()?.settings.get(index).map(|f| &f.value))
    }
}

#[derive(Clone, Debug)]
pub enum Msg {
    Loaded {
        generation: u64,
        result: Result<Vec<PluginRow>, String>,
    },
    ToggleDetails(String),
    SetEnabled {
        id: String,
        on: bool,
    },
    EnabledSet {
        id: String,
        on: bool,
        result: Result<(), String>,
    },
    FieldEdited {
        id: String,
        index: usize,
        value: FieldValue,
    },
    SaveSettings(String),
    SettingsSaved {
        id: String,
        result: Result<(), String>,
    },
    AskRemove(String),
    KeepPlugin,
    ConfirmRemove(String),
    Removed {
        id: String,
        result: Result<(), String>,
    },
    OpenInstall(Option<Mode>),
    CloseInstall,
    SetMode(Mode),
    FolderInput(String),
    AddressInput(String),
    Browse,
    Browsed(Result<Option<PathBuf>, String>),
    Check,
    Checked {
        generation: u64,
        result: Result<Checked, Problem>,
    },
    AbortBusy,
    Back,
    PickRelease(String),
    ReleaseInspected {
        generation: u64,
        result: Result<Box<Candidate>, Problem>,
    },
    ChooseAnotherRelease,
    Continue,
    TurnOn(bool),
    AllowDowngrade(bool),
    Install,
    Installed {
        result: Result<crate::services::plugins::Installed, Problem>,
    },
}

impl From<Msg> for Message {
    fn from(msg: Msg) -> Self {
        Message::Plugins(msg)
    }
}

/// "Enter a full address starting with http:// or https://" unless `text` is one.
pub fn web_address_error(text: &str) -> Option<&'static str> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let host = rest.and_then(|r| r.split(['/', '?', '#']).next());
    match host {
        Some(h) if !h.is_empty() && !text.chars().any(char::is_whitespace) => None,
        _ => Some("Enter a full address starting with http:// or https://"),
    }
}

/// An address without TLS: the connection can be read and changed on the way (D4).
pub fn is_plain_http(text: &str) -> bool {
    text.trim().to_ascii_lowercase().starts_with("http://")
}

/// Loads the list again; the screen keeps showing the old rows meanwhile.
pub fn reload(state: &mut State, services: &dyn Services) -> Task<Message> {
    state.plugins.list_generation += 1;
    state.plugins.loading = true;
    let generation = state.plugins.list_generation;
    Task::perform(services.list_plugins(), move |result| {
        Msg::Loaded { generation, result }.into()
    })
}

fn name_of(state: &State, id: &str) -> String {
    state
        .plugins
        .row(id)
        .map_or_else(|| id.to_owned(), |r| r.name.clone())
}

pub fn update(state: &mut State, msg: Msg, services: &dyn Services) -> Task<Message> {
    if state.closing {
        return Task::none();
    }
    match msg {
        Msg::Loaded { generation, result } => {
            if generation != state.plugins.list_generation {
                return Task::none();
            }
            state.plugins.loading = false;
            match result {
                Ok(rows) => {
                    state.plugins.rows = rows;
                    state.plugins.loaded = true;
                    state.plugins.error = None;
                    settle(&mut state.plugins);
                }
                Err(message) => state.plugins.error = Some(message),
            }
        }
        Msg::ToggleDetails(id) => {
            let closing = state.plugins.expanded.as_deref() == Some(id.as_str());
            state.plugins.expanded = (!closing).then_some(id);
            state.plugins.removing = None;
        }
        Msg::SetEnabled { id, on } => {
            if state.plugins.pending.insert(id.clone()) {
                return Task::perform(services.set_plugin_enabled(id.clone(), on), move |result| {
                    Msg::EnabledSet {
                        id: id.clone(),
                        on,
                        result,
                    }
                    .into()
                });
            }
        }
        Msg::EnabledSet { id, on, result } => {
            state.plugins.pending.remove(&id);
            let name = name_of(state, &id);
            match result {
                Ok(()) => toasts::push(
                    state,
                    if on {
                        format!("{name} is on. It is now a source on the Search screen.")
                    } else {
                        format!("{name} is off.")
                    },
                    false,
                ),
                Err(error) => {
                    toasts::push(state, format!("Could not change {name}. {error}"), true)
                }
            }
            return reload(state, services);
        }
        Msg::FieldEdited { id, index, value } => {
            let Some(row) = state.plugins.row(&id).cloned() else {
                return Task::none();
            };
            let form = state
                .plugins
                .forms
                .entry(id)
                .or_insert_with(|| Form::of(&row));
            if let (Some(slot), Some(error)) =
                (form.values.get_mut(index), form.errors.get_mut(index))
            {
                *slot = value;
                *error = None;
            }
        }
        Msg::SaveSettings(id) => return save_settings(state, &id, services),
        Msg::SettingsSaved { id, result } => {
            let name = name_of(state, &id);
            if let Some(form) = state.plugins.forms.get_mut(&id) {
                form.saving = false;
            }
            match result {
                Ok(()) => {
                    toasts::push(state, format!("Settings for {name} saved."), false);
                    return reload(state, services);
                }
                Err(error) => toasts::push(
                    state,
                    format!("Could not save the settings of {name}. {error}"),
                    true,
                ),
            }
        }
        Msg::AskRemove(id) => state.plugins.removing = Some(id),
        Msg::KeepPlugin => state.plugins.removing = None,
        Msg::ConfirmRemove(id) => {
            if state.plugins.pending.insert(id.clone()) {
                return Task::perform(services.remove_plugin(id.clone()), move |result| {
                    Msg::Removed {
                        id: id.clone(),
                        result,
                    }
                    .into()
                });
            }
        }
        Msg::Removed { id, result } => {
            state.plugins.pending.remove(&id);
            let name = name_of(state, &id);
            match result {
                Ok(()) => {
                    state.plugins.removing = None;
                    state.plugins.expanded = None;
                    state.plugins.forms.remove(&id);
                    toasts::push(state, format!("Removed {name}."), false);
                }
                Err(error) => {
                    toasts::push(state, format!("Could not remove {name}. {error}"), true)
                }
            }
            return reload(state, services);
        }
        Msg::OpenInstall(mode) => {
            state.plugins.install_generation += 1;
            state.plugins.install = Some(Install::new(mode.unwrap_or_default()));
        }
        Msg::CloseInstall => close_install(state),
        Msg::SetMode(mode) => {
            if let Some(install) = state.plugins.install.as_mut().filter(|i| i.step == 1) {
                install.mode = mode;
            }
        }
        Msg::FolderInput(text) => {
            if let Some(install) = state.plugins.install.as_mut() {
                install.folder = text;
            }
        }
        Msg::AddressInput(text) => {
            if let Some(install) = state.plugins.install.as_mut() {
                install.address = text;
            }
        }
        Msg::Browse => {
            if state.plugins.install.is_some() {
                return Task::perform(services.pick_folder(), |r| Msg::Browsed(r).into());
            }
        }
        Msg::Browsed(result) => {
            if let Some(install) = state.plugins.install.as_mut().filter(|i| i.step == 1) {
                match result {
                    Ok(Some(path)) => install.folder = path.display().to_string(),
                    Ok(None) => {}
                    Err(error) => toasts::push(state, error, true),
                }
            }
        }
        Msg::Check => return start_check(state, services),
        Msg::Checked { generation, result } => {
            if generation != state.plugins.install_generation {
                return Task::none();
            }
            let Some(install) = state.plugins.install.as_mut() else {
                return Task::none();
            };
            install.busy = None;
            install.token = None;
            match result {
                Ok(Checked::Releases(releases)) => install.releases = Some(releases),
                Ok(Checked::Candidate(candidate)) => install.candidate = Some(*candidate),
                Err(problem) if problem.kind == ProblemKind::Cancelled => install.step = 1,
                Err(problem) => install.problem = Some(problem),
            }
        }
        Msg::AbortBusy => abort_busy(state),
        Msg::Back => back(state),
        Msg::PickRelease(tag) => return pick_release(state, &tag, services),
        Msg::ReleaseInspected { generation, result } => {
            if generation != state.plugins.install_generation {
                return Task::none();
            }
            let Some(install) = state.plugins.install.as_mut() else {
                return Task::none();
            };
            install.busy = None;
            install.token = None;
            match result {
                Ok(candidate) => install.candidate = Some(*candidate),
                Err(problem) if problem.kind == ProblemKind::Cancelled => {}
                Err(problem) => install.problem = Some(problem),
            }
        }
        Msg::ChooseAnotherRelease => {
            if let Some(install) = state.plugins.install.as_mut() {
                install.problem = None;
                install.candidate = None;
            }
        }
        Msg::Continue => {
            if let Some(install) = state.plugins.install.as_mut()
                && install.candidate.as_ref().is_some_and(|c| c.compatible)
            {
                install.step = 3;
            }
        }
        Msg::TurnOn(on) => {
            if let Some(install) = state.plugins.install.as_mut() {
                install.turn_on = on;
            }
        }
        Msg::AllowDowngrade(allow) => {
            if let Some(install) = state.plugins.install.as_mut() {
                install.allow_downgrade = allow;
            }
        }
        Msg::Install => return start_install(state, services),
        Msg::Installed { result } => return installed(state, result, services),
    }
    Task::none()
}

/// After a reload: edits of plugins that are gone, or whose form changed, or that equal what
/// is saved are dropped; the rest stay.
fn settle(plugins: &mut PluginsState) {
    let rows = &plugins.rows;
    plugins.forms.retain(|id, form| {
        let Some(row) = rows.iter().find(|r| r.id == *id) else {
            return false;
        };
        let saved: Vec<&FieldValue> = row
            .info
            .as_ref()
            .map(|i| i.settings.iter().map(|f| &f.value).collect())
            .unwrap_or_default();
        saved.len() == form.values.len()
            && !form.saving
            && saved.iter().zip(&form.values).any(|(a, b)| *a != b)
    });
    if plugins
        .expanded
        .as_ref()
        .is_some_and(|id| !plugins.rows.iter().any(|r| r.id == *id))
    {
        plugins.expanded = None;
    }
    if plugins
        .removing
        .as_ref()
        .is_some_and(|id| !plugins.rows.iter().any(|r| r.id == *id))
    {
        plugins.removing = None;
    }
}

fn save_settings(state: &mut State, id: &str, services: &dyn Services) -> Task<Message> {
    let Some(row) = state.plugins.row(id).cloned() else {
        return Task::none();
    };
    let Some(info) = row.info.as_ref() else {
        return Task::none();
    };
    let values: Vec<FieldValue> = (0..info.settings.len())
        .filter_map(|i| state.plugins.value(&row, i).cloned())
        .collect();
    let form = state
        .plugins
        .forms
        .entry(row.id.clone())
        .or_insert_with(|| Form::of(&row));
    if form.saving {
        return Task::none();
    }
    let mut valid = true;
    for (index, field) in info.settings.iter().enumerate() {
        form.errors[index] = None;
        if let (FieldKind::Url, Some(FieldValue::Text(text))) = (&field.kind, values.get(index))
            && !text.trim().is_empty()
            && let Some(message) = web_address_error(text)
        {
            form.errors[index] = Some(message.to_owned());
            valid = false;
        }
    }
    if !valid {
        return Task::none();
    }
    form.values.clone_from(&values);
    form.saving = true;
    let keyed = info
        .settings
        .iter()
        .zip(values)
        .map(|(field, value)| (field.key.clone(), value))
        .collect();
    let id = row.id;
    Task::perform(
        services.save_plugin_settings(id.clone(), keyed),
        move |result| {
            Msg::SettingsSaved {
                id: id.clone(),
                result,
            }
            .into()
        },
    )
}

fn close_install(state: &mut State) {
    state.plugins.install_generation += 1;
    if let Some(token) = state
        .plugins
        .install
        .as_ref()
        .and_then(|i| i.token.as_ref())
    {
        token.cancel();
    }
    state.plugins.install = None;
}

fn start_check(state: &mut State, services: &dyn Services) -> Task<Message> {
    let generation = state.plugins.install_generation + 1;
    let Some(install) = state.plugins.install.as_mut() else {
        return Task::none();
    };
    if !install.can_check() || install.busy.is_some() {
        return Task::none();
    }
    install.step = 2;
    install.problem = None;
    install.releases = None;
    install.candidate = None;
    let request = match install.mode {
        Mode::Folder => match ferrolibri_core::config::sanitize_path(&install.folder) {
            Ok(path) => {
                install.busy = Some("Reading plugin.toml and checking compatibility…".into());
                SourceRequest::Folder(path)
            }
            Err(error) => {
                install.problem = Some(Problem::failed(
                    "This folder is not a plugin",
                    format!(
                        "{error}. Choose the folder that contains plugin.toml and the plugin file."
                    ),
                ));
                return Task::none();
            }
        },
        Mode::Address => {
            let address = install.address.trim().to_owned();
            install.busy = Some(
                if address.to_ascii_lowercase().contains("github.com") {
                    "Looking up releases on github.com…"
                } else {
                    "Downloading and checking plugin.toml…"
                }
                .into(),
            );
            SourceRequest::Address(address)
        }
    };
    let token = CancellationToken::default();
    install.token = Some(token.clone());
    state.plugins.install_generation = generation;
    Task::perform(
        services.check_plugin_source(request, token),
        move |result| Msg::Checked { generation, result }.into(),
    )
}

fn pick_release(state: &mut State, tag: &str, services: &dyn Services) -> Task<Message> {
    let generation = state.plugins.install_generation + 1;
    let Some(install) = state.plugins.install.as_mut() else {
        return Task::none();
    };
    let Some(release) = install
        .releases
        .as_ref()
        .and_then(|r| r.items.iter().find(|r| r.tag == tag && r.compatible))
        .cloned()
    else {
        return Task::none();
    };
    if install.busy.is_some() {
        return Task::none();
    }
    install.problem = None;
    install.busy = Some(format!("Downloading and checking {tag}…"));
    let token = CancellationToken::default();
    install.token = Some(token.clone());
    state.plugins.install_generation = generation;
    Task::perform(
        services.inspect_plugin_release(release, token),
        move |result| {
            Msg::ReleaseInspected {
                generation,
                result: result.map(Box::new),
            }
            .into()
        },
    )
}

/// Cancel of a wait: a check goes back to the first step (unless a list of releases is
/// already there), an installation in progress cannot be undone, so the dialog just closes.
fn abort_busy(state: &mut State) {
    let installing = state.plugins.install.as_ref().is_some_and(|i| i.installing);
    if installing {
        close_install(state);
        return;
    }
    state.plugins.install_generation += 1;
    if let Some(install) = state.plugins.install.as_mut() {
        if let Some(token) = install.token.take() {
            token.cancel();
        }
        install.busy = None;
        if install.step == 2 && install.releases.is_none() {
            install.step = 1;
        }
    }
}

fn back(state: &mut State) {
    let Some(install) = state.plugins.install.as_mut() else {
        return;
    };
    if install.step == 3 {
        install.step = 2;
    } else if install.step == 2 && install.candidate.is_some() && install.releases.is_some() {
        install.candidate = None;
        install.problem = None;
    } else {
        install.step = 1;
        install.candidate = None;
        install.releases = None;
        install.problem = None;
    }
}

fn start_install(state: &mut State, services: &dyn Services) -> Task<Message> {
    let Some(install) = state.plugins.install.as_mut() else {
        return Task::none();
    };
    let Some(candidate) = install.candidate.clone().filter(|c| c.compatible) else {
        return Task::none();
    };
    if install.step != 3 || install.busy.is_some() {
        return Task::none();
    }
    let downgrade = matches!(candidate.change, Change::Downgrade { .. });
    if downgrade && !install.allow_downgrade {
        return Task::none();
    }
    install.busy = Some("Installing…".into());
    install.installing = true;
    let options = InstallOptions {
        turn_on: install.turn_on,
        allow_downgrade: install.allow_downgrade,
    };
    Task::perform(services.install_plugin(candidate, options), |result| {
        Msg::Installed { result }.into()
    })
}

fn installed(
    state: &mut State,
    result: Result<crate::services::plugins::Installed, Problem>,
    services: &dyn Services,
) -> Task<Message> {
    match result {
        Ok(done) => {
            state.plugins.install_generation += 1;
            state.plugins.install = None;
            let verb = match &done.change {
                Change::New => "Installed",
                Change::Reinstall { .. } => "Reinstalled",
                Change::Upgrade { .. } => "Updated",
                Change::Downgrade { .. } => "Installed older version of",
            };
            let tail = if done.turned_on {
                " and turned it on."
            } else {
                "."
            };
            toasts::push(
                state,
                format!("{verb} {} v{}{tail}", done.name, done.version),
                false,
            );
            return reload(state, services);
        }
        Err(problem) => {
            if let Some(install) = state.plugins.install.as_mut() {
                install.busy = None;
                install.installing = false;
            }
            toasts::push(
                state,
                format!("{}. {}", problem.title, problem.message),
                true,
            );
        }
    }
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_need_a_scheme_and_a_host() {
        for good in [
            "https://example.org",
            "HTTP://nas:8080/opds",
            "  https://192.168.1.5:8080  ",
        ] {
            assert_eq!(web_address_error(good), None, "{good}");
        }
        for bad in [
            "",
            "example.org",
            "ftp://example.org",
            "https://",
            "https://a b.org",
        ] {
            assert!(web_address_error(bad).is_some(), "{bad}");
        }
        assert!(is_plain_http(" http://nas/opds"));
        assert!(!is_plain_http("https://nas/opds"));
    }
}
