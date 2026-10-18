//! `ferrolibri-plugin-check`: the check an author runs on a plugin folder before a release.
//!
//! ```text
//! ferrolibri-plugin-check <plugin dir> [--query TEXT] [--page N] [--cursor TEXT]
//!                         [--setting key=value]... [--raw]
//! ```
//!
//! It reads `plugin.toml` and the script the way the app does when it installs a plugin and
//! reports what the user would be shown (compatibility, permissions, size). With `--query` it
//! also runs `search` in the same sandbox as the app, over the real network with the plugin's
//! real allow-list, validates the answer against the contract (`parse_search_page`) and prints
//! the results, the warnings (dropped results, the plugin's own log), the number of requests
//! and the time the call took. `--setting key=value` stores a setting as the settings form
//! would, so an address stored this way is allowed (D4) and a `default` of the manifest is not.
//!
//! Exit code: 0 when nothing is wrong (warnings do not count), 1 for a problem, 2 for a wrong
//! command line. Only `search` is run: `details` and `resolveDownload` are called by the app on
//! demand and can be tried with `cargo run -p ferrolibri-plugins --example run_plugin`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    time::Instant,
};

use ferrolibri_core::converter::CancellationToken;
use ferrolibri_plugins::{
    Compatibility, Http, LoadedPlugin, LogLevel, LogRecord, PluginFunction, PluginRuntime,
    PluginStore, SettingKind, SettingValue, app_version, limits, parse_search_page,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: ferrolibri-plugin-check <plugin dir> [--query TEXT] [--page N] \
[--cursor TEXT] [--setting key=value]... [--raw]";

/// How many results are listed; the count of the rest is printed.
const LISTED: usize = 10;

#[derive(Debug, PartialEq, Eq)]
struct Args {
    dir: PathBuf,
    query: Option<String>,
    page: u32,
    cursor: Option<String>,
    settings: Vec<(String, String)>,
    raw: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut dir = None;
    let mut parsed = Args {
        dir: PathBuf::new(),
        query: None,
        page: 1,
        cursor: None,
        settings: Vec::new(),
        raw: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--query" => parsed.query = Some(value("--query")?),
            "--page" => {
                parsed.page = value("--page")?
                    .parse()
                    .ok()
                    .filter(|page| *page >= 1)
                    .ok_or("--page needs a number from 1")?;
            }
            "--cursor" => parsed.cursor = Some(value("--cursor")?),
            "--setting" => {
                let pair = value("--setting")?;
                let (key, text) = pair.split_once('=').ok_or("--setting needs key=value")?;
                parsed.settings.push((key.to_owned(), text.to_owned()));
            }
            "--raw" => parsed.raw = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ if dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    parsed.dir = dir.ok_or("the plugin folder is missing")?;
    Ok(parsed)
}

/// What the check found; the exit code follows from it.
#[derive(Default)]
struct Verdict {
    problems: usize,
    warnings: usize,
}

impl Verdict {
    fn problem(&mut self, message: impl std::fmt::Display) {
        self.problems += 1;
        println!("  PROBLEM: {message}");
    }

    fn warning(&mut self, message: impl std::fmt::Display) {
        self.warnings += 1;
        println!("  warning: {message}");
    }
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let verdict = run(&args, &Http::new());
    println!();
    match (verdict.problems, verdict.warnings) {
        (0, 0) => println!("Result: OK"),
        (0, warnings) => println!("Result: OK, {warnings} warning(s)"),
        (problems, _) => println!("Result: FAILED, {problems} problem(s)"),
    }
    if verdict.problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run(args: &Args, http: &Http) -> Verdict {
    let mut verdict = Verdict::default();
    let Some(plugin) = check_files(&args.dir, args, &mut verdict) else {
        return verdict;
    };
    let Some(query) = &args.query else {
        println!("\nSearch: skipped (give --query TEXT to run it)");
        return verdict;
    };
    check_search(&plugin, query, args, http, &mut verdict);
    verdict
}

/// The manifest, the compatibility, the permissions and the script: what installing would
/// look at. `Some` only for a plugin that can be run.
fn check_files(dir: &Path, args: &Args, verdict: &mut Verdict) -> Option<LoadedPlugin> {
    // The store is only asked to inspect; it never touches these paths.
    let scratch =
        std::env::temp_dir().join(format!("ferrolibri-plugin-check-{}", std::process::id()));
    let store = PluginStore::new(scratch.join("plugins"), scratch.join("plugins.toml"));
    let inspection = match store.inspect_local(dir) {
        Ok(inspection) => inspection,
        Err(error) => {
            println!("Plugin folder: {}", dir.display());
            verdict.problem(error);
            return None;
        }
    };
    let manifest = &inspection.manifest;
    println!(
        "{} ({} {}), plugin API {}",
        manifest.name, manifest.id, manifest.version, manifest.api_version
    );
    println!(
        "  size: {} bytes (plugin.toml and {})",
        inspection.size, manifest.entry
    );

    match &inspection.compatibility {
        Compatibility::Compatible => {
            println!("  compatible with FerroLibri {}: yes", app_version());
        }
        Compatibility::Incompatible(reasons) => {
            println!("  compatible with FerroLibri {}: NO", app_version());
            for reason in reasons {
                verdict.problem(reason);
            }
            return None;
        }
    }

    let permissions = &inspection.permissions;
    if permissions.domains.is_empty() {
        println!("  network domains: none");
    } else {
        println!("  network domains: {}", permissions.domains.join(", "));
    }
    for label in &permissions.network_settings {
        println!("  reaches the address the user enters in: {label}");
    }
    println!(
        "  time per call: {} s{}",
        permissions.call_timeout_s,
        if permissions.call_timeout_s > limits::DEFAULT_CALL_TIMEOUT_S {
            " (more than the usual 30 s: the user is told when installing)"
        } else {
            ""
        }
    );
    if permissions.domains.is_empty() && permissions.network_settings.is_empty() {
        verdict.warning("the plugin has no network.domains and no url setting with network = true: it cannot reach any site");
    }
    for setting in &manifest.settings {
        let default = match &setting.default {
            Some(SettingValue::Text(text)) => format!(" = {text}"),
            Some(SettingValue::Bool(flag)) => format!(" = {flag}"),
            None => String::new(),
        };
        println!("  setting {} ({:?}){default}", setting.key, setting.kind);
    }

    let script_path = dir.join(&manifest.entry);
    let source = match std::fs::read_to_string(&script_path) {
        Ok(source) => source,
        Err(error) => {
            verdict.problem(format!("{}: {error}", manifest.entry));
            return None;
        }
    };
    let sha256 = hex(&Sha256::digest(source.as_bytes()));
    match &manifest.sha256 {
        Some(declared) if declared.eq_ignore_ascii_case(&sha256) => {
            println!("  sha256 of {}: matches plugin.toml", manifest.entry);
        }
        Some(declared) => verdict.problem(format!(
            "sha256 of {} is {sha256}, plugin.toml says {declared}",
            manifest.entry
        )),
        None => println!(
            "  sha256 of {}: {sha256} (plugin.toml does not declare one)",
            manifest.entry
        ),
    }

    let settings = stored_settings(manifest, &args.settings, verdict)?;
    Some(LoadedPlugin {
        manifest: manifest.clone(),
        source,
        settings,
    })
}

/// `--setting key=value` as the settings form would store it.
fn stored_settings(
    manifest: &ferrolibri_plugins::Manifest,
    given: &[(String, String)],
    verdict: &mut Verdict,
) -> Option<BTreeMap<String, SettingValue>> {
    let mut stored = BTreeMap::new();
    for (key, text) in given {
        let Some(setting) = manifest.settings.iter().find(|s| &s.key == key) else {
            verdict.problem(format!(
                "--setting {key}: plugin.toml declares no such setting"
            ));
            return None;
        };
        let value = match setting.kind {
            SettingKind::Bool => match text.as_str() {
                "true" => SettingValue::Bool(true),
                "false" => SettingValue::Bool(false),
                _ => {
                    verdict.problem(format!("--setting {key}: use true or false"));
                    return None;
                }
            },
            SettingKind::Choice if !setting.options.contains(text) => {
                verdict.problem(format!(
                    "--setting {key}: one of {} is expected",
                    setting.options.join(", ")
                ));
                return None;
            }
            _ => SettingValue::Text(text.clone()),
        };
        stored.insert(key.clone(), value);
    }
    Some(stored)
}

fn check_search(
    plugin: &LoadedPlugin,
    query: &str,
    args: &Args,
    http: &Http,
    verdict: &mut Verdict,
) {
    let allow = match plugin.allow_list() {
        Ok(allow) => allow,
        Err(error) => {
            verdict.problem(format!("the allow-list cannot be built: {error}"));
            return;
        }
    };
    let mut question = json!({ "text": query, "page": args.page });
    if let Some(cursor) = &args.cursor {
        question["cursor"] = json!(cursor);
    }
    println!("\nSearch {query:?}, page {}:", args.page);

    let log = Arc::new(Mutex::new(Vec::<LogRecord>::new()));
    let sink = Arc::clone(&log);
    let runtime = PluginRuntime::new(http.clone()).with_log_sink(Arc::new(move |record| {
        if let Ok(mut records) = sink.lock() {
            records.push(record);
        }
    }));
    let requests_before = http.requests_started();
    let started = Instant::now();
    let answer = runtime.call(
        plugin,
        PluginFunction::Search,
        &question,
        &CancellationToken::default(),
    );
    let elapsed = started.elapsed();
    let requests = http.requests_started() - requests_before;

    let records = log
        .lock()
        .map(|records| records.clone())
        .unwrap_or_default();
    for record in &records {
        println!("  log {}: {}", level_name(record.level), record.message);
        if matches!(record.level, LogLevel::Warn | LogLevel::Error) {
            verdict.warnings += 1;
        }
    }

    match answer {
        Err(error) => verdict.problem(format!("search failed: {error}")),
        Ok(answer) => report_answer(plugin, &answer, &allow, args.raw, verdict),
    }

    println!(
        "  cost: {requests} request(s) of {} allowed per call, {:.2} s of {} s allowed",
        limits::MAX_FETCH_REQUESTS_PER_CALL,
        elapsed.as_secs_f64(),
        plugin.manifest.call_timeout().as_secs()
    );
    if requests * 4 > u64::from(limits::MAX_FETCH_REQUESTS_PER_CALL) * 3 {
        verdict.warning("more than three quarters of the request allowance is used");
    }
    if elapsed > plugin.manifest.call_timeout().mul_f64(0.66) {
        verdict.warning("the call took more than two thirds of the time allowed");
    }
}

fn report_answer(
    plugin: &LoadedPlugin,
    answer: &Value,
    allow: &ferrolibri_plugins::AllowList,
    raw: bool,
    verdict: &mut Verdict,
) {
    if raw {
        println!(
            "{}",
            serde_json::to_string_pretty(answer).unwrap_or_else(|_| answer.to_string())
        );
    }
    // The app drops a result whose cover is on a host the plugin may not reach: so does the check.
    let parsed = match parse_search_page(&answer.to_string(), &|host| allow.allows_host(host)) {
        Ok(parsed) => parsed,
        Err(error) => {
            verdict.problem(format!("the answer is rejected as a whole: {error}"));
            return;
        }
    };
    let hits = &parsed.page.hits;
    println!(
        "  {} result(s) accepted, {} dropped, next page: {}",
        hits.len(),
        parsed.warnings.len(),
        if parsed.page.next_cursor.is_some() {
            "yes"
        } else {
            "no"
        }
    );
    for (number, hit) in hits.iter().take(LISTED).enumerate() {
        let authors = if hit.authors.is_empty() {
            "(no author)".to_owned()
        } else {
            hit.authors.join(", ")
        };
        let year = hit
            .year
            .map(|year| format!(" ({year})"))
            .unwrap_or_default();
        let formats: Vec<_> = hit
            .offers
            .iter()
            .map(|offer| offer.format.label())
            .collect();
        let offers = if formats.is_empty() {
            String::new()
        } else {
            format!(" [{}]", formats.join(", "))
        };
        let cover = if hit.cover_url.is_some() {
            " +cover"
        } else {
            ""
        };
        println!(
            "  {:>2}. {} - {authors}{year}{offers}{cover}",
            number + 1,
            shorten(&hit.title, 60)
        );
    }
    if hits.len() > LISTED {
        println!("      ... and {} more", hits.len() - LISTED);
    }
    for dropped in &parsed.warnings {
        verdict.warning(dropped);
    }
    if hits.is_empty() && parsed.warnings.is_empty() {
        verdict.warning(format!(
            "no results: check that the query matches something in {}",
            plugin.manifest.name
        ));
    }
}

fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let kept: String = text.chars().take(max - 3).collect();
        format!("{kept}...")
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn a_folder_alone_is_enough_and_options_come_in_any_order() {
        let args = parse(&["plugin"]).unwrap();
        assert_eq!(args.dir, PathBuf::from("plugin"));
        assert_eq!((args.query, args.page, args.raw), (None, 1, false));

        let args = parse(&[
            "--query",
            "war",
            "plugin",
            "--page",
            "2",
            "--cursor",
            "abc",
            "--setting",
            "a=b=c",
            "--raw",
        ])
        .unwrap();
        assert_eq!(args.query.as_deref(), Some("war"));
        assert_eq!(
            (args.page, args.cursor.as_deref(), args.raw),
            (2, Some("abc"), true)
        );
        assert_eq!(args.settings, [("a".to_owned(), "b=c".to_owned())]);
    }

    #[test]
    fn wrong_command_lines_are_named() {
        assert!(parse(&[]).unwrap_err().contains("folder"));
        assert!(parse(&["a", "b"]).unwrap_err().contains("unexpected"));
        assert!(parse(&["a", "--page", "0"]).unwrap_err().contains("--page"));
        assert!(parse(&["a", "--page", "x"]).unwrap_err().contains("--page"));
        assert!(
            parse(&["a", "--query"])
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            parse(&["a", "--setting", "novalue"])
                .unwrap_err()
                .contains("key=value")
        );
        assert!(
            parse(&["a", "--nope"])
                .unwrap_err()
                .contains("unknown option")
        );
    }

    #[test]
    fn long_titles_are_shortened_by_characters() {
        assert_eq!(shorten("short", 10), "short");
        assert_eq!(shorten("Война и мир: том первый", 10), "Война и...");
    }
}
