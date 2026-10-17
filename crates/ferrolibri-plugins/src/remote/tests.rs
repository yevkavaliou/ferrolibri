use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use ferrolibri_core::converter::CancellationToken;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::{
    AllowList, Compatibility, Downgrade, HostErrorKind, Http, Inspection, InstallChange,
    InstallSource, PluginStatus, PluginStore,
    limits::{MAX_SCRIPT_BYTES, RELEASE_CHECK_WORKERS},
    store::sha256_hex,
    testing::{Reply, TestServer},
};

const SCRIPT: &str = "export function search() { return { hits: [] }; }\n";
const API_PATH: &str = "/repos/acme/books/releases";

fn manifest(id: &str, version: &str, extra: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"Test {id}\"\nversion = \"{version}\"\napi_version = 1\n\
         provides = [\"search\"]\n{extra}"
    )
}

type Routes = Arc<Mutex<HashMap<String, Reply>>>;

/// A scripted GitHub: the API answers at [`API_PATH`] and the files at `/dl/<tag>/<name>`.
struct Env {
    server: TestServer,
    routes: Routes,
    _root: TempDir,
    store: PluginStore,
    installer: RemoteInstaller,
}

impl Env {
    fn new() -> Self {
        let routes: Routes = Arc::default();
        let handler_routes = Arc::clone(&routes);
        let server = TestServer::start(move |request, _| {
            handler_routes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&request.path)
                .cloned()
                .unwrap_or_else(|| Reply::status(404))
        });
        let root = TempDir::new().unwrap();
        let store = PluginStore::new(
            root.path().join("data/plugins"),
            root.path().join("cfg/plugins.toml"),
        );
        let installer = RemoteInstaller::with_endpoint(
            server.http(),
            GithubEndpoint::custom(&server.url_on("api", ""), server.allow_list()),
        );
        Self {
            server,
            routes,
            _root: root,
            store,
            installer,
        }
    }

    fn route(&self, path: &str, reply: Reply) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_owned(), reply);
    }

    /// Serves `body` for the asset and returns its entry for the API's list.
    fn asset_on(
        &self,
        host: &str,
        tag: &str,
        name: &str,
        body: &str,
        digest: Option<&str>,
    ) -> Value {
        let path = format!("/dl/{tag}/{name}");
        self.route(&path, Reply::ok("application/octet-stream", body));
        let mut asset = json!({
            "name": name,
            "state": "uploaded",
            "size": body.len(),
            "browser_download_url": self.server.url_on(host, &path),
        });
        if let Some(digest) = digest {
            asset["digest"] = json!(digest);
        }
        asset
    }

    fn asset(&self, tag: &str, name: &str, body: &str) -> Value {
        self.asset_on("dl", tag, name, body, None)
    }

    /// A release with `plugin.toml` and `plugin.js`.
    fn plugin_release(&self, tag: &str, manifest: &str, script: &str) -> Value {
        release(
            tag,
            false,
            vec![
                self.asset(tag, "plugin.toml", manifest),
                self.asset(tag, "plugin.js", script),
            ],
        )
    }

    fn serve(&self, releases: Vec<Value>) {
        self.route(
            API_PATH,
            Reply::ok("application/json", Value::Array(releases).to_string())
                .header("x-ratelimit-remaining", "57"),
        );
    }

    fn repo() -> GithubRepo {
        GithubRepo {
            owner: "acme".into(),
            name: "books".into(),
        }
    }

    fn list(&self) -> Result<ReleaseList, RemoteError> {
        self.list_with(ReleaseOptions::default())
    }

    fn list_with(&self, options: ReleaseOptions) -> Result<ReleaseList, RemoteError> {
        self.installer.releases(
            &self.store,
            &Self::repo(),
            options,
            &CancellationToken::default(),
        )
    }

    fn inspect(&self, release: &Release) -> Result<Inspection, RemoteError> {
        self.installer
            .inspect_release(&self.store, release, &CancellationToken::default())
    }

    fn only(&self) -> Release {
        let mut list = self.list().unwrap();
        assert_eq!(list.releases.len(), 1, "{list:?}");
        list.releases.remove(0)
    }

    /// Entries of the plugins directory, for "nothing was left behind" checks.
    fn names(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.store.plugins_dir()) else {
            return Vec::new();
        };
        entries
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect()
    }
}

