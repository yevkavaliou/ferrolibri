use std::{collections::BTreeMap, fs, io, path::Path, time::Duration};

use tempfile::TempDir;

use super::{install::sweep_stale, *};
use crate::{Reason, SettingValue};

const SCRIPT: &str = "export function search() { return { hits: [] }; }\n";

fn manifest(id: &str, version: &str, extra: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"Test {id}\"\nversion = \"{version}\"\napi_version = 1\n\
         provides = [\"search\"]\n{extra}"
    )
}

const SETTINGS: &str = "\n[[settings]]\nkey = \"mirror\"\ntype = \"url\"\nlabel = \"Mirror\"\n\
     \n[[settings]]\nkey = \"order\"\ntype = \"choice\"\nlabel = \"Order\"\n\
     options = [\"relevance\", \"newest\"]\ndefault = \"newest\"\n\
     \n[[settings]]\nkey = \"adult\"\ntype = \"bool\"\nlabel = \"Adult\"\n";

struct Env {
    root: TempDir,
    store: PluginStore,
}

impl Env {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let store = PluginStore::new(
            root.path().join("data/plugins"),
            root.path().join("cfg/plugins.toml"),
        );
        Self { root, store }
    }

    /// A plugin folder under `sources/<name>`.
    fn source(&self, name: &str, manifest: &str, script: Option<&str>) -> std::path::PathBuf {
        let dir = self.root.path().join("sources").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("plugin.toml"), manifest).unwrap();
        if let Some(script) = script {
            fs::write(dir.join("plugin.js"), script).unwrap();
        }
        dir
    }

    fn install(&self, dir: &Path) -> InstalledPlugin {
        let inspection = self.store.inspect_local(dir).unwrap();
        self.store.install(&inspection, Downgrade::Refuse).unwrap()
    }

    /// Entries of the plugins directory, for "nothing was left behind" checks.
    fn names(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.store.plugins_dir()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

fn text(value: &str) -> SettingValue {
    SettingValue::Text(value.to_owned())
}

fn version_of(plugin: &InstalledPlugin) -> &str {
    match &plugin.status {
        PluginStatus::Ready { manifest, .. } => &manifest.version,
        PluginStatus::Broken { message } => panic!("broken: {message}"),
    }
}

#[test]
fn an_empty_store_lists_nothing() {
    let env = Env::new();
    assert!(env.store.list().unwrap().is_empty());
}

#[test]
fn installs_a_plugin_and_enables_it() {
    let env = Env::new();
    let dir = env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT));

    let inspection = env.store.inspect_local(&dir).unwrap();
    assert!(inspection.compatibility.is_compatible());
    assert_eq!(inspection.change, InstallChange::New);
    assert_eq!(inspection.manifest.id, "one");
    assert!(inspection.size >= SCRIPT.len() as u64);
    // Nothing was copied yet.
    assert!(env.names().is_empty());

    let installed = env.store.install(&inspection, Downgrade::Refuse).unwrap();
    assert!(installed.enabled);
    let PluginStatus::Ready {
        install,
        compatibility,
        ..
    } = &installed.status
    else {
        panic!("not ready: {installed:?}");
    };
    assert!(compatibility.is_compatible());
    assert_eq!(install.version, "0.1.0");
    assert_eq!(install.sha256, sha256_hex(SCRIPT.as_bytes()));
    assert!(matches!(&install.source, InstallSource::Local { path } if path.contains("one")));
    assert_eq!(env.names(), ["one"]);
    let files: Vec<_> = fs::read_dir(env.store.plugins_dir().join("one"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(files.len(), 3, "{files:?}");
}

#[test]
fn the_list_and_the_switch_survive_a_restart() {
    let env = Env::new();
    let a = env.source("a", &manifest("alpha", "0.1.0", ""), Some(SCRIPT));
    let b = env.source("b", &manifest("beta", "0.2.0", ""), Some(SCRIPT));
    env.install(&a);
    env.install(&b);
    env.store.disable("alpha").unwrap();

    let reopened = PluginStore::new(
        env.store.plugins_dir(),
        env.root.path().join("cfg/plugins.toml"),
    );
    let list = reopened.list().unwrap();
    let summary: Vec<_> = list.iter().map(|p| (p.id.as_str(), p.enabled)).collect();
    assert_eq!(summary, [("alpha", false), ("beta", true)]);
    assert_eq!(version_of(&list[1]), "0.2.0");

    reopened.enable("alpha").unwrap();
    let again = PluginStore::new(
        env.store.plugins_dir(),
        env.root.path().join("cfg/plugins.toml"),
    );
    assert!(again.list().unwrap().iter().all(|p| p.enabled));
}

#[test]
fn reinstalling_the_same_version_is_a_reinstall() {
    let env = Env::new();
    let dir = env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT));
    env.install(&dir);
    let inspection = env.store.inspect_local(&dir).unwrap();
    assert_eq!(
        inspection.change,
        InstallChange::Reinstall {
            version: "0.1.0".into()
        }
    );
    env.store.install(&inspection, Downgrade::Refuse).unwrap();
    assert_eq!(env.names(), ["one"]);
}

