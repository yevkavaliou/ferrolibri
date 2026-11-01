use super::*;

const VALID: &str = r#"
id = "google-books"
name = "Google Books"
version = "0.1.0"
api_version = 1
min_app_version = "0.1.0"
author = "Name"
homepage = "https://example.org/google-books"
license = "MIT"
description = "Search Google Books"
provides = ["search"]
entry = "plugin.js"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
content = ["metadata", "public-domain"]
search_hint = "Search by title, author or ISBN"

[network]
domains = ["www.googleapis.com", "*.books.google.com"]

[limits]
call_timeout_s = 45

[[settings]]
key = "base_url"
type = "url"
label = "Mirror address"
network = true
default = "https://example.org"

[[settings]]
key = "adult"
type = "bool"
label = "Include adult content"
default = false

[[settings]]
key = "order"
type = "choice"
label = "Order"
options = ["relevance", "newest"]
default = "newest"
"#;

fn app() -> Version {
    Version::new(0, 1, 2)
}

fn minimal() -> String {
    r#"
id = "minimal"
name = "Minimal"
version = "1.0.0"
api_version = 1
provides = ["search"]
"#
    .to_owned()
}

fn manifest(text: &str) -> Manifest {
    Manifest::parse(text).unwrap_or_else(|e| panic!("{e}"))
}

fn reasons(manifest: &Manifest) -> Vec<Reason> {
    manifest.compatibility(&app()).reasons().to_vec()
}

/// The single reason a manifest is incompatible, as `(field, message)`.
fn single_invalid(manifest: &Manifest) -> (String, String) {
    match reasons(manifest).as_slice() {
        [Reason::Invalid { field, message }] => (field.clone(), message.clone()),
        other => panic!("expected one Invalid reason, got {other:?}"),
    }
}

fn with(edit: impl FnOnce(&mut Manifest)) -> Manifest {
    let mut m = manifest(&minimal());
    edit(&mut m);
    m
}

#[test]
fn valid_manifest_is_compatible() {
    let m = manifest(VALID);
    assert_eq!(m.compatibility(&app()), Compatibility::Compatible);
    assert_eq!(m.id, "google-books");
    assert_eq!(m.settings.len(), 3);
    assert_eq!(m.settings[0].kind, SettingKind::Url);
    assert!(m.settings[0].network);
    assert_eq!(m.settings[1].default, Some(SettingValue::Bool(false)));
    assert_eq!(m.call_timeout(), Duration::from_secs(45));
    assert_eq!(
        m.domain_rules(),
        vec![
            DomainRule::Exact("www.googleapis.com".into()),
            DomainRule::Subdomains("books.google.com".into())
        ]
    );
}

#[test]
fn minimal_manifest_uses_defaults() {
    let m = manifest(&minimal());
    assert_eq!(m.compatibility(&app()), Compatibility::Compatible);
    assert_eq!(m.entry, "plugin.js");
    assert_eq!(m.call_timeout(), Duration::from_secs(30));
    assert!(m.network.domains.is_empty() && m.settings.is_empty());
}

#[test]
fn empty_manifest_is_rejected_with_the_missing_field() {
    let error = Manifest::parse("").unwrap_err();
    assert!(
        matches!(&error, ManifestError::Invalid(m) if m.contains("missing field")),
        "{error}"
    );
}

#[test]
fn broken_toml_is_rejected() {
    assert!(matches!(
        Manifest::parse("id = "),
        Err(ManifestError::Invalid(_))
    ));
}

#[test]
fn wrong_field_type_is_rejected() {
    let text = minimal().replace("api_version = 1", "api_version = \"1\"");
    assert!(matches!(
        Manifest::parse(&text),
        Err(ManifestError::Invalid(_))
    ));
}

#[test]
fn oversized_manifest_is_rejected_before_parsing() {
    let text = format!("{}\n# {}", minimal(), "x".repeat(MAX_MANIFEST_BYTES));
    assert!(matches!(
        Manifest::parse(&text),
        Err(ManifestError::TooLarge { .. })
    ));
}

#[test]
fn unknown_fields_are_ignored() {
    let text = format!(
        "{}\nfuture_field = 3\n[future_table]\nx = 1\n[network]\ndomains = []\nproxy = true\n",
        minimal()
    );
    let m = manifest(&text);
    assert_eq!(m.compatibility(&app()), Compatibility::Compatible);
}

#[test]
fn unknown_provides_entries_are_ignored_when_search_is_present() {
    let m = with(|m| m.provides = vec!["metadata".into(), "search".into()]);
    assert!(m.compatibility(&app()).is_compatible());
}

