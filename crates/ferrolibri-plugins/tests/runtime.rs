//! The plugin runtime through its public API: fixtures in `tests/fixtures/`, a local server for
//! the network, and no real site.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use ferrolibri_core::{ErrorKind, converter::CancellationToken};
use ferrolibri_plugins::{
    Archive, LoadedPlugin, LogLevel, LogRecord, Manifest, PluginFunction, PluginRuntime, SearchHit,
    SettingValue, app_version, merge_details, parse_download_spec, parse_search_page,
    testing::{Reply, TestServer, opds},
};
use serde_json::{Value, json};

/// The time cap of most calls: well under the 5 s minimum a manifest can ask for.
const CAP: Duration = Duration::from_secs(2);

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, SettingValue> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), SettingValue::Text((*v).to_owned())))
        .collect()
}

fn fixture(name: &str, stored: &[(&str, &str)]) -> LoadedPlugin {
    let dir = fixture_dir(name);
    let manifest = Manifest::parse(&std::fs::read_to_string(dir.join("plugin.toml")).unwrap())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(
        manifest.compatibility(&app_version()).is_compatible(),
        "{name} is not compatible"
    );
    let source = std::fs::read_to_string(dir.join(&manifest.entry)).unwrap();
    LoadedPlugin {
        manifest,
        source,
        settings: settings(stored),
    }
}

/// A plugin written inline for a small experiment; `stored` are the settings the user saved.
fn inline(source: &str, stored: &[(&str, &str)]) -> LoadedPlugin {
    let manifest = Manifest::parse(
        "id = \"inline\"\nname = \"Inline\"\nversion = \"0.1.0\"\napi_version = 1\n\
         provides = [\"search\"]\n\
         [[settings]]\nkey = \"base_url\"\ntype = \"url\"\nlabel = \"Address\"\nnetwork = true\n\
         [[settings]]\nkey = \"plain\"\ntype = \"string\"\nlabel = \"Plain\"\ndefault = \"d\"\n",
    )
    .unwrap();
    LoadedPlugin {
        manifest,
        source: source.to_owned(),
        settings: settings(stored),
    }
}

type Logs = Arc<Mutex<Vec<LogRecord>>>;

fn runtime(server: &TestServer) -> (PluginRuntime, Logs) {
    let logs = Logs::default();
    let sink = Arc::clone(&logs);
    let runtime = PluginRuntime::new(server.http())
        .with_max_call_time(CAP)
        .with_log_sink(Arc::new(move |record| sink.lock().unwrap().push(record)));
    (runtime, logs)
}

fn idle_server() -> TestServer {
    TestServer::start(|_, _| Reply::status(404))
}

fn query(text: &str) -> Value {
    json!({ "text": text, "page": 1 })
}

fn search(runtime: &PluginRuntime, plugin: &LoadedPlugin, text: &str) -> Result<Value, String> {
    call(runtime, plugin, PluginFunction::Search, &query(text))
}

fn call(
    runtime: &PluginRuntime,
    plugin: &LoadedPlugin,
    function: PluginFunction,
    arg: &Value,
) -> Result<Value, String> {
    runtime
        .call(plugin, function, arg, &CancellationToken::default())
        .map_err(|e| match e.kind {
            ErrorKind::Plugin { id, message } => {
                assert_eq!(id, plugin.manifest.id);
                message
            }
            other => panic!("not a plugin error: {other:?}"),
        })
}

fn titles(value: &Value) -> Vec<String> {
    value["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["title"].as_str().unwrap().to_owned())
        .collect()
}

fn allow_any(_: &str) -> bool {
    true
}

// ---- a working plugin ----------------------------------------------------------------------

#[test]
fn a_working_plugin_answers_and_its_answer_passes_validation() {
    let (runtime, logs) = runtime(&idle_server());
    let plugin = fixture("working", &[]);
    let value = search(&runtime, &plugin, "war").unwrap();
    let page = parse_search_page(&value.to_string(), &allow_any).unwrap();
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    assert_eq!(page.page.hits.len(), 2);
    assert_eq!(page.page.hits[0].title, "One & Two");
    assert_eq!(page.page.hits[0].authors, ["Atom title"]);
    // host.url.resolve: relative, and scheme-relative against a base with a path.
    assert_eq!(
        page.page.hits[0].page_url.as_deref(),
        Some("https://books.example.org/b/1")
    );
    assert_eq!(
        page.page.hits[1].page_url.as_deref(),
        Some("https://books.example.org/catalog/b/2")
    );
    assert_eq!(
        page.page.hits[1].cover_url.as_deref(),
        Some("https://books.example.org/cover/2.jpg")
    );
    assert_eq!(page.page.next_cursor.as_deref(), Some("page-2"));
    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].plugin, "test-working");
    assert_eq!(logs[0].level, LogLevel::Info);
    // The setting was not stored: the manifest default reaches the plugin.
    assert_eq!(logs[0].message, "searching for war on main");
}

#[test]
fn stored_settings_reach_the_plugin_and_unknown_ones_do_not() {
    let (runtime, logs) = runtime(&idle_server());
    let mut plugin = fixture("working", &[("shelf", "top"), ("undeclared", "x")]);
    plugin
        .settings
        .insert("loud".into(), SettingValue::Bool(true));
    let value = search(&runtime, &plugin, "war").unwrap();
    assert_eq!(titles(&value), ["One & Two!", "Three!"]);
    assert_eq!(logs.lock().unwrap()[0].message, "searching for war on top");
    let echo = inline(
        "export function search(q, ctx) { return ctx; }",
        &[("plain", "mine"), ("undeclared", "x")],
    );
    assert_eq!(
        search(&runtime, &echo, "").unwrap(),
        json!({ "settings": { "plain": "mine" } })
    );
}

