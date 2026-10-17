//! The FerroLibri plugin contract: manifest, search result types, validation.
//!
//! It fixes what a plugin may declare (`plugin.toml`) and what it may return (JSON), and rejects
//! everything else. Everything that comes from a plugin is untrusted. The reference document is
//! `docs/PLUGIN_API.md`. The crate also holds the HTTP layer through which all network access of
//! the app and of plugins goes ([`http`]); and the QuickJS runtime that runs plugins ([`PluginRuntime`]); and the store of installed plugins
//! ([`PluginStore`]).

mod domain;
mod download;
mod host_error;
pub mod http;
pub mod limits;
mod manifest;
mod remote;
mod runtime;
mod search;
mod store;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod text;

pub use domain::DomainRule;
pub use download::{DownloadSpec, parse_download_spec};
pub use host_error::HostErrorKind;
pub use http::{
    Access, AllowList, AllowListError, FetchResponse, HostError, Http, HttpConfig, HttpSession,
    Identity, NameResolver, ResponseHead, ResponseStream, SystemResolver, is_forbidden_ip,
};
pub use manifest::{
    Compatibility, Limits, Manifest, ManifestError, Network, Reason, Setting, SettingKind,
    SettingValue,
};
pub use remote::{
    AddressError, GithubEndpoint, GithubRepo, Release, ReleaseCheck, ReleaseList, ReleaseOptions,
    RemoteError, RemoteInstaller, RemoteSource,
};
pub use runtime::{LoadedPlugin, LogLevel, LogRecord, LogSink, PluginFunction, PluginRuntime};
pub use search::{
    Archive, Offer, ParsedPage, ResponseError, SearchHit, SearchPage, SearchQuery, Warning,
    merge_details, parse_search_page,
};
pub use store::{
    Downgrade, Inspection, InstallChange, InstallRecord, InstallSource, InstalledPlugin,
    Permissions, PluginStatus, PluginStore,
};

use ferrolibri_core::{Error, ErrorKind};

/// Plugin API major versions this build of the app can run.
pub const SUPPORTED_API_VERSIONS: &[u32] = &[1];

/// Version of the app, as compared with `min_app_version` (the workspace version).
pub fn app_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid semver")
}

/// A plugin-caused failure as a core error (`ErrorKind::Plugin`).
pub fn plugin_error(operation: &'static str, plugin_id: &str, message: impl Into<String>) -> Error {
    Error {
        operation,
        path: None,
        kind: ErrorKind::Plugin {
            id: plugin_id.to_owned(),
            message: message.into(),
        },
    }
}

impl ResponseError {
    /// Converts an invalid plugin response into a core error attributed to the plugin.
    pub fn into_core(self, operation: &'static str, plugin_id: &str) -> Error {
        plugin_error(operation, plugin_id, self.to_string())
    }
}
