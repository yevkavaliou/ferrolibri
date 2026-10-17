//! Installing a plugin from an address (D10): a GitHub repository, whose releases are listed
//! through the API, or a `plugin.toml` at any address with the script next to it.
//!
//! The steps are the same as for a folder. [`RemoteInstaller`] downloads the files, checks them
//! and returns an [`Inspection`](crate::Inspection); [`PluginStore::install`](crate::PluginStore)
//! then copies it into the store. Nothing is written to the plugin directories before that,
//! and everything that comes from the network is untrusted.

mod error;
mod github;
mod source;
#[cfg(test)]
mod tests;

pub use error::RemoteError;
pub use github::{
    GithubEndpoint, Release, ReleaseCheck, ReleaseList, ReleaseOptions, RemoteInstaller,
};
pub use source::{AddressError, GithubRepo, RemoteSource};
