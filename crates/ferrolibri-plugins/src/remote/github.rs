//! Fetching a plugin from the releases of a GitHub repository, or from a `plugin.toml` link.

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicUsize, Ordering},
};

use ferrolibri_core::converter::CancellationToken;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use super::{RemoteError, source::GithubRepo};
use crate::{
    AllowList, Compatibility, HostError, HostErrorKind, Http, Identity, Inspection, InstallChange,
    InstallSource, Manifest, PluginStore, ResponseHead,
    limits::{
        MAX_MANIFEST_BYTES, MAX_RELEASE_NAME_CHARS, MAX_RELEASE_TAG_CHARS, MAX_RELEASES_LISTED,
        MAX_SCRIPT_BYTES, RELEASE_CHECK_WORKERS,
    },
    store::{sha256_hex, store_compatibility},
};

const MANIFEST_ASSET: &str = "plugin.toml";
const API_ACCEPT: &str = "application/vnd.github+json";
const MAX_DATE_CHARS: usize = 40;

/// Where the API is and which hosts the downloads may come from.
#[derive(Clone, Debug)]
pub struct GithubEndpoint {
    api_base: String,
    pub(super) allow: AllowList,
}

impl GithubEndpoint {
    /// `api.github.com`; the files come from `github.com` and, after a redirect, from
    /// `objects.githubusercontent.com` (and other `githubusercontent.com` hosts).
    pub fn github() -> Self {
        let mut allow = AllowList::new();
        for domain in [
            "api.github.com",
            "github.com",
            "objects.githubusercontent.com",
            "*.githubusercontent.com",
        ] {
            allow
                .allow_domain(domain)
                .expect("the built-in domains are valid");
        }
        Self {
            api_base: "https://api.github.com".into(),
            allow,
        }
    }

    /// Another server that speaks the same API (tests use a local one).
    pub fn custom(api_base: &str, allow: AllowList) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').to_owned(),
            allow,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReleaseOptions {
    /// Prereleases are hidden unless this is set.
    pub include_prereleases: bool,
}

/// What the checking of one release found.
#[derive(Clone, Debug)]
pub enum ReleaseCheck {
    /// The manifest was read. If it is compatible, the script is attached to the release too.
    Ready {
        manifest: Box<Manifest>,
        compatibility: Compatibility,
        /// What installing this release would do to the plugin that is installed now.
        change: InstallChange,
    },
    /// The release cannot be installed and the message says why: the manifest could not be
    /// downloaded or read, or the script is not attached.
    Failed { message: String },
}

#[derive(Clone, Debug)]
struct Asset {
    name: String,
    url: String,
    digest: Option<String>,
}

/// One release of a repository that carries a `plugin.toml`.
#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub name: Option<String>,
    /// ISO 8601 as GitHub gave it.
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub check: ReleaseCheck,
    repo: GithubRepo,
    manifest: Arc<Vec<u8>>,
    entry: Option<Asset>,
}

#[derive(Clone, Debug)]
pub struct ReleaseList {
    /// Newest first, as the API lists them. Releases without a `plugin.toml` are not here.
    pub releases: Vec<Release>,
    /// Requests left in the current hour, if GitHub said.
    pub rate_limit_remaining: Option<u32>,
}

/// Downloads plugins; see the [module documentation](super).
#[derive(Clone, Debug)]
pub struct RemoteInstaller {
    http: Http,
    endpoint: GithubEndpoint,
}

impl RemoteInstaller {
    pub fn new(http: Http) -> Self {
        Self::with_endpoint(http, GithubEndpoint::github())
    }

    pub fn with_endpoint(http: Http, endpoint: GithubEndpoint) -> Self {
        Self { http, endpoint }
    }

