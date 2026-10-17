//! Installed plugins: where they live, whether they are enabled, their settings (D2, D11).
//!
//! `<data_dir>/dev.yevkavaliou.ferrolibri/plugins/<id>/` holds `plugin.toml`, the script and
//! `install.json`; `<config_dir>/dev.yevkavaliou.ferrolibri/plugins.toml` holds the switches and
//! the settings. A plugin directory that cannot be read is reported as broken and does not
//! stop the others from loading. Installation from a folder is in [`install`].

mod install;
mod state;
#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use ferrolibri_core::{Error, ErrorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) use install::store_compatibility;
pub use install::{Downgrade, Inspection, InstallChange, Permissions};

use crate::{
    Compatibility, LoadedPlugin, Manifest, SettingValue, app_version, limits::*, plugin_error,
};
use state::StateFile;

const APP_DIR: &str = "dev.yevkavaliou.ferrolibri";
const MANIFEST_FILE: &str = "plugin.toml";
const RECORD_FILE: &str = "install.json";

/// Where a plugin came from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InstallSource {
    /// A folder on this computer (an absolute path as the user gave it).
    Local { path: String },
    /// A release of a GitHub repository: `repo` is `owner/name`, `tag` the release's tag.
    Github { repo: String, tag: String },
    /// A `plugin.toml` at an address the user gave, with the script next to it.
    Url { url: String },
}

/// `install.json`: how the files in the plugin directory got there.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InstallRecord {
    pub source: InstallSource,
    /// `version` of the manifest at the time of installation.
    pub version: String,
    /// SHA-256 of the script, lowercase hex. The script is checked against it on every read.
    pub sha256: String,
    /// Seconds since 1970-01-01 UTC.
    pub installed_at: u64,
}

#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    /// The name of the plugin's directory; the manifest `id` for a healthy plugin.
    pub id: String,
    pub enabled: bool,
    pub status: PluginStatus,
}

#[derive(Clone, Debug)]
pub enum PluginStatus {
    /// The files are intact. `compatibility` is checked against this build of the app, so a
    /// plugin that was installed by an older build can show up as incompatible.
    Ready {
        manifest: Box<Manifest>,
        compatibility: Compatibility,
        install: InstallRecord,
    },
    /// The directory cannot be used; the message says why. It can still be removed.
    Broken { message: String },
}

