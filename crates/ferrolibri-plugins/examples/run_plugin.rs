//! Manual run of a plugin in the sandbox: what a plugin author, or a tester on a
//! new platform, does to see that a plugin answers.
//!
//! ```text
//! cargo run -p ferrolibri-plugins --example run_plugin -- <plugin dir> search "war and peace"
//! cargo run -p ferrolibri-plugins --example run_plugin -- <plugin dir> search "war" --setting base_url=https://example.org/
//! cargo run -p ferrolibri-plugins --example run_plugin -- <plugin dir> details '{"id":"1","title":"T"}'
//! cargo run -p ferrolibri-plugins --example run_plugin -- crates/ferrolibri-plugins/tests/fixtures/sample-opds search мир --catalog
//! ```
//!
//! `<plugin dir>` holds `plugin.toml` and the script. The third argument is the search text for
//! `search`, or the JSON argument (a result, an offer) for `details` and `resolveDownload`.
//! `--setting key=value` stores a setting as the settings form would (an address stored this way
//! is allowed). `--catalog` starts the local stand-in for an OPDS catalog and stores its
//! address as `base_url`. The answer is printed as JSON; for `search` it is also checked against
//! the contract and the verdict goes to stderr together with the plugin's log.

use std::{collections::BTreeMap, path::PathBuf, process::ExitCode, sync::Arc};

use ferrolibri_core::converter::CancellationToken;
use ferrolibri_plugins::{
    Http, LoadedPlugin, Manifest, PluginFunction, PluginRuntime, SettingValue, app_version,
    parse_search_page,
    testing::{TestServer, opds},
};
use serde_json::{Value, json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut positional = Vec::new();
    let mut settings = BTreeMap::new();
    let mut catalog = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--setting" => {
                let pair = args.next().ok_or("--setting needs key=value")?;
                let (key, value) = pair.split_once('=').ok_or("--setting needs key=value")?;
                settings.insert(key.to_owned(), SettingValue::Text(value.to_owned()));
            }
            "--catalog" => catalog = true,
            _ => positional.push(arg),
        }
    }
    let [dir, function, argument] = positional.as_slice() else {
        return Err(
            "usage: run_plugin <plugin dir> <search|details|resolveDownload> <text or JSON> \
             [--setting key=value]... [--catalog]"
                .into(),
        );
    };

    let dir = PathBuf::from(dir);
    let manifest = Manifest::parse(
        &std::fs::read_to_string(dir.join("plugin.toml"))
            .map_err(|e| format!("plugin.toml: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    let compatibility = manifest.compatibility(&app_version());
    if !compatibility.is_compatible() {
        let reasons: Vec<_> = compatibility
            .reasons()
            .iter()
            .map(|r| r.to_string())
            .collect();
        return Err(format!("not compatible: {}", reasons.join("; ")));
    }
    let source = std::fs::read_to_string(dir.join(&manifest.entry))
        .map_err(|e| format!("{}: {e}", manifest.entry))?;

    // Kept alive until the call is over.
    let server = catalog.then(|| TestServer::start(opds::handle));
    let http = match &server {
        Some(server) => {
            settings.insert("base_url".into(), SettingValue::Text(server.url("/")));
            server.http()
        }
        None => Http::new(),
    };

    let (function, argument): (PluginFunction, Value) = match function.as_str() {
        "search" => (
            PluginFunction::Search,
            json!({ "text": argument, "page": 1 }),
        ),
        "details" => (
            PluginFunction::Details,
            serde_json::from_str(argument).map_err(|e| format!("the argument: {e}"))?,
        ),
        "resolveDownload" => (
            PluginFunction::ResolveDownload,
            serde_json::from_str(argument).map_err(|e| format!("the argument: {e}"))?,
        ),
        other => return Err(format!("unknown function {other}")),
    };

    let runtime = PluginRuntime::new(http).with_log_sink(Arc::new(|record| {
        eprintln!("[{} {:?}] {}", record.plugin, record.level, record.message);
    }));
    let plugin = LoadedPlugin {
        manifest,
        source,
        settings,
    };
    let answer = runtime
        .call(&plugin, function, &argument, &CancellationToken::default())
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&answer).map_err(|e| e.to_string())?
    );

    if function == PluginFunction::Search {
        // The domains of `cover_url` are the app's business (it knows the allow-list); not checked.
        match parse_search_page(&answer.to_string(), &|_| true) {
            Ok(parsed) => {
                eprintln!(
                    "contract: {} result(s) accepted, {} dropped, next page: {}",
                    parsed.page.hits.len(),
                    parsed.warnings.len(),
                    parsed.page.next_cursor.is_some()
                );
                for warning in &parsed.warnings {
                    eprintln!("  dropped: {warning}");
                }
            }
            Err(e) => eprintln!("contract: the answer is rejected: {e}"),
        }
    }
    Ok(())
}