    /// The latest releases of `repo` that have a `plugin.toml`, each with its manifest read
    /// and checked for compatibility. One request goes to the API; the manifests are fetched
    /// a few at a time and do not count against the API's limit.
    pub fn releases(
        &self,
        store: &PluginStore,
        repo: &GithubRepo,
        options: ReleaseOptions,
        cancel: &CancellationToken,
    ) -> Result<ReleaseList, RemoteError> {
        let url = format!(
            "{}/repos/{}/{}/releases?per_page={MAX_RELEASES_LISTED}",
            self.endpoint.api_base, repo.owner, repo.name
        );
        let session = self.session(&self.endpoint.allow, cancel);
        let response = session.fetch(&url, &[("Accept", API_ACCEPT)])?;
        if !response.head.is_success() {
            return Err(status_error(
                &response.head,
                &format!("the repository {repo}"),
            ));
        }
        let rate_limit_remaining = response
            .head
            .header("x-ratelimit-remaining")
            .and_then(|v| v.trim().parse().ok());
        let listed = parse_releases(&response.body, options)?;
        let checked = self.check_all(store, repo, listed, cancel)?;
        Ok(ReleaseList {
            releases: checked,
            rate_limit_remaining,
        })
    }

    /// Downloads the script of a release whose manifest is compatible, checks it against the
    /// hashes (the asset's `digest` when GitHub gave one, and `sha256` of the manifest) and
    /// returns what [`PluginStore::install`] takes. The files are kept for as long as the
    /// inspection lives; nothing is installed.
    pub fn inspect_release(
        &self,
        store: &PluginStore,
        release: &Release,
        cancel: &CancellationToken,
    ) -> Result<Inspection, RemoteError> {
        let (manifest, compatibility) = match &release.check {
            ReleaseCheck::Ready {
                manifest,
                compatibility,
                ..
            } => (manifest, compatibility),
            ReleaseCheck::Failed { message } => {
                return Err(RemoteError::invalid(
                    format!("release {}", release.tag),
                    message.clone(),
                ));
            }
        };
        let script = match (&release.entry, compatibility.is_compatible()) {
            (Some(asset), true) => {
                Some(self.download_script(asset, manifest, &self.endpoint.allow, cancel)?)
            }
            _ => None,
        };
        let source = InstallSource::Github {
            repo: release.repo.to_string(),
            tag: release.tag.clone(),
        };
        Ok(store.inspect_downloaded(
            &release.manifest,
            script
                .as_ref()
                .map(|s| (manifest.entry.as_str(), s.as_slice())),
            source,
        )?)
    }

    /// Fetches `plugin.toml` from `url` and the script from the same directory, and inspects
    /// them. Only the host of `url` may be reached (and the GitHub hosts, for a link into a
    /// release).
    pub fn inspect_manifest_url(
        &self,
        store: &PluginStore,
        url: &Url,
        cancel: &CancellationToken,
    ) -> Result<Inspection, RemoteError> {
        let mut allow = self.endpoint.allow.clone();
        allow
            .allow_entered_url(url.as_str())
            .map_err(|e| RemoteError::invalid("the address", e.to_string()))?;
        let bytes = self.download(
            url.as_str(),
            &allow,
            MAX_MANIFEST_BYTES,
            MANIFEST_ASSET,
            cancel,
        )?;
        let manifest = parse_manifest(&bytes)?;
        let script = if store_compatibility(&manifest).is_compatible() {
            let script_url = url
                .join(&manifest.entry)
                .map_err(|e| RemoteError::invalid(manifest.entry.clone(), e.to_string()))?;
            let asset = Asset {
                name: manifest.entry.clone(),
                url: script_url.into(),
                digest: None,
            };
            Some(self.download_script(&asset, &manifest, &allow, cancel)?)
        } else {
            None
        };
        let source = InstallSource::Url {
            url: url.as_str().to_owned(),
        };
        Ok(store.inspect_downloaded(
            &bytes,
            script
                .as_ref()
                .map(|s| (manifest.entry.as_str(), s.as_slice())),
            source,
        )?)
    }

    fn session(&self, allow: &AllowList, cancel: &CancellationToken) -> crate::HttpSession {
        self.http
            .session(Identity::App, allow.clone(), cancel.clone())
            .with_request_budget(1)
    }