#[test]
fn provides_without_search_is_incompatible() {
    let m = with(|m| m.provides = vec!["metadata".into()]);
    assert_eq!(reasons(&m), vec![Reason::NoSearch]);
    let m = with(|m| m.provides.clear());
    assert_eq!(reasons(&m), vec![Reason::NoSearch]);
}

#[test]
fn future_api_version_stops_parsing_even_if_the_rest_does_not_fit() {
    // A newer protocol may lay out the manifest differently: report the version, not a
    // confusing "missing field".
    let error = Manifest::parse("api_version = 2\nid = 5").unwrap_err();
    assert_eq!(error, ManifestError::UnsupportedApiVersion { found: 2 });
    assert!(error.to_string().contains("API version 2"));
    assert_eq!(
        Manifest::parse("api_version = 0\nid = 5").unwrap_err(),
        ManifestError::UnsupportedApiVersion { found: 0 }
    );
    assert_eq!(
        Manifest::parse("api_version = -1").unwrap_err(),
        ManifestError::UnsupportedApiVersion { found: -1 }
    );
}

#[test]
fn compatibility_rejects_an_unsupported_api_version_of_a_built_manifest() {
    let m = with(|m| m.api_version = 7);
    assert_eq!(
        reasons(&m),
        vec![Reason::UnsupportedApiVersion { found: 7 }]
    );
}

#[test]
fn min_app_version_newer_than_the_app_is_incompatible() {
    let m = with(|m| m.min_app_version = Some("0.2.0".into()));
    assert_eq!(
        reasons(&m),
        vec![Reason::AppTooOld {
            required: Version::new(0, 2, 0),
            current: app()
        }]
    );
    let message = reasons(&m)[0].to_string();
    assert!(
        message.contains("0.2.0") && message.contains(&app().to_string()),
        "{message}"
    );
}

#[test]
fn min_app_version_equal_or_older_is_compatible() {
    for required in [app().to_string().as_str(), "0.1.0", "0.0.1"] {
        let m = with(|m| m.min_app_version = Some(required.into()));
        assert!(m.compatibility(&app()).is_compatible(), "{required}");
    }
}

#[test]
fn min_app_version_must_be_semver() {
    let m = with(|m| m.min_app_version = Some("soon".into()));
    assert_eq!(single_invalid(&m).0, "min_app_version");
}

#[test]
fn all_reasons_are_collected() {
    let m = with(|m| {
        m.api_version = 9;
        m.min_app_version = Some("9.0.0".into());
        m.provides.clear();
        m.id = "Bad Id".into();
    });
    assert_eq!(reasons(&m).len(), 4, "{:?}", reasons(&m));
}

#[test]
fn id_must_match_the_pattern() {
    for good in ["ab", "a-b", "0day", "open-library", &"a".repeat(40)] {
        let m = with(|m| m.id = good.to_owned());
        assert!(m.compatibility(&app()).is_compatible(), "{good}");
    }
    for bad in [
        "",
        "a",
        "-ab",
        "Ab",
        "a_b",
        "a b",
        "a.b",
        "../x",
        "абв",
        &"a".repeat(41),
    ] {
        let m = with(|m| m.id = bad.to_owned());
        assert_eq!(single_invalid(&m).0, "id", "{bad:?}");
    }
}

#[test]
fn version_must_be_semver() {
    for bad in ["", "1", "1.0", "v1.0.0", "latest"] {
        let m = with(|m| m.version = bad.to_owned());
        assert_eq!(single_invalid(&m).0, "version", "{bad:?}");
    }
    let m = with(|m| m.version = "1.2.3-beta.1+build5".into());
    assert!(m.compatibility(&app()).is_compatible());
}

