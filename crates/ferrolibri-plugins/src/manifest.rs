use std::{collections::HashSet, fmt, time::Duration};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{SUPPORTED_API_VERSIONS, domain::DomainRule, limits::*, text};

/// `plugin.toml`. Unknown fields are ignored; see [`Manifest::compatibility`] for validation.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    /// Semver of the plugin itself.
    pub version: String,
    /// Major version of the plugin protocol.
    pub api_version: u32,
    #[serde(default)]
    pub min_app_version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Kinds of plugin; v1 knows only `"search"`. Unknown entries are ignored.
    pub provides: Vec<String>,
    #[serde(default = "default_entry")]
    pub entry: String,
    /// SHA-256 of the entry file, 64 hex digits.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Content kinds claimed by the author. Informational, never verified.
    #[serde(default)]
    pub content: Vec<String>,
    #[serde(default)]
    pub search_hint: Option<String>,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub settings: Vec<Setting>,
}

fn default_entry() -> String {
    "plugin.js".into()
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Network {
    /// Exact hosts or `*.example.org`; see [`DomainRule`].
    #[serde(default)]
    pub domains: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Limits {
    /// Total time of one plugin call, 5–60 seconds (default 30).
    #[serde(default)]
    pub call_timeout_s: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    String,
    Url,
    Bool,
    Choice,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Text(String),
}

/// A setting the app renders in the plugin's settings form.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Setting {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: SettingKind,
    pub label: String,
    /// `url` only: the host of the entered address (and its subdomains) is allowed (D4).
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub default: Option<SettingValue>,
    /// `choice` only: the allowed values, shown as they are.
    #[serde(default)]
    pub options: Vec<String>,
}

/// Why a manifest cannot be loaded at all (as opposed to being incompatible).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    TooLarge {
        bytes: usize,
    },
    /// Not TOML, or a required field is missing or has the wrong type.
    Invalid(String),
    /// The manifest targets a protocol version this app does not know; its other fields
    /// may be laid out differently, so they are not read.
    UnsupportedApiVersion {
        found: i64,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                f,
                "plugin.toml is too large ({bytes} bytes, at most {MAX_MANIFEST_BYTES})"
            ),
            Self::Invalid(message) => write!(f, "plugin.toml is invalid: {message}"),
            Self::UnsupportedApiVersion { found } => {
                write!(f, "{}", unsupported_api_message(*found))
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// A reason shown to the user why a plugin cannot be installed or used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    UnsupportedApiVersion {
        found: i64,
    },
    AppTooOld {
        required: Version,
        current: Version,
    },
    /// `provides` does not contain `"search"`, the only kind v1 can run.
    NoSearch,
    Invalid {
        field: String,
        message: String,
    },
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedApiVersion { found } => f.write_str(&unsupported_api_message(*found)),
            Self::AppTooOld { required, current } => write!(
                f,
                "requires FerroLibri {required} or newer (this is {current})"
            ),
            Self::NoSearch => f.write_str("the plugin does not provide \"search\""),
            Self::Invalid { field, message } => write!(f, "{field} {message}"),
        }
    }
}