#[test]
fn an_update_keeps_the_switch_and_the_settings() {
    let env = Env::new();
    let old = env.source("v1", &manifest("one", "0.1.0", SETTINGS), Some(SCRIPT));
    env.install(&old);
    env.store.disable("one").unwrap();
    let saved = BTreeMap::from([
        ("order".to_owned(), text("relevance")),
        ("adult".to_owned(), SettingValue::Bool(true)),
    ]);
    env.store.save_settings("one", &saved).unwrap();

    let new = env.source(
        "v2",
        &manifest("one", "0.2.0", SETTINGS),
        Some("export function search() { return 1; }\n"),
    );
    let inspection = env.store.inspect_local(&new).unwrap();
    assert_eq!(
        inspection.change,
        InstallChange::Upgrade {
            from: "0.1.0".into(),
            to: "0.2.0".into()
        }
    );
    let updated = env.store.install(&inspection, Downgrade::Refuse).unwrap();

    assert_eq!(version_of(&updated), "0.2.0");
    assert!(!updated.enabled, "an update does not switch the plugin on");
    assert_eq!(env.store.settings("one").unwrap(), saved);
    assert_eq!(env.names(), ["one"]);
}

#[test]
fn an_update_drops_settings_the_new_version_no_longer_accepts() {
    let env = Env::new();
    let old = env.source("v1", &manifest("one", "0.1.0", SETTINGS), Some(SCRIPT));
    env.install(&old);
    let saved = BTreeMap::from([
        ("order".to_owned(), text("newest")),
        ("adult".to_owned(), SettingValue::Bool(true)),
    ]);
    env.store.save_settings("one", &saved).unwrap();

    // `adult` is gone and `order` has other choices.
    let changed = "\n[[settings]]\nkey = \"order\"\ntype = \"choice\"\nlabel = \"Order\"\n\
                   options = [\"relevance\"]\n";
    let new = env.source("v2", &manifest("one", "0.2.0", changed), Some(SCRIPT));
    env.install(&new);
    assert!(env.store.settings("one").unwrap().is_empty());
}

#[test]
fn an_update_does_not_turn_a_saved_address_into_a_network_permission() {
    let env = Env::new();
    let plain = "\n[[settings]]\nkey = \"mirror\"\ntype = \"url\"\nlabel = \"Mirror\"\n";
    let open =
        "\n[[settings]]\nkey = \"mirror\"\ntype = \"url\"\nlabel = \"Mirror\"\nnetwork = true\n";
    env.install(&env.source("v1", &manifest("one", "0.1.0", plain), Some(SCRIPT)));
    let saved = BTreeMap::from([("mirror".to_owned(), text("https://mirror.example.org"))]);
    env.store.save_settings("one", &saved).unwrap();

    // 0.2.0 opens the network for that address: the old value is not carried over.
    env.install(&env.source("v2", &manifest("one", "0.2.0", open), Some(SCRIPT)));
    assert!(env.store.settings("one").unwrap().is_empty());

    // A value saved while the setting already opened the network stays through updates.
    env.store.save_settings("one", &saved).unwrap();
    env.install(&env.source("v3", &manifest("one", "0.3.0", open), Some(SCRIPT)));
    assert_eq!(env.store.settings("one").unwrap(), saved);
}