#[test]
fn the_cursor_and_page_reach_the_plugin_as_given() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline("export function search(q, ctx) { return q; }", &[]);
    let arg = json!({ "text": "т", "page": 3, "cursor": "c-3" });
    assert_eq!(
        call(&runtime, &plugin, PluginFunction::Search, &arg).unwrap(),
        arg
    );
}

#[test]
fn details_and_resolve_download_are_validated_by_the_contract_types() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("working", &[("shelf", "top")]);
    let value = search(&runtime, &plugin, "war").unwrap();
    let page = parse_search_page(&value.to_string(), &allow_any).unwrap();
    let hit = &page.page.hits[0];

    let details = call(
        &runtime,
        &plugin,
        PluginFunction::Details,
        &serde_json::to_value(hit).unwrap(),
    )
    .unwrap();
    let merged = merge_details(hit, &details.to_string(), &allow_any).unwrap();
    assert_eq!(merged.description.as_deref(), Some("Details of One & Two"));
    assert_eq!(merged.series.as_deref(), Some("Series top"));
    assert_eq!(merged.id, hit.id);

    let offer = serde_json::to_value(&hit.offers[0]).unwrap();
    let resolved = call(&runtime, &plugin, PluginFunction::ResolveDownload, &offer).unwrap();
    let spec = parse_download_spec(&resolved.to_string(), &hit.offers[0], &allow_any).unwrap();
    assert_eq!(spec.url, "https://books.example.org/files/o1.epub");
    assert_eq!(spec.archive, None);
}

#[test]
fn missing_functions_are_handled_by_their_kind() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline(
        "export function search(q, ctx) { return { hits: [] }; }",
        &[],
    );
    // `details` is optional: nothing to add.
    assert_eq!(
        call(&runtime, &plugin, PluginFunction::Details, &json!({})).unwrap(),
        Value::Null
    );
    let error = call(
        &runtime,
        &plugin,
        PluginFunction::ResolveDownload,
        &json!({}),
    )
    .unwrap_err();
    assert!(
        error.contains("does not export a function resolveDownload"),
        "{error}"
    );
    let none = inline("export const unrelated = 1;", &[]);
    let error = search(&runtime, &none, "x").unwrap_err();
    assert!(
        error.contains("does not export a function search"),
        "{error}"
    );
    let not_a_function = inline("export const search = 5;", &[]);
    assert!(search(&runtime, &not_a_function, "x").is_err());
}

#[test]
fn every_call_starts_from_a_clean_runtime() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline(
        "globalThis.counter = (globalThis.counter || 0) + 1;\n\
         export function search(q, ctx) { return { n: globalThis.counter }; }",
        &[],
    );
    for _ in 0..3 {
        assert_eq!(search(&runtime, &plugin, "").unwrap(), json!({ "n": 1 }));
    }
}

#[test]
fn calls_run_in_parallel_without_disturbing_each_other() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("working", &[]);
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let (runtime, plugin) = (runtime.clone(), plugin.clone());
            thread::spawn(move || search(&runtime, &plugin, &format!("q{n}")).unwrap())
        })
        .collect();
    for handle in threads {
        assert_eq!(titles(&handle.join().unwrap()), ["One & Two", "Three"]);
    }
}

// ---- limits --------------------------------------------------------------------------------

fn assert_stops_within(limit: Duration, run: impl FnOnce() -> Result<Value, String>) -> String {
    let started = Instant::now();
    let error = run().expect_err("the call must fail");
    let took = started.elapsed();
    assert!(took < limit, "took {took:?}, the limit was {limit:?}");
    error
}

#[test]
fn an_infinite_loop_is_stopped_by_the_time_limit() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("infinite-loop", &[]);
    let error = assert_stops_within(Duration::from_secs(5), || search(&runtime, &plugin, "x"));
    assert!(error.contains("did not finish within 2 s"), "{error}");
}

#[test]
fn the_time_limit_covers_loading_the_script_too() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline("while (true) {}\nexport function search() {}", &[]);
    let error = assert_stops_within(Duration::from_secs(5), || search(&runtime, &plugin, "x"));
    assert!(error.contains("did not finish"), "{error}");
}

#[test]
fn a_manifest_time_limit_applies_without_a_cap() {
    // The fixture asks for 5 s, the minimum a manifest can ask for.
    let runtime = PluginRuntime::new(idle_server().http());
    let plugin = fixture("infinite-loop", &[]);
    assert_eq!(plugin.manifest.call_timeout(), Duration::from_secs(5));
    let error = assert_stops_within(Duration::from_secs(9), || search(&runtime, &plugin, "x"));
    assert!(error.contains("did not finish within 5 s"), "{error}");
}

#[test]
fn cancelling_stops_a_running_call_quickly() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("infinite-loop", &[]);
    let cancel = CancellationToken::default();
    let trigger = cancel.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        trigger.cancel();
    });
    let started = Instant::now();
    let error = runtime
        .call(&plugin, PluginFunction::Search, &query("x"), &cancel)
        .unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(error.kind, ErrorKind::Cancelled), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_call_cancelled_before_it_starts_does_nothing() {
    let (runtime, logs) = runtime(&idle_server());
    let cancel = CancellationToken::default();
    cancel.cancel();
    let error = runtime
        .call(
            &fixture("working", &[]),
            PluginFunction::Search,
            &query("x"),
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(error.kind, ErrorKind::Cancelled));
    assert!(logs.lock().unwrap().is_empty());
}