fn release(tag: &str, prerelease: bool, assets: Vec<Value>) -> Value {
    json!({
        "tag_name": tag,
        "name": format!("Release {tag}"),
        "draft": false,
        "prerelease": prerelease,
        "published_at": "2026-10-17T10:00:00Z",
        "assets": assets,
    })
}

fn message(release: &Release) -> &str {
    match &release.check {
        ReleaseCheck::Failed { message } => message,
        ReleaseCheck::Ready { .. } => panic!("expected a failed check: {release:?}"),
    }
}

fn compatibility(release: &Release) -> &Compatibility {
    match &release.check {
        ReleaseCheck::Ready { compatibility, .. } => compatibility,
        ReleaseCheck::Failed { message } => panic!("failed: {message}"),
    }
}

fn github_source(tag: &str) -> InstallSource {
    InstallSource::Github {
        repo: "acme/books".into(),
        tag: tag.into(),
    }
}

// ----- the address -----

#[test]
fn repository_addresses_are_recognised_with_their_tails() {
    let repo = |owner: &str, name: &str| {
        Ok(RemoteSource::Github(GithubRepo {
            owner: owner.into(),
            name: name.into(),
        }))
    };
    for input in [
        "https://github.com/acme/books",
        "  https://github.com/acme/books/  ",
        "https://github.com/acme/books.git",
        "https://github.com/acme/books/releases",
        "https://github.com/acme/books/releases/tag/v1.0.0",
        "https://github.com/acme/books/tree/main/src?tab=readme#top",
        "https://www.GitHub.com/acme/books",
    ] {
        assert_eq!(RemoteSource::parse(input), repo("acme", "books"), "{input}");
    }
}

#[test]
fn a_link_to_plugin_toml_is_a_manifest_source() {
    for input in [
        "https://example.org/plugins/one/plugin.toml",
        "http://192.168.1.5:8080/plugin.toml?x=1",
        "https://github.com/acme/books/releases/download/v1/plugin.toml",
    ] {
        assert!(
            matches!(RemoteSource::parse(input), Ok(RemoteSource::Manifest(_))),
            "{input}"
        );
    }
    let Ok(RemoteSource::Manifest(url)) =
        RemoteSource::parse("https://example.org/a/plugin.toml#x")
    else {
        panic!("not a manifest");
    };
    assert_eq!(url.as_str(), "https://example.org/a/plugin.toml");
}

#[test]
fn other_text_is_an_error_that_shows_the_expected_form() {
    for input in [
        "acme/books",
        "github.com/acme/books",
        "https://github.com/acme",
        "https://github.com/",
        "https://github.com/acme/..",
        "https://github.com/ac me/books",
        "https://gitlab.com/acme/books",
        "http://github.com/acme/books",
        "ftp://example.org/plugin.toml",
        "https://user:pass@example.org/plugin.toml",
        "file:///C:/plugin.toml",
    ] {
        let text = RemoteSource::parse(input).unwrap_err().to_string();
        assert!(
            text.contains("https://github.com/owner/repo"),
            "{input}: {text}"
        );
    }
    assert_eq!(RemoteSource::parse(""), Err(AddressError::Empty));
    assert_eq!(RemoteSource::parse("   "), Err(AddressError::Empty));
}

// ----- the list of releases -----