#[test]
fn a_downgrade_needs_confirmation() {
    let env = Env::new();
    env.install(&env.source("new", &manifest("one", "0.2.0", ""), Some(SCRIPT)));
    let older = env.source(
        "old",
        &manifest("one", "0.1.0", ""),
        Some("export const x = 1;\n"),
    );
    let inspection = env.store.inspect_local(&older).unwrap();
    assert_eq!(
        inspection.change,
        InstallChange::Downgrade {
            from: "0.2.0".into(),
            to: "0.1.0".into()
        }
    );

    let refused = env
        .store
        .install(&inspection, Downgrade::Refuse)
        .unwrap_err();
    assert!(
        refused.to_string().contains("needs confirmation"),
        "{refused}"
    );
    assert_eq!(version_of(&env.store.get("one").unwrap()), "0.2.0");

    let done = env.store.install(&inspection, Downgrade::Allow).unwrap();
    assert_eq!(version_of(&done), "0.1.0");
}

#[test]
fn an_incompatible_plugin_is_described_but_not_installed() {
    let env = Env::new();
    let dir = env.source(
        "late",
        &manifest("late", "0.1.0", "min_app_version = \"99.0.0\"\n"),
        Some(SCRIPT),
    );
    let inspection = env.store.inspect_local(&dir).unwrap();
    let reasons = inspection.compatibility.reasons();
    assert!(matches!(reasons, [Reason::AppTooOld { .. }]), "{reasons:?}");

    let err = env
        .store
        .install(&inspection, Downgrade::Allow)
        .unwrap_err();
    assert!(err.to_string().contains("not compatible"), "{err}");
    assert!(env.names().is_empty());
}

#[test]
fn an_unsupported_api_version_cannot_be_inspected() {
    let env = Env::new();
    let text = manifest("future", "0.1.0", "").replace("api_version = 1", "api_version = 2");
    let dir = env.source("future", &text, Some(SCRIPT));
    let err = env.store.inspect_local(&dir).unwrap_err();
    assert!(err.to_string().contains("not supported"), "{err}");
}

#[test]
fn reserved_windows_names_are_not_accepted_as_ids() {
    let env = Env::new();
    for id in ["con", "nul", "com1", "lpt9"] {
        // The folder cannot be named like the id: on Windows that name is a device.
        let dir = env.source(
            &format!("src-{id}"),
            &manifest(id, "0.1.0", ""),
            Some(SCRIPT),
        );
        let inspection = env.store.inspect_local(&dir).unwrap();
        assert!(!inspection.compatibility.is_compatible(), "{id}");
        assert!(
            env.store.install(&inspection, Downgrade::Allow).is_err(),
            "{id}"
        );
    }
    assert!(env.names().is_empty());
}

#[test]
fn a_damaged_manifest_is_an_error() {
    let env = Env::new();
    let dir = env.source("bad", "id = [this is not toml", Some(SCRIPT));
    let err = env.store.inspect_local(&dir).unwrap_err();
    assert!(err.to_string().contains("plugin.toml"), "{err}");
    let missing = env.root.path().join("sources/nothing");
    assert!(env.store.inspect_local(&missing).is_err());
    let empty = env.root.path().join("sources/empty");
    fs::create_dir_all(&empty).unwrap();
    assert!(env.store.inspect_local(&empty).is_err());
}