#[test]
fn cancelling_interrupts_a_request_that_waits_for_the_server() {
    let server = TestServer::start(|_, _| Reply::silent());
    let (runtime, _) = runtime(&server);
    let plugin = fixture("sample-opds", &[("base_url", &server.url("/"))]);
    let cancel = CancellationToken::default();
    let trigger = cancel.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        trigger.cancel();
    });
    let started = Instant::now();
    let error = runtime
        .call(&plugin, PluginFunction::Search, &query("x"), &cancel)
        .unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(error.kind, ErrorKind::Cancelled), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_request_cannot_outlive_the_call() {
    let server = TestServer::start(|_, _| Reply::silent());
    let (runtime, _) = runtime(&server);
    let plugin = fixture("sample-opds", &[("base_url", &server.url("/"))]);
    let error = assert_stops_within(Duration::from_secs(6), || search(&runtime, &plugin, "x"));
    // Either the request ended with a time-out the plugin did not catch, or the call's own clock did.
    assert!(
        error.contains("Cannot connect") || error.contains("did not finish"),
        "{error}"
    );
}

#[test]
fn a_plugin_that_eats_memory_fails_instead_of_growing() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("memory-hog", &[]);
    let error = assert_stops_within(Duration::from_secs(10), || search(&runtime, &plugin, "x"));
    assert!(error.contains("more memory than the 64 MiB"), "{error}");
}

#[test]
fn a_plugin_that_catches_the_memory_error_still_ends_on_time() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline(
        "export function search() { const keep = [];\n\
         while (true) { try { keep.push(new Array(1e6).fill(1)); } catch (e) { keep.length = 0; } } }",
        &[],
    );
    assert_stops_within(Duration::from_secs(6), || search(&runtime, &plugin, "x"));
}