fn unsupported_api_message(found: i64) -> String {
    let supported: Vec<String> = SUPPORTED_API_VERSIONS.iter().map(u32::to_string).collect();
    format!(
        "plugin API version {found} is not supported (supported: {})",
        supported.join(", ")
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compatibility {
    Compatible,
    /// Never empty.
    Incompatible(Vec<Reason>),
}

impl Compatibility {
    pub fn is_compatible(&self) -> bool {
        matches!(self, Self::Compatible)
    }

    pub fn reasons(&self) -> &[Reason] {
        match self {
            Self::Compatible => &[],
            Self::Incompatible(reasons) => reasons,
        }
    }
}

impl Manifest {
    /// Reads a manifest. Only structure is checked here; call [`Manifest::compatibility`]
    /// before using the plugin.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge { bytes: text.len() });
        }
        let table: toml::Table = toml::from_str(text)
            .map_err(|e| ManifestError::Invalid(e.message().trim().to_owned()))?;
        if let Some(toml::Value::Integer(found)) = table.get("api_version")
            && !is_supported_api(*found)
        {
            return Err(ManifestError::UnsupportedApiVersion { found: *found });
        }
        table
            .try_into()
            .map_err(|e: toml::de::Error| ManifestError::Invalid(e.message().trim().to_owned()))
    }

    /// Checks the manifest against this app: protocol version, `min_app_version`, `provides`,
    /// and every field limit of the contract. Collects all reasons, not just the first.
    pub fn compatibility(&self, app_version: &Version) -> Compatibility {
        let mut reasons = Vec::new();
        if !is_supported_api(i64::from(self.api_version)) {
            reasons.push(Reason::UnsupportedApiVersion {
                found: i64::from(self.api_version),
            });
        }
        if let Some(required) = &self.min_app_version {
            match Version::parse(required) {
                Ok(required) if required > *app_version => reasons.push(Reason::AppTooOld {
                    required,
                    current: app_version.clone(),
                }),
                Ok(_) => {}
                Err(e) => invalid(
                    &mut reasons,
                    "min_app_version",
                    format!("is not semver ({e})"),
                ),
            }
        }
        if !self.provides.iter().any(|kind| kind == "search") {
            reasons.push(Reason::NoSearch);
        }
        self.check_fields(&mut reasons);
        if reasons.is_empty() {
            Compatibility::Compatible
        } else {
            Compatibility::Incompatible(reasons)
        }
    }

    /// The time budget of one plugin call.
    pub fn call_timeout(&self) -> Duration {
        Duration::from_secs(u64::from(
            self.limits.call_timeout_s.unwrap_or(DEFAULT_CALL_TIMEOUT_S),
        ))
    }

    /// Parsed `network.domains`; entries that fail validation are skipped, so call this
    /// for a manifest that passed [`Manifest::compatibility`].
    pub fn domain_rules(&self) -> Vec<DomainRule> {
        self.network
            .domains
            .iter()
            .filter_map(|d| DomainRule::parse(d).ok())
            .collect()
    }

    fn check_fields(&self, reasons: &mut Vec<Reason>) {
        let mut check = |field: &str, result: Result<(), String>| {
            if let Err(message) = result {
                invalid(reasons, field, message);
            }
        };

        check("id", check_id(&self.id));
        check("name", text::line(&self.name, MAX_NAME_CHARS));
        check("version", check_semver(&self.version));
        optional_line(&mut check, "author", &self.author, MAX_AUTHOR_CHARS);
        optional_line(&mut check, "license", &self.license, MAX_LICENSE_CHARS);
        optional_line(
            &mut check,
            "description",
            &self.description,
            MAX_DESCRIPTION_CHARS,
        );
        optional_line(
            &mut check,
            "search_hint",
            &self.search_hint,
            MAX_SEARCH_HINT_CHARS,
        );
        if let Some(homepage) = &self.homepage {
            check(
                "homepage",
                text::http_url(homepage, MAX_URL_CHARS).map(drop),
            );
        }
        check("entry", check_entry(&self.entry));
        if let Some(sha) = &self.sha256 {
            let ok = sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit());
            check(
                "sha256",
                if ok {
                    Ok(())
                } else {
                    Err("must be 64 hexadecimal digits".into())
                },
            );
        }

        check(
            "provides",
            list_limit(self.provides.len(), MAX_PROVIDES, "entries"),
        );
        for (i, kind) in self.provides.iter().enumerate() {
            check(
                &format!("provides[{i}]"),
                text::line(kind, MAX_PROVIDES_CHARS),
            );
        }
        check(
            "content",
            list_limit(self.content.len(), MAX_CONTENT_TAGS, "entries"),
        );
        for (i, tag) in self.content.iter().enumerate() {
            check(
                &format!("content[{i}]"),
                text::line(tag, MAX_CONTENT_TAG_CHARS),
            );
        }

        check(
            "network.domains",
            list_limit(self.network.domains.len(), MAX_DOMAINS, "domains"),
        );
        for (i, domain) in self.network.domains.iter().enumerate() {
            check(
                &format!("network.domains[{i}]"),
                DomainRule::parse(domain).map(drop),
            );
        }

        if let Some(seconds) = self.limits.call_timeout_s {
            let ok = (MIN_CALL_TIMEOUT_S..=MAX_CALL_TIMEOUT_S).contains(&seconds);
            check(
                "limits.call_timeout_s",
                if ok {
                    Ok(())
                } else {
                    Err(format!(
                        "must be between {MIN_CALL_TIMEOUT_S} and {MAX_CALL_TIMEOUT_S} (got {seconds})"
                    ))
                },
            );
        }

        check(
            "settings",
            list_limit(self.settings.len(), MAX_SETTINGS, "settings"),
        );
        let mut keys = HashSet::new();
        for (i, setting) in self.settings.iter().enumerate() {
            let field = format!("settings[{i}]");
            if !keys.insert(setting.key.as_str()) {
                check(&format!("{field}.key"), Err("is used twice".into()));
            }
            setting.check(&field, &mut check);
        }
    }
}