#[test]
fn only_releases_with_a_manifest_are_listed() {
    let env = Env::new();
    let mut draft = env.plugin_release("v0.5.0", &manifest("books", "0.5.0", ""), SCRIPT);
    draft["draft"] = json!(true);
    env.serve(vec![
        env.plugin_release("v1.1.0", &manifest("books", "1.1.0", ""), SCRIPT),
        release("v1.0.5", false, Vec::new()),
        release(
            "v1.0.4",
            false,
            vec![env.asset("v1.0.4", "plugin.js", SCRIPT)],
        ),
        release(
            "v1.0.3",
            false,
            vec![env.asset("v1.0.3", "notes.txt", "hello")],
        ),
        draft,
        env.plugin_release("v1.0.0", &manifest("books", "1.0.0", ""), SCRIPT),
    ]);

    let list = env.list().unwrap();
    let tags: Vec<_> = list.releases.iter().map(|r| r.tag.as_str()).collect();
    assert_eq!(tags, ["v1.1.0", "v1.0.0"]);
    assert_eq!(list.rate_limit_remaining, Some(57));
    let first = &list.releases[0];
    assert_eq!(first.name.as_deref(), Some("Release v1.1.0"));
    assert_eq!(first.published_at.as_deref(), Some("2026-10-17T10:00:00Z"));
    assert!(!first.prerelease);
    let ReleaseCheck::Ready {
        manifest,
        compatibility,
        change,
    } = &first.check
    else {
        panic!("{first:?}");
    };
    assert_eq!(manifest.version, "1.1.0");
    assert!(compatibility.is_compatible());
    assert_eq!(*change, InstallChange::New);
    // Nothing was written.
    assert!(env.names().is_empty());
}

#[test]
fn prereleases_are_hidden_unless_asked_for() {
    let env = Env::new();
    let tag = "v2.0.0-rc.1";
    let pre = release(
        tag,
        true,
        vec![
            env.asset(tag, "plugin.toml", &manifest("books", "2.0.0-rc.1", "")),
            env.asset(tag, "plugin.js", SCRIPT),
        ],
    );
    env.serve(vec![
        pre,
        env.plugin_release("v1.0.0", &manifest("books", "1.0.0", ""), SCRIPT),
    ]);

    let tags =
        |list: ReleaseList| -> Vec<String> { list.releases.into_iter().map(|r| r.tag).collect() };
    assert_eq!(tags(env.list().unwrap()), ["v1.0.0"]);
    let all = env
        .list_with(ReleaseOptions {
            include_prereleases: true,
        })
        .unwrap();
    assert!(all.releases[0].prerelease);
    assert_eq!(tags(all), ["v2.0.0-rc.1", "v1.0.0"]);
}

#[test]
fn the_request_to_the_api_is_a_plain_get_with_the_right_headers() {
    let env = Env::new();
    env.serve(Vec::new());
    env.list().unwrap();
    let requests = env.server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, API_PATH);
    assert_eq!(requests[0].query, "per_page=10");
    assert_eq!(
        requests[0].header("accept"),
        Some("application/vnd.github+json")
    );
    assert!(
        requests[0]
            .header("user-agent")
            .unwrap()
            .starts_with("FerroLibri/")
    );
    assert!(requests[0].header("authorization").is_none());
}

#[test]
fn a_repository_without_releases_gives_an_empty_list() {
    let env = Env::new();
    env.serve(Vec::new());
    assert!(env.list().unwrap().releases.is_empty());
}