#[test]
fn hostile_scripts_all_fail_within_the_limits() {
    let (runtime, _) = runtime(&idle_server());
    let scripts = [
        (
            "deep recursion",
            "function f() { return f() + 1; } export function search() { return f(); }",
        ),
        (
            "huge string",
            "export function search() { return 'x'.repeat(2 ** 30).repeat(64); }",
        ),
        (
            "array of a billion",
            "export function search() { return new Array(1e9).fill(0).join(''); }",
        ),
        (
            "catastrophic regular expression",
            "export function search() { return /(a+)+$/.test('a'.repeat(60) + 'b'); }",
        ),
        (
            "endless generator",
            "export function search() { function* g() { while (true) yield 1; } return [...g()]; }",
        ),
        (
            "endless promise chain",
            "export function search() { let p = Promise.resolve(); while (true) p = p.then(() => 1); }",
        ),
    ];
    for (name, source) in scripts {
        let plugin = inline(source, &[]);
        let started = Instant::now();
        let outcome = search(&runtime, &plugin, "x");
        assert!(outcome.is_err(), "{name}: {outcome:?}");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{name} took {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn a_result_over_two_mebibytes_is_refused() {
    let server = TestServer::start(|request, _| {
        if request.path == "/huge" {
            Reply::repeated("text/plain", b'x', 9 * 1024 * 1024)
        } else {
            Reply::status(404)
        }
    });
    let (runtime, _) = runtime(&server);
    let plugin = fixture("huge", &[("base_url", &server.url("/"))]);
    let error = search(&runtime, &plugin, "result").unwrap_err();
    assert!(error.contains("larger than 2097152 bytes"), "{error}");
}

#[test]
fn a_response_over_eight_mebibytes_is_a_too_large_error_the_plugin_can_catch() {
    let server = TestServer::start(|request, _| {
        if request.path == "/huge" {
            Reply::repeated("text/plain", b'x', 9 * 1024 * 1024)
        } else {
            Reply::status(404)
        }
    });
    let (runtime, _) = runtime(&server);
    let plugin = fixture("huge", &[("base_url", &server.url("/"))]);
    let error = search(&runtime, &plugin, "response").unwrap_err();
    assert!(error.contains("network error (too_large)"), "{error}");
    let catching = inline(
        "export function search(q, ctx) { try { host.fetch(host.url.resolve(ctx.settings.base_url, '/huge')); }\n\
         catch (e) { return { kind: e.kind }; } }",
        &[("base_url", &server.url("/"))],
    );
    assert_eq!(
        search(&runtime, &catching, "").unwrap(),
        json!({ "kind": "too_large" })
    );
}

#[test]
fn at_most_twenty_requests_per_call() {
    let server = TestServer::start(|_, _| Reply::ok("text/plain", "ok"));
    let (runtime, _) = runtime(&server);
    let plugin = inline(
        "export function search(q, ctx) { let done = 0; let kind = null;\n\
         for (let i = 0; i < 30; i++) { try { host.fetch(ctx.settings.base_url); done++; }\n\
         catch (e) { kind = e.kind; break; } } return { done, kind }; }",
        &[("base_url", &server.url("/"))],
    );
    assert_eq!(
        search(&runtime, &plugin, "").unwrap(),
        json!({ "done": 20, "kind": "too_many_requests" })
    );
    assert_eq!(server.requests().len(), 20);
}

// ---- the sandbox ---------------------------------------------------------------------------

#[test]
fn the_plugin_sees_the_language_and_host_and_nothing_else() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline(
        "export function search() {\n\
           const absent = ['require', 'process', 'fs', 'console', 'fetch', 'XMLHttpRequest',\n\
             'WebSocket', 'setTimeout', 'setInterval', 'std', 'os', 'Deno', 'Buffer', 'module',\n\
             'exports', '__dirname', 'print', 'scriptArgs', 'localStorage', 'navigator', 'window']\n\
             .filter((name) => typeof globalThis[name] !== 'undefined');\n\
           return { present: absent, host: Object.keys(host).sort(),\n\
             url: Object.keys(host.url), html: Object.keys(host.html), xml: Object.keys(host.xml) };\n\
         }",
        &[],
    );
    assert_eq!(
        search(&runtime, &plugin, "").unwrap(),
        json!({
            "present": [],
            "host": ["fetch", "html", "log", "url", "xml"],
            "url": ["resolve"],
            "html": ["select"],
            "xml": ["parse"],
        })
    );
}

#[test]
fn eval_from_strings_is_switched_off_in_every_form() {
    let (runtime, _) = runtime(&idle_server());
    let attempts = [
        "eval('1 + 1')",
        "(0, eval)('1 + 1')",
        "globalThis.eval('1 + 1')",
        "new Function('return 1')()",
        "Function('return 1')()",
        "(function () {}).constructor('return 1')()",
        "(() => {}).constructor('return 1')()",
        "Object.getPrototypeOf(async function () {}).constructor('return 1')",
        "Object.getPrototypeOf(function* () {}).constructor('return 1')",
        "Object.getPrototypeOf(async function* () {}).constructor('return 1')",
        "new (Object.getPrototypeOf(function () {}).constructor)('return 1')",
        "[].constructor.constructor('return 1')()",
    ];
    for attempt in attempts {
        let plugin = inline(
            &format!("export function search() {{ return {{ value: {attempt} }}; }}"),
            &[],
        );
        let error = search(&runtime, &plugin, "").unwrap_err();
        assert!(
            error.contains("eval is not supported"),
            "{attempt}: {error}"
        );
    }
}

#[test]
fn what_scripts_legitimately_need_keeps_working_without_eval() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = inline(
        "export function search() { return {\n\
           regex: /a+/.test('caaat'), built: new RegExp('b+').test('abbb'), sticky: 'x-y'.split(/-/),\n\
           json: JSON.parse(JSON.stringify({ a: [1, { b: 2 }] })), date: typeof Date.now(),\n\
           map: new Map([[1, 2]]).size, set: new Set([1, 1]).size, template: `a${1 + 1}`,\n\
           typed: new Uint8Array(4).length, proxy: new Proxy({}, { get: () => 3 }).x,\n\
           spread: [...[1, 2], ...'ab'], cls: new (class A { x = 1 })().x, big: typeof BigInt,\n\
           url: encodeURIComponent('а б&'), nan: Number('x'), sort: [3, 1, 2].sort().join(),\n\
           generator: [...(function* () { yield 1; yield 2; })()], async: typeof (async () => 1),\n\
         }; }",
        &[],
    );
    let value = search(&runtime, &plugin, "").unwrap();
    assert_eq!(value["regex"], true);
    assert_eq!(value["built"], true);
    assert_eq!(value["sticky"], json!(["x", "y"]));
    assert_eq!(value["json"], json!({ "a": [1, { "b": 2 }] }));
    assert_eq!(value["map"], 1);
    assert_eq!(value["set"], 1);
    assert_eq!(value["template"], "a2");
    assert_eq!(value["proxy"], 3);
    assert_eq!(value["spread"], json!([1, 2, "a", "b"]));
    assert_eq!(value["url"], "%D0%B0%20%D0%B1%26");
    assert_eq!(value["generator"], json!([1, 2]));
}

#[test]
fn modules_cannot_import_anything() {
    let (runtime, _) = runtime(&idle_server());
    for source in [
        "import { x } from 'std';\nexport function search() { return x; }",
        "import * as os from 'os';\nexport function search() { return os; }",
        "import x from './other.js';\nexport function search() { return x; }",
        "export function search() { return import('./other.js'); }",
    ] {
        let plugin = inline(source, &[]);
        assert!(search(&runtime, &plugin, "").is_err(), "{source}");
    }
}

#[test]
fn host_is_the_only_door_even_when_the_plugin_tampers_with_it() {
    let (runtime, _) = runtime(&idle_server());
    // Replacing host.fetch only changes what this call sees.
    let plugin = inline(
        "host.fetch = () => ({ status: 200, body: 'forged' });\n\
         export function search() { return { body: host.fetch('http://x').body }; }",
        &[],
    );
    assert_eq!(
        search(&runtime, &plugin, "").unwrap(),
        json!({ "body": "forged" })
    );
    let again = inline(
        "export function search() { return { kind: typeof host.fetch }; }",
        &[],
    );
    assert_eq!(
        search(&runtime, &again, "").unwrap(),
        json!({ "kind": "function" })
    );
}

// ---- failures are plugin errors --------------------------------------------------------------

#[test]
fn a_thrown_exception_becomes_a_plugin_error_with_its_stack_in_the_log() {
    let (runtime, logs) = runtime(&idle_server());
    let plugin = fixture("throws", &[]);
    let error = search(&runtime, &plugin, "error").unwrap_err();
    assert_eq!(error, "TypeError: the catalog format changed");
    assert!(!error.contains(" at "), "no stack in the message: {error}");
    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].level, LogLevel::Error);
    assert!(logs[0].message.contains("the catalog format changed"));
    assert!(logs[0].message.contains("at inner"), "{}", logs[0].message);
    assert!(logs[0].message.contains("plugin.js"), "{}", logs[0].message);
}

#[test]
fn other_thrown_values_and_engine_errors_have_readable_messages() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("throws", &[]);
    assert_eq!(
        search(&runtime, &plugin, "string").unwrap_err(),
        "just a string"
    );
    assert!(
        search(&runtime, &plugin, "object")
            .unwrap_err()
            .contains("not an Error")
    );
    let error = search(&runtime, &plugin, "undefined-call").unwrap_err();
    assert!(error.contains("ReferenceError"), "{error}");
}

#[test]
fn a_syntax_error_and_a_failing_top_level_are_plugin_errors() {
    let (runtime, _) = runtime(&idle_server());
    let broken = inline("export function search( { return 1; }", &[]);
    let error = search(&runtime, &broken, "").unwrap_err();
    assert!(error.contains("plugin.js cannot be compiled"), "{error}");
    assert!(error.contains("SyntaxError"), "{error}");
    let failing = inline("throw new Error('boom');\nexport function search() {}", &[]);
    let error = search(&runtime, &failing, "").unwrap_err();
    assert!(
        error.contains("failed while loading") && error.contains("boom"),
        "{error}"
    );
    let empty = inline("", &[]);
    assert!(
        search(&runtime, &empty, "")
            .unwrap_err()
            .contains("does not export")
    );
}