impl Setting {
    fn check(&self, field: &str, check: &mut impl FnMut(&str, Result<(), String>)) {
        let key = |name: &str| format!("{field}.{name}");
        check(&key("key"), check_setting_key(&self.key));
        check(
            &key("label"),
            text::line(&self.label, MAX_SETTING_LABEL_CHARS),
        );
        if self.network && self.kind != SettingKind::Url {
            check(
                &key("network"),
                Err("is allowed only for settings of type \"url\"".into()),
            );
        }
        if self.kind == SettingKind::Choice {
            let count = list_limit(self.options.len(), MAX_SETTING_OPTIONS, "options");
            check(
                &key("options"),
                if self.options.is_empty() {
                    Err("must list the choices".into())
                } else {
                    count
                },
            );
            let mut seen = HashSet::new();
            for (i, option) in self.options.iter().enumerate() {
                check(
                    &format!("{field}.options[{i}]"),
                    text::line(option, MAX_SETTING_OPTION_CHARS),
                );
                if !seen.insert(option.as_str()) {
                    check(
                        &format!("{field}.options[{i}]"),
                        Err("is used twice".into()),
                    );
                }
            }
        } else if !self.options.is_empty() {
            check(
                &key("options"),
                Err("is allowed only for settings of type \"choice\"".into()),
            );
        }

        if let Some(default) = &self.default {
            check(&key("default"), self.check_value(default));
        }
    }

    /// Whether `value` suits this setting: the type, the length, the address, the choice.
    /// Used for `default` and for what the user stores.
    pub(crate) fn check_value(&self, value: &SettingValue) -> Result<(), String> {
        match (self.kind, value) {
            (SettingKind::Bool, SettingValue::Bool(_)) => Ok(()),
            (SettingKind::Bool, _) => Err("must be true or false".into()),
            (_, SettingValue::Bool(_)) => Err("must be a string".into()),
            (SettingKind::String, SettingValue::Text(v)) => {
                text::multiline(v, MAX_SETTING_TEXT_CHARS)
            }
            (SettingKind::Url, SettingValue::Text(v)) => text::http_url(v, MAX_URL_CHARS).map(drop),
            (SettingKind::Choice, SettingValue::Text(v)) => {
                if self.options.contains(v) {
                    Ok(())
                } else {
                    Err("must be one of the options".into())
                }
            }
        }
    }
}

fn is_supported_api(version: i64) -> bool {
    u32::try_from(version).is_ok_and(|v| SUPPORTED_API_VERSIONS.contains(&v))
}

fn invalid(reasons: &mut Vec<Reason>, field: &str, message: String) {
    reasons.push(Reason::Invalid {
        field: field.to_owned(),
        message,
    });
}

fn optional_line(
    check: &mut impl FnMut(&str, Result<(), String>),
    field: &str,
    value: &Option<String>,
    max: usize,
) {
    if let Some(value) = value {
        check(field, text::line(value, max));
    }
}

fn list_limit(count: usize, max: usize, what: &str) -> Result<(), String> {
    if count > max {
        return Err(format!("may have at most {max} {what} (got {count})"));
    }
    Ok(())
}

/// `[a-z0-9][a-z0-9-]{1,39}`
pub(crate) fn check_id(id: &str) -> Result<(), String> {
    let length_ok = (MIN_ID_CHARS..=MAX_ID_CHARS).contains(&id.len());
    let mut bytes = id.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    let rest_ok = bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if length_ok && first_ok && rest_ok {
        Ok(())
    } else {
        Err(format!(
            "must be {MIN_ID_CHARS}-{MAX_ID_CHARS} characters of a-z, 0-9 and '-', not starting with '-'"
        ))
    }
}

fn check_semver(value: &str) -> Result<(), String> {
    Version::parse(value)
        .map(drop)
        .map_err(|e| format!("is not semver ({e})"))
}

/// A plain file name ending in `.js`: v1 plugins are two files, no sub-folders.
pub(crate) fn check_entry(entry: &str) -> Result<(), String> {
    text::line(entry, MAX_ENTRY_CHARS)?;
    let plain = !entry.starts_with('.')
        && !entry.contains(['/', '\\', ':'])
        && entry.len() > ".js".len()
        && entry.ends_with(".js");
    if plain {
        Ok(())
    } else {
        Err("must be a file name ending in .js, without a path".into())
    }
}

/// `[a-z][a-z0-9_]{0,39}`
fn check_setting_key(key: &str) -> Result<(), String> {
    let mut bytes = key.bytes();
    let first_ok = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
    let rest_ok = bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if first_ok && rest_ok && key.len() <= MAX_SETTING_KEY_CHARS {
        Ok(())
    } else {
        Err(format!(
            "must be at most {MAX_SETTING_KEY_CHARS} characters of a-z, 0-9 and '_', starting with a letter"
        ))
    }
}

#[cfg(test)]
mod tests;