#[test]
fn an_unknown_repository_is_not_found() {
    let env = Env::new();
    env.route(
        API_PATH,
        Reply::status(404).body(r#"{"message":"Not Found"}"#),
    );
    assert!(matches!(env.list(), Err(RemoteError::NotFound { .. })));
}

#[test]
fn an_answer_that_is_not_a_list_is_invalid() {
    let env = Env::new();
    for body in ["<html>", r#"{"message":"hi"}"#, "null"] {
        env.route(API_PATH, Reply::ok("application/json", body));
        assert!(
            matches!(env.list(), Err(RemoteError::Invalid { .. })),
            "{body}"
        );
    }
}

#[test]
fn a_release_the_api_describes_wrongly_is_skipped() {
    let env = Env::new();
    let good = env.plugin_release("v1.0.0", &manifest("books", "1.0.0", ""), SCRIPT);
    env.serve(vec![
        json!({"tag_name": 5}),
        json!("text"),
        json!({"tag_name": "x".repeat(300), "assets": []}),
        good,
    ]);
    assert_eq!(env.list().unwrap().releases.len(), 1);
}

#[test]
fn at_most_ten_releases_are_looked_at() {
    let env = Env::new();
    let many = (0..15)
        .map(|i| {
            let tag = format!("v1.0.{i}");
            env.plugin_release(&tag, &manifest("books", &format!("1.0.{i}"), ""), SCRIPT)
        })
        .collect();
    env.serve(many);
    assert_eq!(env.list().unwrap().releases.len(), 10);
}

#[test]
fn compatibility_is_reported_for_every_release() {
    let env = Env::new();
    env.serve(vec![
        env.plugin_release(
            "v3.0.0",
            &manifest("books", "3.0.0", "min_app_version = \"99.0.0\"\n"),
            SCRIPT,
        ),
        env.plugin_release("v1.0.0", &manifest("books", "1.0.0", ""), SCRIPT),
        env.plugin_release(
            "v0.1.0",
            &manifest("books", "0.1.0", "").replace("api_version = 1", "api_version = 7"),
            SCRIPT,
        ),
    ]);
    let list = env.list().unwrap();
    assert!(!compatibility(&list.releases[0]).is_compatible());
    assert!(compatibility(&list.releases[1]).is_compatible());
    // A manifest for an API this app does not know cannot even be read.
    assert!(message(&list.releases[2]).contains("plugin.toml"));
}

#[test]
fn a_release_without_its_script_or_with_a_broken_manifest_is_marked() {
    let env = Env::new();
    let mut gone = env.asset("v1", "plugin.toml", "x");
    gone["browser_download_url"] = json!(env.server.url_on("dl", "/dl/v1/gone/plugin.toml"));
    env.serve(vec![
        release(
            "v3",
            false,
            vec![env.asset("v3", "plugin.toml", &manifest("books", "3.0.0", ""))],
        ),
        release(
            "v2",
            false,
            vec![env.asset("v2", "plugin.toml", "this is = not toml [")],
        ),
        // Listed with a manifest asset that the server does not have.
        release("v1", false, vec![gone]),
    ]);
    let list = env.list().unwrap();
    assert!(message(&list.releases[0]).contains("plugin.js"));
    assert!(message(&list.releases[1]).contains("plugin.toml"));
    assert!(
        message(&list.releases[2]).contains("not found"),
        "{:?}",
        list.releases[2]
    );
    // They cannot be installed.
    assert!(env.inspect(&list.releases[0]).is_err());
}

#[test]
fn the_script_asset_follows_the_manifests_entry() {
    let env = Env::new();
    env.serve(vec![release(
        "v1",
        false,
        vec![
            env.asset(
                "v1",
                "plugin.toml",
                &manifest("books", "1.0.0", "entry = \"main.js\"\n"),
            ),
            env.asset("v1", "main.js", SCRIPT),
        ],
    )]);
    let inspection = env.inspect(&env.only()).unwrap();
    assert_eq!(inspection.manifest.entry, "main.js");
    env.store.install(&inspection, Downgrade::Refuse).unwrap();
    assert!(env.store.plugins_dir().join("books/main.js").is_file());
}

#[test]
fn the_manifests_are_fetched_a_few_at_a_time() {
    let env = Env::new();
    let releases = (0..10)
        .map(|i| {
            let tag = format!("v1.0.{i}");
            let host = format!("dl{i}");
            let path = format!("/dl/{tag}/plugin.toml");
            env.route(
                &path,
                Reply::ok("text/plain", manifest("books", &format!("1.0.{i}"), ""))
                    .delay(Duration::from_millis(200)),
            );
            let script = env.asset_on(&host, &tag, "plugin.js", SCRIPT, None);
            let toml = json!({
                "name": "plugin.toml",
                "browser_download_url": env.server.url_on(&host, &path),
            });
            release(&tag, false, vec![toml, script])
        })
        .collect();
    env.serve(releases);

    let list = env.list().unwrap();
    // Newest first, as the API gave them, whichever worker finished first.
    let tags: Vec<_> = list.releases.iter().map(|r| r.tag.clone()).collect();
    let expected: Vec<_> = (0..10).map(|i| format!("v1.0.{i}")).collect();
    assert_eq!(tags, expected);
    let peak = env.server.max_concurrent();
    assert!(peak > 1, "no overlap: {peak}");
    assert!(peak <= RELEASE_CHECK_WORKERS, "too many at once: {peak}");
}

// ----- the limit of the API -----

#[test]
fn the_api_limit_is_reported_in_the_forms_github_uses() {
    let env = Env::new();
    let limited = [
        Reply::status(403)
            .header("x-ratelimit-remaining", "0")
            .body(r#"{"message":"API rate limit exceeded"}"#),
        Reply::status(429).header("retry-after", "60"),
        Reply::status(403).header("retry-after", "30"),
    ];
    for reply in limited {
        env.route(API_PATH, reply);
        let error = env.list().unwrap_err();
        assert!(
            matches!(error, RemoteError::RateLimited { .. }),
            "{error:?}"
        );
    }
    env.route(
        API_PATH,
        Reply::status(403)
            .header("x-ratelimit-remaining", "0")
            .header("x-ratelimit-reset", "1760000000"),
    );
    let error = env.list().unwrap_err();
    assert!(
        matches!(
            error,
            RemoteError::RateLimited {
                reset_at: Some(1_760_000_000)
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("try again later"));
}

#[test]
fn a_refusal_that_is_not_the_limit_is_an_ordinary_status() {
    let env = Env::new();
    env.route(
        API_PATH,
        Reply::status(403).header("x-ratelimit-remaining", "41"),
    );
    assert!(matches!(
        env.list(),
        Err(RemoteError::Status { status: 403, .. })
    ));
    env.route(API_PATH, Reply::status(500));
    assert!(matches!(
        env.list(),
        Err(RemoteError::Status { status: 500, .. })
    ));
}

#[test]
fn running_out_of_the_limit_while_fetching_manifests_stops_the_listing() {
    let env = Env::new();
    let asset = env.asset("v1.0.0", "plugin.toml", "x");
    env.route(
        "/dl/v1.0.0/plugin.toml",
        Reply::status(429).header("retry-after", "60"),
    );
    env.serve(vec![release("v1.0.0", false, vec![asset])]);
    assert!(matches!(env.list(), Err(RemoteError::RateLimited { .. })));
}

// ----- installing a release -----

#[test]
fn a_release_is_downloaded_inspected_and_installed() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let inspection = env.inspect(&env.only()).unwrap();
    assert_eq!(inspection.manifest.id, "books");
    assert!(inspection.compatibility.is_compatible());
    assert_eq!(inspection.change, InstallChange::New);
    assert_eq!(inspection.source, github_source("v1.0.0"));
    // Only the download directory exists so far, and it is not a plugin.
    assert!(env.store.list().unwrap().is_empty());

    let installed = env.store.install(&inspection, Downgrade::Refuse).unwrap();
    assert!(installed.enabled);
    let PluginStatus::Ready { install, .. } = &installed.status else {
        panic!("{installed:?}");
    };
    assert_eq!(install.sha256, sha256_hex(SCRIPT.as_bytes()));
    assert_eq!(install.source, github_source("v1.0.0"));
    let record: Value = serde_json::from_slice(
        &fs::read(env.store.plugins_dir().join("books/install.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record["source"],
        json!({"type": "github", "repo": "acme/books", "tag": "v1.0.0"})
    );
    assert!(env.store.load("books").is_ok());

    drop(inspection);
    assert_eq!(env.names(), ["books"], "the download directory is gone");
}

#[test]
fn a_newer_release_is_an_upgrade_and_an_older_one_a_downgrade() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let first = env.inspect(&env.only()).unwrap();
    env.store.install(&first, Downgrade::Refuse).unwrap();

    env.serve(vec![
        env.plugin_release("v1.1.0", &manifest("books", "1.1.0", ""), SCRIPT),
        env.plugin_release("v0.9.0", &manifest("books", "0.9.0", ""), SCRIPT),
    ]);
    let list = env.list().unwrap();
    let change = |i: usize| match &list.releases[i].check {
        ReleaseCheck::Ready { change, .. } => change.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        change(0),
        InstallChange::Upgrade {
            from: "1.0.0".into(),
            to: "1.1.0".into()
        }
    );
    assert!(matches!(change(1), InstallChange::Downgrade { .. }));
    let old = env.inspect(&list.releases[1]).unwrap();
    assert!(env.store.install(&old, Downgrade::Refuse).is_err());
    env.store.install(&old, Downgrade::Allow).unwrap();
}

#[test]
fn an_incompatible_release_can_be_inspected_but_not_installed() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", "min_app_version = \"99.0.0\"\n"),
        SCRIPT,
    )]);
    let inspection = env.inspect(&env.only()).unwrap();
    assert!(!inspection.compatibility.is_compatible());
    // The script was not even downloaded.
    assert!(!env.server.paths().iter().any(|p| p.ends_with("plugin.js")));
    assert!(env.store.install(&inspection, Downgrade::Refuse).is_err());
    assert!(env.store.list().unwrap().is_empty());
}