#[test]
fn results_that_are_not_json_are_plugin_errors() {
    let (runtime, _) = runtime(&idle_server());
    let cases = [
        (
            "a promise",
            "export async function search() { return { hits: [] }; }",
            "synchronous",
        ),
        (
            "a cycle",
            "export function search() { const a = {}; a.self = a; return a; }",
            "cannot be converted to JSON",
        ),
        (
            "a bigint",
            "export function search() { return { n: 10n }; }",
            "cannot be converted to JSON",
        ),
        (
            "too deep for the contract",
            "export function search() { let v = {}; for (let i = 0; i < 200; i++) v = { v }; return v; }",
            "nested too deeply",
        ),
        (
            "too deep for the engine",
            "export function search() { let v = {}; for (let i = 0; i < 100000; i++) v = { v }; return v; }",
            "cannot be converted to JSON",
        ),
    ];
    for (name, source, expected) in cases {
        let plugin = inline(source, &[]);
        let error = search(&runtime, &plugin, "").unwrap_err();
        assert!(error.contains(expected), "{name}: {error}");
    }
    // `undefined` and functions say nothing: JSON null.
    let nothing = inline("export function search() {}", &[]);
    assert_eq!(search(&runtime, &nothing, "").unwrap(), Value::Null);
}

#[test]
fn a_wrong_shape_passes_the_runtime_and_stops_at_validation() {
    let (runtime, _) = runtime(&idle_server());
    let plugin = fixture("wrong-shape", &[]);
    for text in ["hits-not-array", "no-hits", "bad-cursor", "array", "string"] {
        let value = search(&runtime, &plugin, text).unwrap();
        assert!(
            parse_search_page(&value.to_string(), &allow_any).is_err(),
            "{text}: {value}"
        );
    }
    // Bad entries are dropped one by one with a warning; the good one stays.
    let value = search(&runtime, &plugin, "mixed").unwrap();
    let page = parse_search_page(&value.to_string(), &allow_any).unwrap();
    assert_eq!(page.page.hits.len(), 1);
    assert_eq!(page.warnings.len(), 2);
}

// ---- the network -----------------------------------------------------------------------------

#[test]
fn a_domain_nobody_allowed_is_blocked_and_never_contacted() {
    let server = idle_server();
    let (runtime, _) = runtime(&server);
    let plugin = fixture("forbidden-domain", &[]);
    let error = search(&runtime, &plugin, &server.url("/")).unwrap_err();
    assert!(error.contains("network error (blocked_domain)"), "{error}");
    assert!(server.requests().is_empty());
    // Not even a name that merely looks like an allowed one.
    for url in [
        "https://books.example.org.evil.test/",
        "https://evilbooks.example.org.test/",
        "ftp://books.example.org/",
        "https://user:pass@books.example.org/",
        "http://books.example.org/",
    ] {
        let error = search(&runtime, &plugin, url).unwrap_err();
        assert!(error.contains("blocked_domain"), "{url}: {error}");
    }
}

#[test]
fn loopback_and_private_addresses_are_blocked_unless_the_user_typed_them() {
    let server = idle_server();
    let (runtime, _) = runtime(&server);
    let port = server.port();
    let targets = format!(
        "http://127.0.0.1:{port}/ http://localhost:{port}/ http://[::1]:{port}/ \
         http://192.168.1.1/ http://169.254.169.254/latest/meta-data/ http://10.0.0.1/ \
         http://0.0.0.0:{port}/"
    );
    let plugin = fixture("loopback", &[]);
    let value = search(&runtime, &plugin, &targets).unwrap();
    for title in titles(&value) {
        assert!(title.ends_with("-> blocked_domain"), "{title}");
    }
    assert!(server.requests().is_empty());

    // The user typed the catalog's address: that name is allowed, nothing else.
    let plugin = fixture("loopback", &[("base_url", &server.url("/"))]);
    let value = search(
        &runtime,
        &plugin,
        &format!("{} http://127.0.0.1:{port}/", server.url("/x")),
    )
    .unwrap();
    let titles = titles(&value);
    assert!(titles[0].ends_with("-> 404"), "{titles:?}");
    assert!(titles[1].ends_with("-> blocked_domain"), "{titles:?}");
}

#[test]
fn a_default_address_is_not_an_address_the_user_typed() {
    let server = idle_server();
    let (runtime, _) = runtime(&server);
    let mut plugin = inline(
        "export function search(q, ctx) { try { return { status: host.fetch(ctx.settings.base_url).status }; }\n\
         catch (e) { return { kind: e.kind, url: ctx.settings.base_url }; } }",
        &[],
    );
    // A manifest that defaults the address to a private host...
    plugin.manifest.settings[0].default = Some(SettingValue::Text(server.url("/")));
    let value = search(&runtime, &plugin, "").unwrap();
    // ...gets the default in `ctx`, but no permission to use it.
    assert_eq!(value["kind"], "blocked_domain");
    assert_eq!(value["url"], server.url("/"));
    assert!(server.requests().is_empty());
}

