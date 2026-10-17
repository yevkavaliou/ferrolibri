//! `plugins.toml`: which installed plugins are enabled and the values of their settings (D11).

use std::{collections::BTreeMap, fs, io, path::Path};

use ferrolibri_core::{Error, config::write_atomic};
use serde::{Deserialize, Serialize};

use crate::{Manifest, Setting, SettingKind, SettingValue};

#[derive(Default, Deserialize, Serialize)]
pub(super) struct StateFile {
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginState>,
}

#[derive(Default, Deserialize, Serialize)]
pub(super) struct PluginState {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub settings: BTreeMap<String, SettingValue>,
}

impl StateFile {
    /// A missing file is an empty state; a damaged one is an error, so it is never overwritten
    /// by a state that forgot everything.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::io("read plugin state", path, e)),
        };
        toml::from_str(&text).map_err(|e| Error::invalid_data("parse plugin state", path, e))
    }

    pub fn write(&self, path: &Path) -> Result<(), Error> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| Error::invalid_data("serialize plugin state", path, e))?;
        write_atomic(path, text.as_bytes())
    }
}

/// Which setting stores a value: it is kept only if the manifest declares the key and the value
/// still suits it.
fn declared<'a>(manifest: &'a Manifest, key: &str, value: &SettingValue) -> Option<&'a Setting> {
    manifest
        .settings
        .iter()
        .find(|s| s.key == key)
        .filter(|s| s.check_value(value).is_ok())
}

/// The stored values that a plugin run may see.
pub(super) fn usable(
    manifest: &Manifest,
    stored: &BTreeMap<String, SettingValue>,
) -> BTreeMap<String, SettingValue> {
    stored
        .iter()
        .filter(|(key, value)| declared(manifest, key, value).is_some())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The stored values that survive a change of the installed version. An address the user saved
/// for a setting that did not open the network must not start doing so just because the new
/// version says `network = true`: the user never agreed to that.
pub(super) fn carried(
    old: Option<&Manifest>,
    new: &Manifest,
    stored: BTreeMap<String, SettingValue>,
) -> BTreeMap<String, SettingValue> {
    stored
        .into_iter()
        .filter(|(key, value)| {
            let Some(setting) = declared(new, key, value) else {
                return false;
            };
            let was_network = old
                .and_then(|m| m.settings.iter().find(|s| s.key == *key))
                .is_some_and(|s| s.kind == SettingKind::Url && s.network);
            !setting.network || was_network
        })
        .collect()
}