#[test]
fn a_missing_or_oversized_script_is_an_error() {
    let env = Env::new();
    let none = env.source("none", &manifest("none", "0.1.0", ""), None);
    let err = env.store.inspect_local(&none).unwrap_err();
    assert!(err.to_string().contains("plugin.js"), "{err}");

    let big = env.source("big", &manifest("big", "0.1.0", ""), None);
    fs::write(big.join("plugin.js"), vec![b' '; MAX_SCRIPT_BYTES + 1]).unwrap();
    let err = env.store.inspect_local(&big).unwrap_err();
    assert!(err.to_string().contains("larger than"), "{err}");

    let limit = env.source("limit", &manifest("limit", "0.1.0", ""), None);
    fs::write(limit.join("plugin.js"), vec![b' '; MAX_SCRIPT_BYTES]).unwrap();
    env.install(&limit);
}

#[test]
fn a_wrong_sha256_is_refused_and_leaves_nothing() {
    let env = Env::new();
    let wrong = "sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n";
    let dir = env.source("sum", &manifest("sum", "0.1.0", wrong), Some(SCRIPT));
    let inspection = env.store.inspect_local(&dir).unwrap();
    let err = env
        .store
        .install(&inspection, Downgrade::Refuse)
        .unwrap_err();
    assert!(err.to_string().contains("sha256"), "{err}");
    assert!(env.names().is_empty());

    let right = format!(
        "sha256 = \"{}\"\n",
        sha256_hex(SCRIPT.as_bytes()).to_uppercase()
    );
    let ok = env.source("sum-ok", &manifest("sum", "0.1.0", &right), Some(SCRIPT));
    env.install(&ok);
}

#[test]
fn a_failure_while_copying_leaves_no_staging_and_keeps_the_old_version() {
    let env = Env::new();
    env.install(&env.source("v1", &manifest("one", "0.1.0", ""), Some(SCRIPT)));
    let next = env.source("v2", &manifest("one", "0.2.0", ""), Some(SCRIPT));
    let inspection = env.store.inspect_local(&next).unwrap();

    let mut seen = Vec::new();
    let err = env
        .store
        .install_with(&inspection, Downgrade::Refuse, |staging| {
            seen = fs::read_dir(staging)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            Err(io::Error::other("disk full"))
        })
        .unwrap_err();
    assert!(err.to_string().contains("disk full"), "{err}");
    assert_eq!(
        seen.len(),
        3,
        "the staging directory was complete: {seen:?}"
    );
    assert_eq!(env.names(), ["one"]);
    assert_eq!(version_of(&env.store.get("one").unwrap()), "0.1.0");

    let fresh = env.source("two", &manifest("two", "0.1.0", ""), Some(SCRIPT));
    let inspection = env.store.inspect_local(&fresh).unwrap();
    env.store
        .install_with(&inspection, Downgrade::Refuse, |_| {
            Err(io::Error::other("gone"))
        })
        .unwrap_err();
    assert_eq!(env.names(), ["one"]);
    assert!(env.store.get("two").is_err());
}

#[test]
fn a_plugin_changed_after_inspection_is_not_installed() {
    let env = Env::new();
    let dir = env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT));
    let inspection = env.store.inspect_local(&dir).unwrap();
    fs::write(
        dir.join("plugin.toml"),
        manifest("one", "0.1.0", "").replace("Test one", "Other name"),
    )
    .unwrap();
    let err = env
        .store
        .install(&inspection, Downgrade::Refuse)
        .unwrap_err();
    assert!(
        err.to_string().contains("changed after it was inspected"),
        "{err}"
    );
    assert!(env.names().is_empty());
}

#[test]
fn symbolic_links_in_the_source_are_not_followed() {
    let env = Env::new();
    let outside = env.root.path().join("secret.js");
    fs::write(&outside, "export const secret = 1;").unwrap();
    let dir = env.source("linked", &manifest("linked", "0.1.0", ""), None);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, dir.join("plugin.js"));
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&outside, dir.join("plugin.js"));
    if linked.is_err() {
        eprintln!("skipped: this account cannot create symbolic links");
        return;
    }
    let err = env.store.inspect_local(&dir).unwrap_err();
    assert!(err.to_string().contains("symbolic link"), "{err}");
    assert!(env.names().is_empty());
}