#[test]
fn text_fields_obey_their_limits() {
    type Case = (&'static str, fn(&mut Manifest, String), usize);
    let cases: [Case; 5] = [
        ("name", |m, v| m.name = v, MAX_NAME_CHARS),
        ("author", |m, v| m.author = Some(v), MAX_AUTHOR_CHARS),
        ("license", |m, v| m.license = Some(v), MAX_LICENSE_CHARS),
        (
            "description",
            |m, v| m.description = Some(v),
            MAX_DESCRIPTION_CHARS,
        ),
        (
            "search_hint",
            |m, v| m.search_hint = Some(v),
            MAX_SEARCH_HINT_CHARS,
        ),
    ];
    for (field, set, max) in cases {
        let at_limit = with(|m| set(m, "я".repeat(max)));
        assert!(
            at_limit.compatibility(&app()).is_compatible(),
            "{field} at the limit"
        );
        let over = with(|m| set(m, "я".repeat(max + 1)));
        assert_eq!(single_invalid(&over).0, field, "{field} over the limit");
        let control = with(|m| set(m, "line\nbreak".into()));
        assert_eq!(
            single_invalid(&control).0,
            field,
            "{field} with a line break"
        );
        let blank = with(|m| set(m, "  ".into()));
        assert_eq!(single_invalid(&blank).0, field, "{field} blank");
    }
}

#[test]
fn homepage_must_be_an_http_url() {
    for bad in [
        "example.org",
        "ftp://example.org",
        "javascript:alert(1)",
        "https://u:p@example.org/",
    ] {
        let m = with(|m| m.homepage = Some(bad.into()));
        assert_eq!(single_invalid(&m).0, "homepage", "{bad}");
    }
}

#[test]
fn entry_must_be_a_plain_js_file_name() {
    for bad in [
        "",
        "plugin",
        "plugin.py",
        "../plugin.js",
        "sub/plugin.js",
        "c:plugin.js",
        ".js",
        ".hidden.js",
        "a\\b.js",
    ] {
        let m = with(|m| m.entry = bad.into());
        assert_eq!(single_invalid(&m).0, "entry", "{bad:?}");
    }
    let m = with(|m| m.entry = "main.js".into());
    assert!(m.compatibility(&app()).is_compatible());
}

#[test]
fn sha256_must_be_64_hex_digits() {
    for bad in ["", "abc", &"g".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
        let m = with(|m| m.sha256 = Some(bad.into()));
        assert_eq!(single_invalid(&m).0, "sha256", "{bad:?}");
    }
    let m = with(|m| m.sha256 = Some("A".repeat(64)));
    assert!(m.compatibility(&app()).is_compatible());
}

#[test]
fn provides_and_content_lists_are_limited() {
    let m = with(|m| {
        m.provides = (0..=MAX_PROVIDES).map(|i| format!("kind{i}")).collect();
        m.provides.push("search".into());
    });
    assert_eq!(single_invalid(&m).0, "provides");
    let m = with(|m| m.content = (0..=MAX_CONTENT_TAGS).map(|i| format!("t{i}")).collect());
    assert_eq!(single_invalid(&m).0, "content");
    let m = with(|m| m.content = vec!["x".repeat(MAX_CONTENT_TAG_CHARS + 1)]);
    assert_eq!(single_invalid(&m).0, "content[0]");
}

#[test]
fn domains_must_be_plain_hosts_or_a_leading_wildcard() {
    for good in [
        "example.org",
        "a.b.example.org",
        "*.example.org",
        "EXAMPLE.org",
        "xn--80ak6aa92e.com",
    ] {
        let m = with(|m| m.network.domains = vec![good.into()]);
        assert!(m.compatibility(&app()).is_compatible(), "{good}");
    }
    for bad in [
        "",
        "localhost",
        "*",
        "*.org",
        "*.*.example.org",
        "ex*mple.org",
        "example.*",
        "http://example.org",
        "example.org/path",
        "example.org:8080",
        "user@example.org",
        "127.0.0.1",
        "1.2.3",
        "[::1]",
        "-bad.example.org",
        "bad-.example.org",
        "exa mple.org",
        "пример.рф",
        "a..org",
        ".example.org",
        "example.org.",
    ] {
        let m = with(|m| m.network.domains = vec![bad.into()]);
        assert_eq!(single_invalid(&m).0, "network.domains[0]", "{bad:?}");
    }
}

#[test]
fn too_many_domains_are_rejected() {
    let m = with(|m| {
        m.network.domains = (0..=MAX_DOMAINS)
            .map(|i| format!("d{i}.example.org"))
            .collect()
    });
    assert_eq!(single_invalid(&m).0, "network.domains");
}

#[test]
fn call_timeout_must_be_between_5_and_60() {
    for good in [5, 30, 60] {
        let m = with(|m| m.limits.call_timeout_s = Some(good));
        assert!(m.compatibility(&app()).is_compatible(), "{good}");
        assert_eq!(m.call_timeout(), Duration::from_secs(u64::from(good)));
    }
    for bad in [0, 4, 61, 3600] {
        let m = with(|m| m.limits.call_timeout_s = Some(bad));
        assert_eq!(single_invalid(&m).0, "limits.call_timeout_s", "{bad}");
    }
}

fn setting(kind: SettingKind) -> Setting {
    Setting {
        key: "key".into(),
        kind,
        label: "Label".into(),
        network: false,
        default: None,
        options: Vec::new(),
    }
}

fn setting_problem(setting: Setting) -> (String, String) {
    single_invalid(&with(|m| m.settings = vec![setting]))
}

#[test]
fn setting_keys_are_validated_and_unique() {
    for bad in ["", "Key", "1key", "ke-y", "k y", &"k".repeat(41)] {
        let s = Setting {
            key: bad.into(),
            ..setting(SettingKind::String)
        };
        assert_eq!(setting_problem(s).0, "settings[0].key", "{bad:?}");
    }
    let m = with(|m| m.settings = vec![setting(SettingKind::String), setting(SettingKind::Bool)]);
    assert_eq!(single_invalid(&m).0, "settings[1].key");
}

#[test]
fn setting_label_is_limited() {
    let s = Setting {
        label: "x".repeat(MAX_SETTING_LABEL_CHARS + 1),
        ..setting(SettingKind::String)
    };
    assert_eq!(setting_problem(s).0, "settings[0].label");
}

#[test]
fn too_many_settings_are_rejected() {
    let m = with(|m| {
        m.settings = (0..=MAX_SETTINGS)
            .map(|i| Setting {
                key: format!("k{i}"),
                ..setting(SettingKind::String)
            })
            .collect()
    });
    assert_eq!(single_invalid(&m).0, "settings");
}

#[test]
fn network_flag_is_for_url_settings_only() {
    for kind in [SettingKind::String, SettingKind::Bool, SettingKind::Choice] {
        let s = Setting {
            network: true,
            options: vec!["a".into()],
            ..setting(kind)
        };
        let problems = reasons(&with(|m| m.settings = vec![s]));
        assert!(
            problems.iter().any(
                |r| matches!(r, Reason::Invalid { field, .. } if field == "settings[0].network")
            ),
            "{kind:?}: {problems:?}"
        );
    }
    let s = Setting {
        network: true,
        ..setting(SettingKind::Url)
    };
    assert!(
        with(|m| m.settings = vec![s])
            .compatibility(&app())
            .is_compatible()
    );
}

#[test]
fn setting_defaults_must_match_the_type() {
    let cases = [
        (SettingKind::Bool, SettingValue::Text("yes".into())),
        (SettingKind::String, SettingValue::Bool(true)),
        (SettingKind::Url, SettingValue::Bool(true)),
        (SettingKind::Url, SettingValue::Text("not a url".into())),
        (
            SettingKind::Url,
            SettingValue::Text("ftp://example.org".into()),
        ),
        (
            SettingKind::String,
            SettingValue::Text("x".repeat(MAX_SETTING_TEXT_CHARS + 1)),
        ),
    ];
    for (kind, default) in cases {
        let s = Setting {
            default: Some(default.clone()),
            ..setting(kind)
        };
        assert_eq!(
            setting_problem(s).0,
            "settings[0].default",
            "{kind:?} {default:?}"
        );
    }
}

#[test]
fn choice_settings_need_unique_options_and_a_default_among_them() {
    let no_options = setting(SettingKind::Choice);
    assert_eq!(setting_problem(no_options).0, "settings[0].options");

    let duplicate = Setting {
        options: vec!["a".into(), "a".into()],
        ..setting(SettingKind::Choice)
    };
    assert_eq!(setting_problem(duplicate).0, "settings[0].options[1]");

    let too_many = Setting {
        options: (0..=MAX_SETTING_OPTIONS).map(|i| format!("o{i}")).collect(),
        ..setting(SettingKind::Choice)
    };
    assert_eq!(setting_problem(too_many).0, "settings[0].options");

    let stray_default = Setting {
        options: vec!["a".into()],
        default: Some(SettingValue::Text("b".into())),
        ..setting(SettingKind::Choice)
    };
    assert_eq!(setting_problem(stray_default).0, "settings[0].default");

    let not_a_choice = Setting {
        options: vec!["a".into()],
        ..setting(SettingKind::String)
    };
    assert_eq!(setting_problem(not_a_choice).0, "settings[0].options");
}

#[test]
fn unknown_setting_type_is_a_parse_error() {
    let text = format!(
        "{}\n[[settings]]\nkey = \"n\"\ntype = \"number\"\nlabel = \"N\"\n",
        minimal()
    );
    assert!(matches!(
        Manifest::parse(&text),
        Err(ManifestError::Invalid(_))
    ));
}

#[test]
fn reasons_read_as_sentences_for_the_user() {
    let m = with(|m| m.id = "Bad".into());
    let message = reasons(&m)[0].to_string();
    assert!(message.starts_with("id must be"), "{message}");
}

#[test]
fn manifest_survives_a_toml_round_trip() {
    let m = manifest(VALID);
    let text = toml::to_string(&m).unwrap();
    assert_eq!(manifest(&text), m);
}