    /// One file, at most `max` bytes. Any status but 2xx is an error.
    fn download(
        &self,
        url: &str,
        allow: &AllowList,
        max: usize,
        what: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, RemoteError> {
        let session = self.session(allow, cancel);
        let mut stream = session.open(url, &[], max as u64)?;
        if !stream.head().is_success() {
            return Err(status_error(stream.head(), what));
        }
        Ok(stream.read_to_end()?)
    }

    fn download_script(
        &self,
        asset: &Asset,
        manifest: &Manifest,
        allow: &AllowList,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, RemoteError> {
        let script = self.download(&asset.url, allow, MAX_SCRIPT_BYTES, &asset.name, cancel)?;
        verify_digest(&asset.name, &script, asset.digest.as_deref())?;
        if let Some(expected) = &manifest.sha256
            && !expected.eq_ignore_ascii_case(&sha256_hex(&script))
        {
            return Err(RemoteError::HashMismatch {
                file: asset.name.clone(),
                announced_by: "plugin.toml",
            });
        }
        Ok(script)
    }

    /// Reads the manifest of every listed release, a few at a time. Running out of the API's
    /// limit and cancellation end the whole thing; any other failure marks only its release.
    fn check_all(
        &self,
        store: &PluginStore,
        repo: &GithubRepo,
        listed: Vec<Listed>,
        cancel: &CancellationToken,
    ) -> Result<Vec<Release>, RemoteError> {
        let total = listed.len();
        let next = AtomicUsize::new(0);
        let slots: Mutex<Vec<Option<Result<Release, RemoteError>>>> =
            Mutex::new((0..total).map(|_| None).collect());
        std::thread::scope(|scope| {
            for _ in 0..RELEASE_CHECK_WORKERS.min(total) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = listed.get(index) else {
                            break;
                        };
                        let result = self.check_one(store, repo, item, cancel);
                        let fatal = result.is_err();
                        slots.lock().unwrap_or_else(PoisonError::into_inner)[index] = Some(result);
                        if fatal {
                            // The rest would only hit the same wall.
                            next.store(total, Ordering::Relaxed);
                            break;
                        }
                    }
                });
            }
        });
        let mut releases = Vec::with_capacity(total);
        for slot in slots.into_inner().unwrap_or_else(PoisonError::into_inner) {
            match slot {
                Some(Ok(release)) => releases.push(release),
                Some(Err(e)) => return Err(e),
                None => {}
            }
        }
        if cancel.is_cancelled() {
            return Err(HostError::new(HostErrorKind::Cancelled, "cancelled").into());
        }
        Ok(releases)
    }

    fn check_one(
        &self,
        store: &PluginStore,
        repo: &GithubRepo,
        item: &Listed,
        cancel: &CancellationToken,
    ) -> Result<Release, RemoteError> {
        let mut release = Release {
            tag: item.tag.clone(),
            name: item.name.clone(),
            published_at: item.published_at.clone(),
            prerelease: item.prerelease,
            check: ReleaseCheck::Failed {
                message: String::new(),
            },
            repo: repo.clone(),
            manifest: Arc::default(),
            entry: None,
        };
        let outcome = self
            .download(
                &item.manifest.url,
                &self.endpoint.allow,
                MAX_MANIFEST_BYTES,
                MANIFEST_ASSET,
                cancel,
            )
            .and_then(|bytes| {
                verify_digest(MANIFEST_ASSET, &bytes, item.manifest.digest.as_deref())?;
                let manifest = parse_manifest(&bytes)?;
                Ok((bytes, manifest))
            });
        let (bytes, manifest) = match outcome {
            Ok(found) => found,
            Err(e @ RemoteError::RateLimited { .. }) => return Err(e),
            Err(e) if e.is_cancelled() => return Err(e),
            Err(e) => {
                release.check = ReleaseCheck::Failed {
                    message: e.to_string(),
                };
                return Ok(release);
            }
        };
        let compatibility = store_compatibility(&manifest);
        let entry = item
            .assets
            .iter()
            .find(|asset| asset.name == manifest.entry)
            .cloned();
        release.check = if compatibility.is_compatible() && entry.is_none() {
            ReleaseCheck::Failed {
                message: format!("the release has no {} file", manifest.entry),
            }
        } else {
            let change = if compatibility.is_compatible() {
                store.change_for(&manifest)
            } else {
                InstallChange::New
            };
            release.entry = entry;
            ReleaseCheck::Ready {
                manifest: Box::new(manifest),
                compatibility,
                change,
            }
        };
        release.manifest = Arc::new(bytes);
        Ok(release)
    }
}

