//! The `ferrolibri-plugin-check` binary, run as an author would run it. The network is a local
//! server addressed by IP (an address the user types is allowed, D4); the live services are
//! checked by hand.

use std::{path::PathBuf, process::Command};

use ferrolibri_plugins::testing::{TestServer, opds};

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn check(args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_ferrolibri-plugin-check"))
        .args(args)
        .output()
        .expect("the checker starts");
    Run {
        code: output.status.code().expect("the checker exits normally"),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// A plugin folder in a temporary directory.
fn plugin(manifest: &str, script: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    std::fs::write(dir.path().join("plugin.js"), script).unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

const MANIFEST: &str = "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0.0\"\napi_version = 1\n\
                        provides = [\"search\"]\n";

#[test]
fn without_a_query_only_the_files_are_checked() {
    let run = check(&[&fixture("sample-opds")]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(
        run.out
            .contains("Sample OPDS catalog (sample-opds 0.1.0), plugin API 1")
    );
    assert!(run.out.contains("compatible with FerroLibri"));
    assert!(
        run.out
            .contains("reaches the address the user enters in: Catalog address")
    );
    assert!(run.out.contains("Search: skipped"));
    assert!(run.out.contains("Result: OK"));
}

#[test]
fn a_search_over_the_network_reports_results_requests_and_time() {
    let server = TestServer::start(opds::handle);
    let address = format!("base_url={}", server.ip_url("/"));
    let run = check(&[
        &fixture("sample-opds"),
        "--query",
        "war",
        "--setting",
        &address,
    ]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(run.out.contains("Search \"war\", page 1:"), "{}", run.out);
    assert!(
        run.out.contains("result(s) accepted, 0 dropped"),
        "{}",
        run.out
    );
    // The sample plugin searches in two steps: the menu, then the results.
    assert!(
        run.out.contains("cost: 2 request(s) of 20 allowed"),
        "{}",
        run.out
    );
    assert!(run.out.contains("s of 30 s allowed"), "{}", run.out);
    assert!(run.out.contains("Result: OK"));
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_setting_the_manifest_does_not_declare_or_a_bad_value_is_a_problem() {
    for (setting, expected) in [
        ("nope=1", "declares no such setting"),
        ("loud=maybe", "use true or false"),
    ] {
        let run = check(&[&fixture("working"), "--query", "x", "--setting", setting]);
        assert_eq!(run.code, 1, "{}", run.out);
        assert!(
            run.out.contains("PROBLEM") && run.out.contains(expected),
            "{}",
            run.out
        );
    }
}

#[test]
fn dropped_results_and_the_plugins_log_are_warnings_not_failures() {
    // `working` hands out covers on a host its manifest does not name: the app would drop them.
    let run = check(&[&fixture("working"), "--query", "x"]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(
        run.out.contains("0 result(s) accepted, 2 dropped"),
        "{}",
        run.out
    );
    assert!(run.out.contains("warning: dropped hits[0]"), "{}", run.out);
    assert!(run.out.contains("no network.domains"), "{}", run.out);
    assert!(
        run.out.contains("log info: searching for x on main"),
        "{}",
        run.out
    );
    assert!(run.out.contains("Result: OK, "), "{}", run.out);

    let run = check(&[&fixture("wrong-shape"), "--query", "mixed"]);
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(
        run.out.contains("1 result(s) accepted, 2 dropped"),
        "{}",
        run.out
    );
}

#[test]
fn an_answer_that_breaks_the_contract_or_a_throwing_plugin_fails() {
    let run = check(&[&fixture("wrong-shape"), "--query", "hits-not-array"]);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(run.out.contains("rejected as a whole"), "{}", run.out);
    assert!(
        run.out.contains("Result: FAILED, 1 problem(s)"),
        "{}",
        run.out
    );

    let run = check(&[&fixture("throws"), "--query", "x"]);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(run.out.contains("search failed"), "{}", run.out);
    assert!(
        run.out.contains("the catalog format changed"),
        "{}",
        run.out
    );
}

#[test]
fn a_plugin_that_cannot_be_installed_is_reported_before_anything_runs() {
    let run = check(&[&fixture("no-such-folder"), "--query", "x"]);
    assert_eq!(run.code, 1);
    assert!(run.out.contains("PROBLEM"), "{}", run.out);
    assert!(!run.out.contains("Search \"x\""), "{}", run.out);

    // An API version the app does not run: the reason is shown, nothing is run.
    let (_dir, path) = plugin(&MANIFEST.replace("api_version = 1", "api_version = 99"), "");
    let run = check(&[&path, "--query", "x"]);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(run.out.contains("PROBLEM"), "{}", run.out);

    // The declared hash must match the script.
    let wrong = format!("{MANIFEST}sha256 = \"{}\"\n", "0".repeat(64));
    let (_dir, path) = plugin(&wrong, "export function search() { return { hits: [] }; }");
    let run = check(&[&path, "--query", "x"]);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(run.out.contains("sha256 of plugin.js is "), "{}", run.out);
}

#[test]
fn a_matching_hash_is_confirmed_and_a_missing_one_is_printed_for_the_author() {
    let script = "export function search() { return { hits: [] }; }";
    let (_dir, path) = plugin(MANIFEST, script);
    let run = check(&[&path]);
    assert_eq!(run.code, 0, "{}", run.out);
    let line = run
        .out
        .lines()
        .find(|l| l.contains("sha256 of plugin.js:"))
        .unwrap();
    let digest = line.split_whitespace().nth(3).unwrap();
    assert_eq!(digest.len(), 64);

    let (_dir, path) = plugin(&format!("{MANIFEST}sha256 = \"{digest}\"\n"), script);
    let run = check(&[&path]);
    assert_eq!(run.code, 0, "{}", run.out);
    assert!(run.out.contains("matches plugin.toml"), "{}", run.out);
}

#[test]
fn a_wrong_command_line_exits_with_2_and_the_usage() {
    for args in [&[][..], &["folder", "--page", "0"], &["folder", "--what"]] {
        let run = check(args);
        assert_eq!(run.code, 2, "{args:?}");
        assert!(
            run.err.contains("usage: ferrolibri-plugin-check"),
            "{}",
            run.err
        );
    }
}