#[test]
fn fetch_returns_any_status_the_final_url_and_lower_case_headers() {
    let server = TestServer::start(opds::handle);
    let (runtime, _) = runtime(&server);
    let plugin = inline(
        "export function search(q, ctx) {\n\
           const base = ctx.settings.base_url;\n\
           const bad = host.fetch(host.url.resolve(base, '/error/502'));\n\
           const missing = host.fetch(host.url.resolve(base, '/nowhere'));\n\
           const zip = host.fetch(host.url.resolve(base, '/b/12/fb2'));\n\
           return { bad: [bad.status, bad.headers['content-type'], bad.body.length > 50],\n\
             missing: missing.status, final: zip.url, type: zip.headers['content-type'],\n\
             keys: Object.keys(zip.headers).every((k) => k === k.toLowerCase()) };\n\
         }",
        &[("base_url", &server.url("/"))],
    );
    let value = search(&runtime, &plugin, "").unwrap();
    assert_eq!(value["bad"], json!([502, "text/html", true]));
    assert_eq!(value["missing"], 404);
    assert_eq!(
        value["final"],
        format!(
            "http://static.catalog.test:{}/files/12.fb2.zip",
            server.port()
        )
    );
    assert_eq!(value["type"], "application/zip");
    assert_eq!(value["keys"], true);
}

#[test]
fn fetch_accepts_get_only_and_reports_dropped_headers() {
    let server = TestServer::start(|request, _| {
        Reply::ok(
            "text/plain",
            request.header("accept").unwrap_or("-").to_owned(),
        )
    });
    let (runtime, logs) = runtime(&server);
    let plugin = inline(
        "export function search(q, ctx) {\n\
           const url = ctx.settings.base_url;\n\
           const outcome = {};\n\
           for (const [name, options] of [\n\
             ['post', { method: 'POST' }], ['body', { body: 'x' }], ['bad options', 5],\n\
             ['lower get', { method: 'get' }]]) {\n\
             try { outcome[name] = host.fetch(url, options).status; }\n\
             catch (e) { outcome[name] = e.name; }\n\
           }\n\
           outcome.accept = host.fetch(url, { headers: { Accept: 'text/x', Cookie: 'a=b', 'X-Mine': '1' } }).body;\n\
           outcome.bare = host.fetch(url, undefined).status;\n\
           return outcome;\n\
         }",
        &[("base_url", &server.url("/"))],
    );
    let value = search(&runtime, &plugin, "").unwrap();
    assert_eq!(value["post"], "TypeError");
    assert_eq!(value["body"], "TypeError");
    assert_eq!(value["bad options"], "TypeError");
    assert_eq!(value["lower get"], 200);
    assert_eq!(value["accept"], "text/x");
    assert_eq!(value["bare"], 200);
    let logs = logs.lock().unwrap();
    let dropped: Vec<_> = logs
        .iter()
        .filter(|l| l.level == LogLevel::Warn)
        .map(|l| l.message.clone())
        .collect();
    assert_eq!(dropped.len(), 2, "{dropped:?}");
    assert!(dropped.iter().any(|m| m.contains("Cookie")), "{dropped:?}");
    assert!(dropped.iter().any(|m| m.contains("X-Mine")), "{dropped:?}");
    // The requests that were refused in the runtime never reached the server.
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_body_that_is_not_utf8_reads_with_replacement_characters() {
    // windows-1251 "Война": the plugin gets U+FFFD, there is no `encoding` option in v1.
    let server = TestServer::start(|_, _| {
        Reply::ok(
            "text/html; charset=windows-1251",
            vec![0xC2, 0xEE, 0xE9, 0xED, 0xE0],
        )
    });
    let (runtime, _) = runtime(&server);
    let plugin = inline(
        "export function search(q, ctx) { const b = host.fetch(ctx.settings.base_url).body;
         return { length: b.length, replaced: b.includes('\\uFFFD') }; }",
        &[("base_url", &server.url("/"))],
    );
    assert_eq!(
        search(&runtime, &plugin, "").unwrap(),
        json!({ "length": 5, "replaced": true })
    );
}

#[test]
fn a_failing_request_is_an_exception_with_a_kind() {
    // A name that does not exist, and a redirect loop (more than 5 redirects is `connect`).
    let server = TestServer::start(|request, _| {
        if request.path == "/loop" {
            Reply::redirect(302, "/loop")
        } else {
            Reply::status(404)
        }
    });
    let (runtime, _) = runtime(&server);
    let source = "export function search(q, ctx) {
           try { host.fetch(q.text); return { ok: true }; }
           catch (e) { return { kind: e.kind, name: e.name, hasMessage: e.message.length > 0 }; }
         }";
    let typed = |address: &str| inline(source, &[("base_url", address)]);
    assert_eq!(
        search(
            &runtime,
            &typed("http://nxdomain.test/"),
            "http://nxdomain.test/"
        )
        .unwrap(),
        json!({ "kind": "dns", "name": "Error", "hasMessage": true })
    );
    let on_server = typed(&server.url("/"));
    assert_eq!(
        search(&runtime, &on_server, &server.url("/loop")).unwrap(),
        json!({ "kind": "connect", "name": "Error", "hasMessage": true })
    );
    assert_eq!(
        search(&runtime, &on_server, "not a url").unwrap()["kind"],
        "connect"
    );
    // Uncaught, the kind reaches the message.
    let uncaught = inline(
        "export function search(q, ctx) { return host.fetch('http://nxdomain.test/'); }",
        &[("base_url", "http://nxdomain.test/")],
    );
    let error = search(&runtime, &uncaught, "").unwrap_err();
    assert!(error.starts_with("network error (dns)"), "{error}");
}