/// A release from the API's page that is worth checking.
struct Listed {
    tag: String,
    name: Option<String>,
    published_at: Option<String>,
    prerelease: bool,
    manifest: Asset,
    /// All the files attached to the release.
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Value>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

/// Picks the releases to check out of the API's answer. A release that does not parse is
/// skipped, not a reason to give up on the others.
fn parse_releases(body: &[u8], options: ReleaseOptions) -> Result<Vec<Listed>, RemoteError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| RemoteError::invalid("the list of releases", e.to_string()))?;
    let Value::Array(items) = value else {
        return Err(RemoteError::invalid(
            "the list of releases",
            "expected a list",
        ));
    };
    let mut listed = Vec::new();
    for item in items {
        let Ok(release) = serde_json::from_value::<ApiRelease>(item) else {
            continue;
        };
        if release.draft
            || (release.prerelease && !options.include_prereleases)
            || !plain_text(&release.tag_name, MAX_RELEASE_TAG_CHARS)
        {
            continue;
        }
        let assets: Vec<Asset> = release
            .assets
            .into_iter()
            .filter_map(|a| serde_json::from_value::<ApiAsset>(a).ok())
            .filter(|a| a.state.as_deref().is_none_or(|s| s == "uploaded"))
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                digest: a.digest,
            })
            .collect();
        let Some(manifest) = assets.iter().find(|a| a.name == MANIFEST_ASSET).cloned() else {
            continue;
        };
        listed.push(Listed {
            tag: release.tag_name,
            name: release
                .name
                .filter(|n| plain_text(n, MAX_RELEASE_NAME_CHARS)),
            published_at: release
                .published_at
                .filter(|d| plain_text(d, MAX_DATE_CHARS)),
            prerelease: release.prerelease,
            manifest,
            assets,
        });
        if listed.len() == MAX_RELEASES_LISTED {
            break;
        }
    }
    Ok(listed)
}

fn plain_text(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, RemoteError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| RemoteError::invalid(MANIFEST_ASSET, "not UTF-8 text"))?;
    Manifest::parse(text).map_err(|e| RemoteError::invalid(MANIFEST_ASSET, e.to_string()))
}

/// Checks `bytes` against the `digest` GitHub reports for an asset (`sha256:<hex>`). A digest
/// of another kind, or none at all, is not a reason to refuse: the field is not guaranteed.
fn verify_digest(file: &str, bytes: &[u8], digest: Option<&str>) -> Result<(), RemoteError> {
    let Some(expected) = digest
        .and_then(|d| d.trim().strip_prefix("sha256:"))
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return Ok(());
    };
    if expected.eq_ignore_ascii_case(&sha256_hex(bytes)) {
        Ok(())
    } else {
        Err(RemoteError::HashMismatch {
            file: file.to_owned(),
            announced_by: "GitHub",
        })
    }
}

/// A response that is not a success, as an error. GitHub says the limit is used up with 429,
/// or with 403 and `x-ratelimit-remaining: 0` (or `retry-after` for the secondary limit).
fn status_error(head: &ResponseHead, what: &str) -> RemoteError {
    let exhausted = head
        .header("x-ratelimit-remaining")
        .is_some_and(|v| v.trim() == "0");
    match head.status {
        429 => rate_limited(head),
        403 if exhausted || head.header("retry-after").is_some() => rate_limited(head),
        404 | 410 => RemoteError::NotFound {
            what: what.to_owned(),
        },
        status => RemoteError::Status {
            status,
            what: what.to_owned(),
        },
    }
}

fn rate_limited(head: &ResponseHead) -> RemoteError {
    RemoteError::RateLimited {
        reset_at: head
            .header("x-ratelimit-reset")
            .and_then(|v| v.trim().parse().ok()),
    }
}
