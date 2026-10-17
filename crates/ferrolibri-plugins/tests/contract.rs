//! Public-surface checks that do not belong to a single module.

use ferrolibri_plugins::{
    Compatibility, DomainRule, HostErrorKind, Manifest, SUPPORTED_API_VERSIONS, app_version,
    plugin_error,
};

#[test]
fn supported_api_versions_start_at_one() {
    assert_eq!(SUPPORTED_API_VERSIONS, [1]);
}

#[test]
fn the_app_version_is_the_workspace_version() {
    assert_eq!(app_version().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_manifest_for_the_current_app_is_compatible() {
    let manifest = Manifest::parse(&format!(
        "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0.0\"\napi_version = 1\n\
         provides = [\"search\"]\nmin_app_version = \"{}\"\n",
        app_version()
    ))
    .unwrap();
    assert_eq!(
        manifest.compatibility(&app_version()),
        Compatibility::Compatible
    );
}

#[test]
fn host_error_kinds_use_the_names_of_the_contract() {
    let names: Vec<_> = HostErrorKind::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(
        names,
        [
            "timeout",
            "connect",
            "dns",
            "tls",
            "blocked_domain",
            "too_large",
            "too_many_requests",
            "cancelled"
        ]
    );
    for kind in HostErrorKind::ALL {
        assert_eq!(kind.as_str().parse(), Ok(kind));
        assert_eq!(kind.to_string(), kind.as_str());
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!("\"{}\"", kind.as_str())
        );
        assert_eq!(
            serde_json::from_str::<HostErrorKind>(&format!("\"{}\"", kind.as_str())).unwrap(),
            kind
        );
    }
    assert!("Timeout".parse::<HostErrorKind>().is_err());
    assert!("other".parse::<HostErrorKind>().is_err());
}

#[test]
fn domain_rules_match_hosts_and_subdomains() {
    let exact = DomainRule::parse("Example.org").unwrap();
    assert!(exact.matches("example.org"));
    assert!(exact.matches("EXAMPLE.ORG"));
    assert!(!exact.matches("www.example.org"));
    assert!(!exact.matches("badexample.org"));

    let wildcard = DomainRule::parse("*.example.org").unwrap();
    assert!(wildcard.matches("static.example.org"));
    assert!(wildcard.matches("a.b.example.org"));
    assert!(!wildcard.matches("example.org"));
    assert!(!wildcard.matches("badexample.org"));
    assert!(!wildcard.matches("example.org.evil.net"));
    assert!(!wildcard.matches(".example.org"));
}

#[test]
fn plugin_errors_name_the_plugin() {
    let error = plugin_error("run plugin", "demo", "returned garbage");
    assert!(matches!(
        error.kind,
        ferrolibri_core::ErrorKind::Plugin { ref id, .. } if id == "demo"
    ));
    assert_eq!(
        error.to_string(),
        "run plugin: plugin demo: returned garbage"
    );
    let network = ferrolibri_core::Error {
        operation: "fetch",
        path: None,
        kind: ferrolibri_core::ErrorKind::Network,
    };
    assert_eq!(network.to_string(), "fetch: network error");
}