#[test]
fn requests_to_one_host_never_overlap_even_from_parallel_calls() {
    let server =
        TestServer::start(|_, _| Reply::ok("text/plain", "ok").delay(Duration::from_millis(30)));
    let (runtime, _) = runtime(&server);
    // The twelve requests wait for each other (250 ms apart) and take several seconds; a slow
    // machine must not run into the short cap the other tests use.
    let runtime = runtime.with_max_call_time(Duration::from_secs(30));
    let plugin = inline(
        "export function search(q, ctx) { for (let i = 0; i < 3; i++) host.fetch(ctx.settings.base_url);\n\
         return {}; }",
        &[("base_url", &server.url("/"))],
    );
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (runtime, plugin) = (runtime.clone(), plugin.clone());
            thread::spawn(move || search(&runtime, &plugin, "").unwrap())
        })
        .collect();
    for handle in threads {
        handle.join().unwrap();
    }
    assert_eq!(server.requests().len(), 12);
    assert_eq!(server.max_concurrent_for("catalog.test"), 1);
}

// ---- the host helpers ------------------------------------------------------------------------

fn evaluate(runtime: &PluginRuntime, expression: &str) -> Result<Value, String> {
    let plugin = inline(
        &format!("export function search() {{ return {{ value: {expression} }}; }}"),
        &[],
    );
    search(runtime, &plugin, "").map(|v| v["value"].clone())
}

#[test]
fn url_resolve_follows_rfc_3986_for_the_references_catalogs_use() {
    let (runtime, _) = runtime(&idle_server());
    let base = "https://catalog.example/opds/search?term=a&x=1";
    for (reference, expected) in [
        (
            "/opds/search?searchType=books",
            "https://catalog.example/opds/search?searchType=books",
        ),
        (
            "//static.example/files/1.zip",
            "https://static.example/files/1.zip",
        ),
        ("next?page=2", "https://catalog.example/opds/next?page=2"),
        ("../b/12", "https://catalog.example/b/12"),
        ("?page=2", "https://catalog.example/opds/search?page=2"),
        ("#top", "https://catalog.example/opds/search?term=a&x=1#top"),
        ("", "https://catalog.example/opds/search?term=a&x=1"),
        ("http://other.example/a b", "http://other.example/a%20b"),
        ("/b/%D0%B0", "https://catalog.example/b/%D0%B0"),
        (
            "/книга",
            "https://catalog.example/%D0%BA%D0%BD%D0%B8%D0%B3%D0%B0",
        ),
    ] {
        let value = evaluate(
            &runtime,
            &format!("host.url.resolve({base:?}, {reference:?})"),
        )
        .unwrap();
        assert_eq!(value, expected, "resolving {reference:?}");
    }
    let error = evaluate(&runtime, "host.url.resolve('/relative', 'x')").unwrap_err();
    assert!(error.contains("not an absolute URL"), "{error}");
    assert!(evaluate(&runtime, "host.url.resolve(1)").is_err());
}

#[test]
fn html_select_and_xml_parse_report_bad_input_as_type_errors() {
    let (runtime, _) = runtime(&idle_server());
    let caught = |expression: &str| {
        evaluate(
            &runtime,
            &format!("(() => {{ try {{ {expression}; return 'no error'; }} catch (e) {{ return e.name + ': ' + e.message; }} }})()"),
        )
        .unwrap()
    };
    assert!(
        caught("host.html.select('<a>', 'a[')")
            .as_str()
            .unwrap()
            .starts_with("TypeError:")
    );
    assert!(
        caught("host.xml.parse('<a><b></a>')")
            .as_str()
            .unwrap()
            .starts_with("TypeError:")
    );
    assert!(
        caught("host.xml.parse('<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>')")
            .as_str()
            .unwrap()
            .starts_with("TypeError:")
    );
    assert!(
        caught("host.html.select(5)")
            .as_str()
            .unwrap()
            .contains("Error")
    );
    let value = evaluate(
        &runtime,
        "host.html.select('<p class=a>x</p><p>y</p>', 'p.a').map((e) => [e.text, e.html, e.attrs.class])",
    )
    .unwrap();
    assert_eq!(value, json!([["x", "<p class=\"a\">x</p>", "a"]]));
}

#[test]
fn log_levels_are_checked_and_floods_are_cut() {
    let (runtime, logs) = runtime(&idle_server());
    let plugin = inline(
        "export function search() {\n\
           host.log('debug', 'd'); host.log('warn', 42); host.log('error', 'x'.repeat(5000));\n\
           for (let i = 0; i < 1000; i++) host.log('info', 'line ' + i);\n\
           try { host.log('shout', 'x'); } catch (e) { return { name: e.name }; }\n\
         }",
        &[],
    );
    assert_eq!(
        search(&runtime, &plugin, "").unwrap(),
        json!({ "name": "TypeError" })
    );
    let logs = logs.lock().unwrap();
    assert_eq!(
        logs.len(),
        201,
        "200 messages and one note that the rest was dropped"
    );
    assert_eq!(logs[0].level, LogLevel::Debug);
    assert_eq!(logs[1].message, "42");
    assert_eq!(logs[2].message.chars().count(), 2001);
    assert!(
        logs[200].message.contains("dropped"),
        "{}",
        logs[200].message
    );
}

// ---- the sample plugin on the catalog stand-in -------------------------------------------------

fn sample(server: &TestServer) -> LoadedPlugin {
    fixture("sample-opds", &[("base_url", &server.url("/"))])
}

fn catalog_host(host: &str) -> bool {
    host.ends_with("catalog.test")
}