#[test]
fn a_script_that_is_not_the_one_in_the_manifest_is_refused() {
    let env = Env::new();
    let real = sha256_hex(SCRIPT.as_bytes());
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", &format!("sha256 = \"{real}\"\n")),
        "export function search() { return { hits: [{}] }; }\n",
    )]);
    let error = env.inspect(&env.only()).unwrap_err();
    assert!(
        matches!(&error, RemoteError::HashMismatch { file, announced_by: "plugin.toml" } if file == "plugin.js"),
        "{error:?}"
    );
    assert!(env.names().is_empty(), "{:?}", env.names());
    assert!(env.store.list().unwrap().is_empty());
}

#[test]
fn the_digest_github_reports_is_checked_when_there_is_one() {
    let env = Env::new();
    let good = format!("sha256:{}", sha256_hex(SCRIPT.as_bytes()));
    let wrong = format!("sha256:{}", sha256_hex(b"something else"));
    let toml = manifest("books", "1.0.0", "");
    let case = |digest: Option<&str>| {
        let tag = "v1.0.0";
        env.serve(vec![release(
            tag,
            false,
            vec![
                env.asset(tag, "plugin.toml", &toml),
                env.asset_on("dl", tag, "plugin.js", SCRIPT, digest),
            ],
        )]);
        env.inspect(&env.only())
    };
    assert!(case(Some(&good)).is_ok());
    assert!(case(Some(&good.to_uppercase().replace("SHA256:", "sha256:"))).is_ok());
    assert!(case(None).is_ok(), "the field is optional");
    assert!(
        case(Some("sha512:abcdef")).is_ok(),
        "another algorithm is not checked"
    );
    assert!(case(Some("sha256:nonsense")).is_ok());
    let error = case(Some(&wrong)).unwrap_err();
    assert!(
        matches!(
            &error,
            RemoteError::HashMismatch {
                announced_by: "GitHub",
                ..
            }
        ),
        "{error:?}"
    );
    assert!(env.names().is_empty());
}