#[test]
fn a_broken_directory_does_not_hide_the_others_and_can_be_removed() {
    let env = Env::new();
    env.install(&env.source("good", &manifest("good", "0.1.0", ""), Some(SCRIPT)));
    env.install(&env.source("hurt", &manifest("hurt", "0.1.0", ""), Some(SCRIPT)));
    env.install(&env.source("gone", &manifest("gone", "0.1.0", ""), Some(SCRIPT)));
    let plugins = env.store.plugins_dir();
    fs::write(plugins.join("hurt/plugin.toml"), "not = [valid").unwrap();
    fs::remove_file(plugins.join("gone/plugin.js")).unwrap();
    fs::create_dir(plugins.join("stray")).unwrap();

    let list = env.store.list().unwrap();
    let ids: Vec<_> = list.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["gone", "good", "hurt", "stray"]);
    let broken: Vec<_> = list
        .iter()
        .filter(|p| matches!(p.status, PluginStatus::Broken { .. }))
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(broken, ["gone", "hurt", "stray"]);

    let err = env.store.load("hurt").unwrap_err();
    assert!(err.to_string().contains("hurt"), "{err}");
    assert!(env.store.enable("hurt").is_err());
    env.store.remove("hurt").unwrap();
    env.store.remove("stray").unwrap();
    assert_eq!(env.store.list().unwrap().len(), 2);
}

#[test]
fn a_script_changed_after_installation_is_broken() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT)));
    fs::write(
        env.store.plugins_dir().join("one/plugin.js"),
        "export const evil = 1;",
    )
    .unwrap();
    let plugin = env.store.get("one").unwrap();
    let PluginStatus::Broken { message } = &plugin.status else {
        panic!("{plugin:?}");
    };
    assert!(message.contains("changed after installation"), "{message}");
    assert!(env.store.load("one").is_err());
}

#[test]
fn remove_deletes_the_files_the_switch_and_the_settings() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", SETTINGS), Some(SCRIPT)));
    env.store
        .save_settings(
            "one",
            &BTreeMap::from([("adult".to_owned(), SettingValue::Bool(true))]),
        )
        .unwrap();
    env.store.remove("one").unwrap();
    assert!(env.names().is_empty());
    assert!(env.store.get("one").is_err());
    assert!(env.store.remove("one").is_err());

    // A plugin installed again starts clean.
    env.install(&env.source("again", &manifest("one", "0.1.0", SETTINGS), Some(SCRIPT)));
    assert!(env.store.settings("one").unwrap().is_empty());
}

#[test]
fn ids_cannot_reach_outside_the_plugins_directory() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT)));
    let victim = env.root.path().join("data/victim");
    fs::create_dir_all(&victim).unwrap();
    for id in [
        "../victim",
        "..",
        ".",
        "a/b",
        "a\\b",
        "",
        ".staging-1",
        "c:",
    ] {
        assert!(env.store.remove(id).is_err(), "{id:?}");
        assert!(env.store.get(id).is_err(), "{id:?}");
        assert!(env.store.disable(id).is_err(), "{id:?}");
        assert!(env.store.load(id).is_err(), "{id:?}");
    }
    assert!(victim.exists());
}