#[test]
fn the_sample_plugin_searches_in_two_steps_and_offers_the_zip() {
    let server = TestServer::start(opds::handle);
    let (runtime, logs) = runtime(&server);
    let value = search(&runtime, &sample(&server), "мир").unwrap();
    let page = parse_search_page(&value.to_string(), &catalog_host).unwrap();
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    assert_eq!(page.page.next_cursor, None);
    let [hit] = page.page.hits.as_slice() else {
        panic!("{:?}", page.page.hits)
    };
    assert_eq!(hit.title, "Война и мир");
    assert_eq!(hit.authors, ["Толстой, Лев"]);
    assert_eq!(hit.language.as_deref(), Some("ru"));
    let [offer] = hit.offers.as_slice() else {
        panic!("{:?}", hit.offers)
    };
    assert_eq!(offer.archive, Some(Archive::Zip));
    assert_eq!(offer.label.as_deref(), Some("FB2 (zip)"));
    assert_eq!(offer.id, server.url("/b/12/fb2"));
    // The menu, then the results: two requests, no more.
    let targets: Vec<_> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(targets.len(), 2, "{targets:?}");
    assert!(targets[0].starts_with("/opds/search?searchTerm="));
    assert!(targets[1].contains("searchType=books"));
    assert!(logs.lock().unwrap().is_empty());
}

#[test]
fn the_sample_plugin_follows_the_cursor_through_every_page() {
    let server = TestServer::start(opds::handle);
    let (runtime, _) = runtime(&server);
    let plugin = sample(&server);
    let mut all = Vec::new();
    let mut arg = query("и");
    for expected in [2, 2, 1] {
        let value = call(&runtime, &plugin, PluginFunction::Search, &arg).unwrap();
        let page = parse_search_page(&value.to_string(), &catalog_host).unwrap();
        assert_eq!(page.page.hits.len(), expected);
        all.extend(page.page.hits.iter().map(|h| h.title.clone()));
        match page.page.next_cursor {
            Some(cursor) => arg = json!({ "text": "и", "page": 2, "cursor": cursor }),
            None => assert_eq!(expected, 1, "the last page has no cursor"),
        }
    }
    assert_eq!(all.len(), 5);
    // The first page cost the menu and a feed, every later page one feed.
    assert_eq!(server.requests().len(), 4);
}

#[test]
fn the_sample_plugin_adds_details_and_resolves_the_download() {
    let server = TestServer::start(opds::handle);
    let (runtime, _) = runtime(&server);
    let plugin = sample(&server);
    let value = search(&runtime, &plugin, "мир").unwrap();
    let hit: SearchHit = parse_search_page(&value.to_string(), &catalog_host)
        .unwrap()
        .page
        .hits
        .remove(0);

    let details = call(
        &runtime,
        &plugin,
        PluginFunction::Details,
        &serde_json::to_value(&hit).unwrap(),
    )
    .unwrap();
    let merged = merge_details(&hit, &details.to_string(), &catalog_host).unwrap();
    assert_eq!(
        merged.description.as_deref(),
        Some("From the feed \u{201c}Books\u{201d}.")
    );
    assert_eq!(merged.title, hit.title);

    let offer = &hit.offers[0];
    let resolved = call(
        &runtime,
        &plugin,
        PluginFunction::ResolveDownload,
        &serde_json::to_value(offer).unwrap(),
    )
    .unwrap();
    let spec = parse_download_spec(&resolved.to_string(), offer, &catalog_host).unwrap();
    assert_eq!(spec.archive, Some(Archive::Zip));
    // The link leads, through two redirects, to the book.
    let book = server.session().fetch(&spec.url, &[]).unwrap();
    assert_eq!(book.body, opds::book_zip(&opds::BOOKS[1]));
}

#[test]
fn the_sample_plugin_tries_again_after_a_502_within_the_request_limit() {
    let failures_left = Arc::new(AtomicUsize::new(2));
    let counter = Arc::clone(&failures_left);
    let server = TestServer::start(move |request, context| {
        let menu = request.path == "/opds/search" && request.query_param("searchType").is_none();
        if menu && counter.load(Ordering::SeqCst) > 0 {
            counter.fetch_sub(1, Ordering::SeqCst);
            return Reply::html(502, "<html>502 Bad Gateway</html>");
        }
        opds::handle(request, context)
    });
    let (runtime, logs) = runtime(&server);
    let value = search(&runtime, &sample(&server), "мир").unwrap();
    assert_eq!(titles(&value), ["Война и мир"]);
    // Two failures and the success for the menu, then the results.
    assert_eq!(server.requests().len(), 4);
    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), 2);
    assert!(logs.iter().all(|l| l.level == LogLevel::Warn));
}

#[test]
fn the_sample_plugin_gives_up_after_three_tries() {
    let server = TestServer::start(|_, _| Reply::html(502, "<html>502</html>"));
    let (runtime, _) = runtime(&server);
    let error = search(&runtime, &sample(&server), "мир").unwrap_err();
    assert_eq!(error, "Error: The catalog answered with status 502");
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn the_sample_plugin_turns_a_connection_failure_into_a_message_about_the_settings() {
    let server = idle_server();
    let (runtime, _) = runtime(&server);
    let plugin = fixture("sample-opds", &[("base_url", "http://nxdomain.test/")]);
    let error = search(&runtime, &plugin, "мир").unwrap_err();
    assert_eq!(
        error,
        "Error: Cannot connect to the catalog. Check its address in the plugin settings."
    );
}

#[test]
fn the_sample_plugin_finds_nothing_cleanly() {
    let server = TestServer::start(opds::handle);
    let (runtime, _) = runtime(&server);
    let value = search(&runtime, &sample(&server), "zzzzzz").unwrap();
    let page = parse_search_page(&value.to_string(), &catalog_host).unwrap();
    assert!(page.page.hits.is_empty());
    assert_eq!(page.page.next_cursor, None);
}

#[test]
fn the_sample_plugin_without_a_stored_address_cannot_reach_the_network() {
    let server = TestServer::start(opds::handle);
    let (runtime, _) = runtime(&server);
    let mut plugin = fixture("sample-opds", &[]);
    plugin.manifest.settings[0].default = Some(SettingValue::Text(server.url("/")));
    let error = search(&runtime, &plugin, "мир").unwrap_err();
    assert!(error.contains("blocked_domain"), "{error}");
    assert!(server.requests().is_empty());
}