#[test]
fn a_manifest_that_does_not_fit_its_digest_marks_the_release() {
    let env = Env::new();
    let wrong = format!("sha256:{}", sha256_hex(b"something else"));
    let tag = "v1.0.0";
    env.serve(vec![release(
        tag,
        false,
        vec![
            env.asset_on(
                "dl",
                tag,
                "plugin.toml",
                &manifest("books", "1.0.0", ""),
                Some(&wrong),
            ),
            env.asset(tag, "plugin.js", SCRIPT),
        ],
    )]);
    let list = env.list().unwrap();
    assert!(
        message(&list.releases[0]).contains("hash"),
        "{:?}",
        list.releases[0]
    );
}

#[test]
fn an_asset_that_cannot_be_downloaded_fails_the_installation() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let found = env.only();
    env.route("/dl/v1.0.0/plugin.js", Reply::status(404));
    assert!(matches!(
        env.inspect(&found),
        Err(RemoteError::NotFound { .. })
    ));
    env.route("/dl/v1.0.0/plugin.js", Reply::status(503));
    assert!(matches!(
        env.inspect(&found),
        Err(RemoteError::Status { status: 503, .. })
    ));
    env.route("/dl/v1.0.0/plugin.js", Reply::status(429));
    assert!(matches!(
        env.inspect(&found),
        Err(RemoteError::RateLimited { .. })
    ));
    assert!(env.names().is_empty());
}