#[test]
fn load_gives_the_script_and_the_settings_that_are_still_valid() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", SETTINGS), Some(SCRIPT)));
    env.store
        .save_settings(
            "one",
            &BTreeMap::from([
                ("order".to_owned(), text("relevance")),
                ("mirror".to_owned(), text(" https://m.example.org ")),
            ]),
        )
        .unwrap();
    let loaded = env.store.load("one").unwrap();
    assert_eq!(loaded.source, SCRIPT);
    assert_eq!(loaded.manifest.id, "one");
    assert_eq!(loaded.settings["order"], text("relevance"));
    assert_eq!(loaded.settings["mirror"], text("https://m.example.org"));

    // A value that no longer suits the manifest is not passed on, even if it is still stored.
    let state_path = env.root.path().join("cfg/plugins.toml");
    let stored = fs::read_to_string(&state_path)
        .unwrap()
        .replace("relevance", "bogus");
    fs::write(&state_path, stored).unwrap();
    assert!(
        !env.store
            .load("one")
            .unwrap()
            .settings
            .contains_key("order")
    );
}

#[test]
fn a_disabled_plugin_does_not_load() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT)));
    env.store.disable("one").unwrap();
    let err = env.store.load("one").unwrap_err();
    assert!(err.to_string().contains("disabled"), "{err}");
    env.store.enable("one").unwrap();
    env.store.load("one").unwrap();
}

#[test]
fn settings_are_checked_against_the_manifest() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", SETTINGS), Some(SCRIPT)));
    let save = |key: &str, value: SettingValue| {
        env.store
            .save_settings("one", &BTreeMap::from([(key.to_owned(), value)]))
    };
    assert!(save("nope", text("x")).is_err(), "unknown key");
    assert!(save("order", text("sideways")).is_err(), "not a choice");
    assert!(save("order", text("")).is_err(), "a choice cannot be blank");
    assert!(save("adult", text("yes")).is_err(), "text for a bool");
    assert!(
        save("mirror", SettingValue::Bool(true)).is_err(),
        "bool for a url"
    );
    assert!(
        save("mirror", text("ftp://example.org")).is_err(),
        "not http(s)"
    );
    assert!(save("mirror", text("not a url")).is_err());
    assert!(
        env.store.settings("one").unwrap().is_empty(),
        "nothing was stored"
    );

    // A blank address means "not set".
    save("mirror", text("  ")).unwrap();
    assert!(env.store.settings("one").unwrap().is_empty());
    // The whole form is replaced.
    save("adult", SettingValue::Bool(false)).unwrap();
    save("order", text("newest")).unwrap();
    assert_eq!(
        env.store.settings("one").unwrap(),
        BTreeMap::from([("order".to_owned(), text("newest"))])
    );
    assert!(
        env.store
            .save_settings("missing", &BTreeMap::new())
            .is_err()
    );
}

#[test]
fn a_damaged_state_file_is_reported_and_not_overwritten() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT)));
    let state_path = env.root.path().join("cfg/plugins.toml");
    fs::write(&state_path, "plugins = [oops").unwrap();
    assert!(env.store.list().is_err());
    assert!(env.store.disable("one").is_err());
    let next = env.source("two", &manifest("two", "0.1.0", ""), Some(SCRIPT));
    let inspection = env.store.inspect_local(&next).unwrap();
    assert!(env.store.install(&inspection, Downgrade::Refuse).is_err());
    assert_eq!(env.names(), ["one"], "nothing was installed");
    assert_eq!(fs::read_to_string(&state_path).unwrap(), "plugins = [oops");
}

#[test]
fn leftovers_of_a_crash_are_swept_and_never_listed() {
    let env = Env::new();
    env.install(&env.source("one", &manifest("one", "0.1.0", ""), Some(SCRIPT)));
    let plugins = env.store.plugins_dir();
    fs::create_dir_all(plugins.join(".staging-abc/inner")).unwrap();
    fs::create_dir_all(plugins.join(".old-abc")).unwrap();
    assert_eq!(env.store.list().unwrap().len(), 1);

    sweep_stale(plugins, Duration::from_secs(3600));
    assert!(
        plugins.join(".staging-abc").exists(),
        "recent leftovers may belong to a running install"
    );
    sweep_stale(plugins, Duration::ZERO);
    assert_eq!(env.names(), ["one"]);
}