/// The plugin store. Cheap to clone; clones share one lock, so changes made through them do not
/// interleave. Two processes working on one store are not coordinated.
#[derive(Clone, Debug)]
pub struct PluginStore {
    plugins_dir: PathBuf,
    state_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl PluginStore {
    pub fn new(plugins_dir: impl Into<PathBuf>, state_path: impl Into<PathBuf>) -> Self {
        Self {
            plugins_dir: plugins_dir.into(),
            state_path: state_path.into(),
            lock: Arc::default(),
        }
    }

    /// The store at its place in the user's data and config directories (D11).
    pub fn default_location() -> Result<Self, Error> {
        let data =
            dirs::data_dir().ok_or_else(|| Error::unavailable("locate data directory", None))?;
        let config = dirs::config_dir()
            .ok_or_else(|| Error::unavailable("locate config directory", None))?;
        Ok(Self::new(
            data.join(APP_DIR).join("plugins"),
            config.join(APP_DIR).join("plugins.toml"),
        ))
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// Every installed plugin, sorted by `id`; a missing `plugins` directory is an empty list.
    /// Fails only if the directory or `plugins.toml` cannot be read at all.
    pub fn list(&self) -> Result<Vec<InstalledPlugin>, Error> {
        let state = StateFile::read(&self.state_path)?;
        let entries = match fs::read_dir(&self.plugins_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io("list plugins", &self.plugins_dir, e)),
        };
        let mut plugins = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::io("list plugins", &self.plugins_dir, e))?;
            // Staging and backup directories start with a dot; so does nothing a user installed.
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            plugins.push(self.describe(&name, &state));
        }
        plugins.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(plugins)
    }

    /// One installed plugin; `NotFound` if there is no such directory.
    pub fn get(&self, id: &str) -> Result<InstalledPlugin, Error> {
        self.existing_dir(id)?;
        let state = StateFile::read(&self.state_path)?;
        Ok(self.describe(id, &state))
    }

    /// Prepares an enabled, healthy, compatible plugin to run: its script and the stored
    /// settings that its manifest still declares and accepts.
    pub fn load(&self, id: &str) -> Result<LoadedPlugin, Error> {
        let state = StateFile::read(&self.state_path)?;
        let found = self
            .read_installed(id)
            .map_err(|m| plugin_error("load plugin", id, m))?;
        let stored = state.plugins.get(id);
        if !stored.is_some_and(|s| s.enabled) {
            return Err(plugin_error("load plugin", id, "the plugin is disabled"));
        }
        if let Compatibility::Incompatible(reasons) = &found.compatibility {
            return Err(plugin_error("load plugin", id, not_compatible(reasons)));
        }
        let source = String::from_utf8(found.script.unwrap_or_default())
            .map_err(|_| plugin_error("load plugin", id, "the script is not UTF-8 text"))?;
        let settings = stored.map_or_else(BTreeMap::new, |s| {
            state::usable(&found.manifest, &s.settings)
        });
        Ok(LoadedPlugin {
            manifest: found.manifest,
            source,
            settings,
        })
    }

    /// Turns a healthy, compatible plugin on.
    pub fn enable(&self, id: &str) -> Result<(), Error> {
        let _guard = self.guard();
        let found = self
            .read_installed(id)
            .map_err(|m| plugin_error("enable plugin", id, m))?;
        if let Compatibility::Incompatible(reasons) = &found.compatibility {
            return Err(plugin_error("enable plugin", id, not_compatible(reasons)));
        }
        self.update_state(|state| state.plugins.entry(id.to_owned()).or_default().enabled = true)
    }

    /// Turns a plugin off; works for a broken one too.
    pub fn disable(&self, id: &str) -> Result<(), Error> {
        let _guard = self.guard();
        self.existing_dir(id)?;
        self.update_state(|state| state.plugins.entry(id.to_owned()).or_default().enabled = false)
    }

    /// Deletes the plugin's directory and forgets its settings. A directory that is only partly
    /// deleted (a file was in use) shows up as broken and can be removed again.
    pub fn remove(&self, id: &str) -> Result<(), Error> {
        let _guard = self.guard();
        let dir = self.existing_dir(id)?;
        fs::remove_dir_all(&dir).map_err(|e| Error::io("remove plugin", &dir, e))?;
        self.update_state(|state| drop(state.plugins.remove(id)))
    }

    /// The values the user saved for the plugin, as stored.
    pub fn settings(&self, id: &str) -> Result<BTreeMap<String, SettingValue>, Error> {
        self.dir_for(id)?;
        let state = StateFile::read(&self.state_path)?;
        Ok(state
            .plugins
            .get(id)
            .map(|s| s.settings.clone())
            .unwrap_or_default())
    }

    /// Replaces the plugin's saved values with `values` (the whole form). Every key must be
    /// declared by the manifest and every value must suit it. Text is trimmed, and a blank
    /// `string` or `url` is dropped: it means "not set".
    pub fn save_settings(
        &self,
        id: &str,
        values: &BTreeMap<String, SettingValue>,
    ) -> Result<(), Error> {
        let _guard = self.guard();
        let found = self
            .read_installed(id)
            .map_err(|m| plugin_error("save plugin settings", id, m))?;
        let mut checked = BTreeMap::new();
        for (key, value) in values {
            let setting = found
                .manifest
                .settings
                .iter()
                .find(|s| s.key == *key)
                .ok_or_else(|| {
                    plugin_error("save plugin settings", id, format!("unknown setting {key}"))
                })?;
            let value = match value {
                SettingValue::Text(t) => SettingValue::Text(t.trim().to_owned()),
                other => other.clone(),
            };
            if matches!(&value, SettingValue::Text(t) if t.is_empty())
                && setting.kind != crate::SettingKind::Choice
            {
                continue;
            }
            setting
                .check_value(&value)
                .map_err(|m| plugin_error("save plugin settings", id, format!("{key} {m}")))?;
            checked.insert(key.clone(), value);
        }
        self.update_state(|state| {
            state.plugins.entry(id.to_owned()).or_default().settings = checked
        })
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read-modify-write of `plugins.toml`; the caller holds the lock.
    fn update_state<R>(&self, change: impl FnOnce(&mut StateFile) -> R) -> Result<R, Error> {
        let mut state = StateFile::read(&self.state_path)?;
        let result = change(&mut state);
        state.write(&self.state_path)?;
        Ok(result)
    }

    /// The directory of plugin `id`. Only a plain name is accepted: the id may come from a
    /// user interface or from a directory listing, and must not reach outside `plugins/`.
    fn dir_for(&self, id: &str) -> Result<PathBuf, Error> {
        let plain = !id.is_empty()
            && id.len() <= 255
            && !id.starts_with('.')
            && !id.ends_with([' ', '.'])
            && !id.contains(['/', '\\', ':'])
            && !id.chars().any(char::is_control);
        if plain {
            Ok(self.plugins_dir.join(id))
        } else {
            Err(Error {
                operation: "locate plugin",
                path: None,
                kind: ErrorKind::InvalidInput,
            })
        }
    }

    fn existing_dir(&self, id: &str) -> Result<PathBuf, Error> {
        let dir = self.dir_for(id)?;
        match fs::symlink_metadata(&dir) {
            Ok(_) => Ok(dir),
            Err(e) => Err(Error::io("find plugin", &dir, e)),
        }
    }

    fn describe(&self, id: &str, state: &StateFile) -> InstalledPlugin {
        let enabled = state.plugins.get(id).is_some_and(|s| s.enabled);
        let status = match self.read_installed(id) {
            Ok(found) => PluginStatus::Ready {
                manifest: Box::new(found.manifest),
                compatibility: found.compatibility,
                install: found.record,
            },
            Err(message) => PluginStatus::Broken { message },
        };
        InstalledPlugin {
            id: id.to_owned(),
            enabled,
            status,
        }
    }

    /// Reads and verifies the files of an installed plugin. The script is read, and checked
    /// against the recorded hash, only if the manifest is compatible: for any other manifest
    /// `entry` may not even be a file name.
    fn read_installed(&self, id: &str) -> Result<Found, String> {
        let dir = self.dir_for(id).map_err(|e| e.to_string())?;
        let manifest_path = dir.join(MANIFEST_FILE);
        let text =
            read_plain_file(&manifest_path, MAX_MANIFEST_BYTES).map_err(|e| e.to_string())?;
        let text = String::from_utf8(text).map_err(|_| "plugin.toml is not UTF-8 text")?;
        let manifest = Manifest::parse(&text).map_err(|e| e.to_string())?;
        if manifest.id != id {
            return Err(format!("plugin.toml names the plugin \"{}\"", manifest.id));
        }
        let record_path = dir.join(RECORD_FILE);
        let record = read_plain_file(&record_path, MAX_MANIFEST_BYTES)
            .map_err(|e| e.to_string())
            .and_then(|b| {
                serde_json::from_slice::<InstallRecord>(&b)
                    .map_err(|e| format!("{RECORD_FILE} is damaged: {e}"))
            })?;
        if record.version != manifest.version {
            return Err("plugin.toml was changed after installation".into());
        }
        let compatibility = manifest.compatibility(&app_version());
        let script = if compatibility.is_compatible() {
            let script = read_plain_file(&dir.join(&manifest.entry), MAX_SCRIPT_BYTES)
                .map_err(|e| e.to_string())?;
            if sha256_hex(&script) != record.sha256 {
                return Err(format!("{} was changed after installation", manifest.entry));
            }
            Some(script)
        } else {
            None
        };
        Ok(Found {
            manifest,
            compatibility,
            record,
            script,
        })
    }
}

struct Found {
    manifest: Manifest,
    compatibility: Compatibility,
    record: InstallRecord,
    script: Option<Vec<u8>>,
}

pub(crate) fn not_compatible(reasons: &[crate::Reason]) -> String {
    let reasons: Vec<String> = reasons.iter().map(ToString::to_string).collect();
    format!("not compatible: {}", reasons.join("; "))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Reads a regular file of at most `max` bytes. A symbolic link is refused rather than
/// followed: a folder offered for installation must not make the app read files elsewhere.
pub(crate) fn read_plain_file(path: &Path, max: usize) -> Result<Vec<u8>, Error> {
    let invalid = |message: &str| Error::invalid_data("read plugin file", path, message.to_owned());
    let meta = fs::symlink_metadata(path).map_err(|e| Error::io("read plugin file", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(invalid("symbolic links are not followed"));
    }
    if !meta.is_file() {
        return Err(invalid("not a regular file"));
    }
    if meta.len() > max as u64 {
        return Err(invalid(&format!("larger than {max} bytes")));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(max as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| Error::io("read plugin file", path, e))?;
    if bytes.len() > max {
        return Err(invalid(&format!("larger than {max} bytes")));
    }
    Ok(bytes)
}