#[test]
fn a_script_that_is_too_large_is_not_downloaded() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let found = env.only();
    env.route(
        "/dl/v1.0.0/plugin.js",
        Reply::repeated(
            "application/octet-stream",
            b'x',
            MAX_SCRIPT_BYTES as u64 + 1,
        ),
    );
    let error = env.inspect(&found).unwrap_err();
    assert!(
        matches!(&error, RemoteError::Network(e) if e.kind == HostErrorKind::TooLarge),
        "{error:?}"
    );
}

#[test]
fn a_redirect_inside_the_allowed_domains_is_followed() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let found = env.only();
    env.route(
        "/objects/plugin.js",
        Reply::ok("application/octet-stream", SCRIPT),
    );
    env.route(
        "/dl/v1.0.0/plugin.js",
        Reply::redirect(302, &env.server.url_on("objects", "/objects/plugin.js")),
    );
    assert!(env.inspect(&found).is_ok());
}

#[test]
fn a_redirect_to_another_domain_is_refused() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let found = env.only();
    let elsewhere = format!("http://evil.example:{}/stolen.js", env.server.port());
    env.route("/dl/v1.0.0/plugin.js", Reply::redirect(302, &elsewhere));
    let error = env.inspect(&found).unwrap_err();
    assert!(
        matches!(&error, RemoteError::Network(e) if e.kind == HostErrorKind::BlockedDomain),
        "{error:?}"
    );
    assert!(!env.server.paths().iter().any(|p| p == "/stolen.js"));
    assert!(env.names().is_empty());

    // The same for a manifest, which marks its release instead of failing the list.
    env.route("/dl/v1.0.0/plugin.toml", Reply::redirect(302, &elsewhere));
    let list = env.list().unwrap();
    assert!(
        message(&list.releases[0]).contains("allowed"),
        "{:?}",
        list.releases[0]
    );
}

#[test]
fn cancelling_stops_the_listing() {
    let env = Env::new();
    env.serve(vec![env.plugin_release(
        "v1.0.0",
        &manifest("books", "1.0.0", ""),
        SCRIPT,
    )]);
    let cancel = CancellationToken::default();
    cancel.cancel();
    let error = env
        .installer
        .releases(&env.store, &Env::repo(), ReleaseOptions::default(), &cancel)
        .unwrap_err();
    assert!(error.is_cancelled(), "{error:?}");
    assert!(env.server.requests().is_empty());
}

// ----- a link to plugin.toml -----

fn manifest_url(env: &Env, path: &str) -> url::Url {
    url::Url::parse(&env.server.url(path)).unwrap()
}

fn inspect_url(env: &Env, url: &url::Url) -> Result<Inspection, RemoteError> {
    env.installer
        .inspect_manifest_url(&env.store, url, &CancellationToken::default())
}

#[test]
fn a_manifest_link_installs_the_script_from_the_same_directory() {
    let env = Env::new();
    env.route(
        "/p/one/plugin.toml",
        Reply::ok("text/plain", manifest("one", "0.3.0", "")),
    );
    env.route("/p/one/plugin.js", Reply::ok("text/javascript", SCRIPT));
    let url = manifest_url(&env, "/p/one/plugin.toml?ref=main");
    let inspection = inspect_url(&env, &url).unwrap();
    assert_eq!(inspection.manifest.id, "one");
    assert_eq!(
        inspection.source,
        InstallSource::Url {
            url: url.as_str().into()
        }
    );
    let installed = env.store.install(&inspection, Downgrade::Refuse).unwrap();
    assert!(installed.enabled);
    assert!(env.store.load("one").is_ok());
}

