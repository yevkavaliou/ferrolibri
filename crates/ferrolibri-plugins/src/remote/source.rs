//! Reading the address the user typed.

use std::fmt;

use url::Url;

use crate::limits::MAX_URL_CHARS;

/// The manifest's file name; a link that ends with it is a link to a manifest.
const MANIFEST_NAME: &str = "plugin.toml";
const MAX_OWNER_CHARS: usize = 39;
const MAX_REPO_CHARS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubRepo {
    pub owner: String,
    pub name: String,
}

impl fmt::Display for GithubRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Where the plugin is to be fetched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteSource {
    /// The releases of a repository on github.com.
    Github(GithubRepo),
    /// A `plugin.toml` at this address; the script is looked for in the same directory.
    Manifest(Url),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    /// Not an `http(s)` address, too long, or with a user name or password.
    NotAnAddress(String),
    /// An address of a kind that is not understood.
    Unsupported,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("enter an address")?,
            Self::NotAnAddress(why) => write!(f, "not a usable address: {why}")?,
            Self::Unsupported => f.write_str("the address is not recognised")?,
        }
        f.write_str(
            "; expected https://github.com/owner/repo or a direct link to a plugin.toml file",
        )
    }
}

impl std::error::Error for AddressError {}

impl RemoteSource {
    /// `https://github.com/owner/repo` (`www.` and a `.git` suffix are fine, as is anything
    /// after the repository: `/releases`, `/tree/main`, a query) is a repository. A link whose
    /// last part is `plugin.toml` is a manifest, wherever it is. Anything else is an error.
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(AddressError::Empty);
        }
        if input.chars().count() > MAX_URL_CHARS {
            return Err(AddressError::NotAnAddress("it is too long".into()));
        }
        if input.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(AddressError::NotAnAddress("it contains spaces".into()));
        }
        let mut url = Url::parse(input).map_err(|_| AddressError::Unsupported)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AddressError::NotAnAddress(
                "only http and https are supported".into(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(AddressError::NotAnAddress(
                "it contains a user name or password".into(),
            ));
        }
        let host = url.host_str().map(str::to_ascii_lowercase);
        url.set_fragment(None);
        let segments: Vec<&str> = url
            .path_segments()
            .map(|s| s.filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        if segments
            .last()
            .is_some_and(|s| s.eq_ignore_ascii_case(MANIFEST_NAME))
        {
            return Ok(Self::Manifest(url));
        }
        if matches!(host.as_deref(), Some("github.com" | "www.github.com"))
            && url.scheme() == "https"
        {
            return github_repo(&segments).map(Self::Github);
        }
        Err(AddressError::Unsupported)
    }
}

fn github_repo(segments: &[&str]) -> Result<GithubRepo, AddressError> {
    let [owner, name, ..] = segments else {
        return Err(AddressError::Unsupported);
    };
    let name = name.strip_suffix(".git").unwrap_or(name);
    let plain = |part: &str, max: usize| {
        !part.is_empty()
            && part.chars().count() <= max
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !plain(owner, MAX_OWNER_CHARS) || !plain(name, MAX_REPO_CHARS) {
        return Err(AddressError::Unsupported);
    }
    Ok(GithubRepo {
        owner: (*owner).to_owned(),
        name: name.to_owned(),
    })
}