#[test]
fn a_manifest_link_follows_the_entry_name() {
    let env = Env::new();
    env.route(
        "/p/plugin.toml",
        Reply::ok(
            "text/plain",
            manifest("one", "0.3.0", "entry = \"main.js\"\n"),
        ),
    );
    env.route("/p/main.js", Reply::ok("text/javascript", SCRIPT));
    assert!(inspect_url(&env, &manifest_url(&env, "/p/plugin.toml")).is_ok());
}

#[test]
fn a_manifest_link_without_a_script_or_with_a_wrong_one_is_refused() {
    let env = Env::new();
    env.route(
        "/p/plugin.toml",
        Reply::ok("text/plain", manifest("one", "0.3.0", "")),
    );
    let url = manifest_url(&env, "/p/plugin.toml");
    assert!(matches!(
        inspect_url(&env, &url),
        Err(RemoteError::NotFound { .. })
    ));

    let other = sha256_hex(b"other");
    env.route(
        "/p/plugin.toml",
        Reply::ok(
            "text/plain",
            manifest("one", "0.3.0", &format!("sha256 = \"{other}\"\n")),
        ),
    );
    env.route("/p/plugin.js", Reply::ok("text/javascript", SCRIPT));
    assert!(matches!(
        inspect_url(&env, &url),
        Err(RemoteError::HashMismatch { .. })
    ));

    env.route(
        "/p/plugin.toml",
        Reply::ok("text/html", "<html>not a manifest</html>"),
    );
    assert!(matches!(
        inspect_url(&env, &url),
        Err(RemoteError::Invalid { .. })
    ));
    assert!(env.names().is_empty());
}

#[test]
fn a_manifest_link_may_only_reach_its_own_host() {
    let env = Env::new();
    // The typed address is the only thing allowed: the endpoint here has an empty list.
    let installer = RemoteInstaller::with_endpoint(
        env.server.http(),
        GithubEndpoint::custom(&env.server.url_on("api", ""), AllowList::new()),
    );
    let elsewhere = format!("http://evil.example:{}/plugin.js", env.server.port());
    env.route(
        "/p/plugin.toml",
        Reply::ok("text/plain", manifest("one", "0.3.0", "")),
    );
    env.route("/p/plugin.js", Reply::redirect(302, &elsewhere));
    let error = installer
        .inspect_manifest_url(
            &env.store,
            &manifest_url(&env, "/p/plugin.toml"),
            &CancellationToken::default(),
        )
        .unwrap_err();
    assert!(
        matches!(&error, RemoteError::Network(e) if e.kind == HostErrorKind::BlockedDomain),
        "{error:?}"
    );
}

#[test]
fn the_built_in_endpoint_allows_only_the_github_hosts() {
    let endpoint = GithubEndpoint::github();
    let allow = &endpoint.allow;
    for good in [
        "https://api.github.com/repos/a/b/releases",
        "https://github.com/a/b/releases/download/v1/plugin.js",
        "https://objects.githubusercontent.com/x",
        "https://release-assets.githubusercontent.com/x",
    ] {
        assert!(
            allow.check(&url::Url::parse(good).unwrap()).is_ok(),
            "{good}"
        );
    }
    for bad in [
        "http://github.com/a",
        "https://evil.example/plugin.js",
        "https://github.com.evil.example/plugin.js",
        "https://gist.github.com/a",
        "https://githubusercontent.com.evil.example/",
    ] {
        assert!(
            allow.check(&url::Url::parse(bad).unwrap()).is_err(),
            "{bad}"
        );
    }
}

// --- The real GitHub (needs the internet; run with `cargo test -p ferrolibri-plugins -- --ignored`) ---

#[test]
#[ignore = "needs the internet"]
fn the_real_api_answers_and_a_repository_without_plugins_gives_an_empty_list() {
    let env = Env::new();
    let installer = RemoteInstaller::new(Http::new());
    let repo = GithubRepo {
        owner: "BurntSushi".into(),
        name: "ripgrep".into(),
    };
    let list = installer
        .releases(
            &env.store,
            &repo,
            ReleaseOptions::default(),
            &CancellationToken::default(),
        )
        .unwrap();
    assert!(list.releases.is_empty(), "ripgrep ships no plugin.toml");
    assert!(list.rate_limit_remaining.is_some());
}
