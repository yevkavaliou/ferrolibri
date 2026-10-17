//! The plugin runtime: runs a plugin's JavaScript in a sandbox and returns what it answered.
//!
//! One call is one thread with its own QuickJS runtime (D5), limited in memory, stack and time
//! (D6). The plugin sees nothing but the language and the `host` object (D3); `eval` from strings
//! is disabled (see `worker`). What a call returns is untrusted JSON that the caller validates
//! with [`crate::parse_search_page`], [`crate::SearchHit::with_details`] or
//! [`crate::parse_download_spec`].

mod host;
mod html;
mod worker;
mod xml;

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
    time::{Duration, Instant},
};

use ferrolibri_core::{Error, ErrorKind, converter::CancellationToken};
use serde_json::Value;

use crate::{AllowList, AllowListError, Http, Manifest, SettingValue, limits, plugin_error};
use worker::{Failure, Job};

/// A plugin ready to run: its manifest, its script and what the user stored in its settings.
#[derive(Clone, Debug)]
pub struct LoadedPlugin {
    pub manifest: Manifest,
    /// The text of the file named by `manifest.entry`.
    pub source: String,
    /// The values stored from the plugin's settings form, by `key`. Only `url` settings with
    /// `network = true` open the network, and only what is stored here counts: a `default` of the
    /// manifest is passed to the plugin but is not an address the user entered (D4).
    pub settings: BTreeMap<String, SettingValue>,
}

impl LoadedPlugin {
    /// The hosts this plugin's network access reaches (D4): the domains of its manifest plus the
    /// `url` settings with `network = true` that the user filled in. The same list serves the
    /// plugin's own requests and the download of the file it pointed to.
    pub fn allow_list(&self) -> Result<AllowList, AllowListError> {
        let entered = worker::entered_addresses(self);
        AllowList::for_manifest(
            &self.manifest,
            &entered.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }
}

/// The functions a plugin can export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFunction {
    /// `search(query, ctx)`; the argument is a [`crate::SearchQuery`] as JSON.
    Search,
    /// `details(hit, ctx)`, optional: absent means the result is `null` (nothing to add).
    Details,
    /// `resolveDownload(offer, ctx)`, optional: absent is an error.
    ResolveDownload,
}

impl PluginFunction {
    /// The name the plugin exports.
    pub const fn js_name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Details => "details",
            Self::ResolveDownload => "resolveDownload",
        }
    }

    /// `Error::operation` of failures of this call.
    const fn operation(self) -> &'static str {
        match self {
            Self::Search => "plugin search",
            Self::Details => "plugin details",
            Self::ResolveDownload => "plugin resolveDownload",
        }
    }
}

impl fmt::Display for PluginFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.js_name())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// One line of a plugin's log: `host.log`, and what the runtime reports about the plugin
/// (JavaScript stacks, request headers that were not sent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    pub plugin: String,
    pub level: LogLevel,
    pub message: String,
}

/// Where plugin logs go. The default writes to the `log` facade, target `plugin`.
pub type LogSink = Arc<dyn Fn(LogRecord) + Send + Sync>;

fn default_sink() -> LogSink {
    Arc::new(|record| {
        let level = match record.level {
            LogLevel::Debug => log::Level::Debug,
            LogLevel::Info => log::Level::Info,
            LogLevel::Warn => log::Level::Warn,
            LogLevel::Error => log::Level::Error,
        };
        log::log!(target: "plugin", level, "[{}] {}", record.plugin, record.message);
    })
}

/// Runs plugin functions. Cheap to clone; clones share the HTTP client (and so its per-host queues).
#[derive(Clone)]
pub struct PluginRuntime {
    http: Http,
    sink: LogSink,
    max_call_time: Option<Duration>,
}

impl fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("http", &self.http)
            .field("max_call_time", &self.max_call_time)
            .finish_non_exhaustive()
    }
}

impl PluginRuntime {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            sink: default_sink(),
            max_call_time: None,
        }
    }

    /// The HTTP client the plugins use; the app's own downloads must share it, so that the
    /// per-host queues and pauses count every request.
    pub fn http(&self) -> &Http {
        &self.http
    }

    /// Sends plugin logs to `sink` instead of the `log` facade.
    pub fn with_log_sink(mut self, sink: LogSink) -> Self {
        self.sink = sink;
        self
    }

    /// Caps the time of a call below the manifest's `call_timeout_s`. For tests and diagnostics.
    pub fn with_max_call_time(mut self, max: Duration) -> Self {
        self.max_call_time = Some(max);
        self
    }

    /// Calls `function` of `plugin` with `arg` (a query, a result or an offer, as JSON) and
    /// the `ctx` argument the runtime builds from the plugin's settings.
    ///
    /// Blocks the calling thread until the plugin has answered or failed; run it on a worker
    /// thread. Cancelling `cancel` ends the call within about 100 ms (`ErrorKind::Cancelled`).
    /// Every other failure — an exception, a syntax error, time, memory, a result that is not
    /// JSON or is too large — is an `ErrorKind::Plugin` attributed to the plugin; the JavaScript
    /// stack goes to the log, not into the error. The value is untrusted (see the module docs).
    pub fn call(
        &self,
        plugin: &LoadedPlugin,
        function: PluginFunction,
        arg: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value, Error> {
        let operation = function.operation();
        let id = plugin.manifest.id.as_str();
        if cancel.is_cancelled() {
            return Err(cancelled(operation));
        }
        let allowed = plugin.manifest.call_timeout();
        let limit = self.max_call_time.map_or(allowed, |cap| cap.min(allowed));
        let deadline = Instant::now() + limit;
        let abandoned = Arc::new(AtomicBool::new(false));
        let job = Job {
            plugin: plugin.clone(),
            function,
            arg: arg.clone(),
            http: self.http.clone(),
            cancel: cancel.clone(),
            deadline,
            sink: Arc::clone(&self.sink),
            abandoned: Arc::clone(&abandoned),
        };

        let (sender, receiver) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name(format!("plugin-{id}"))
            .stack_size(limits::PLUGIN_THREAD_STACK_BYTES)
            .spawn(move || {
                // The receiver is gone when the caller gave up waiting.
                let _ = sender.send(worker::run(job));
            });
        if let Err(e) = spawned {
            return Err(plugin_error(
                operation,
                id,
                format!("could not start the plugin's thread: {e}"),
            ));
        }

        match receiver.recv_timeout(limit + limits::PLUGIN_UNWIND_GRACE) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(Failure::Cancelled)) => Err(cancelled(operation)),
            Ok(Err(Failure::TimedOut)) => Err(timed_out(operation, id, limit)),
            Ok(Err(Failure::Plugin(message))) => Err(plugin_error(operation, id, message)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The call did not unwind after the deadline (stuck in native code). Stop
                // waiting; the thread ends on its own at its next interrupt check.
                abandoned.store(true, std::sync::atomic::Ordering::Relaxed);
                if cancel.is_cancelled() {
                    Err(cancelled(operation))
                } else {
                    Err(timed_out(operation, id, limit))
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(plugin_error(
                operation,
                id,
                "the plugin runtime stopped unexpectedly",
            )),
        }
    }
}

fn cancelled(operation: &'static str) -> Error {
    Error {
        operation,
        path: None,
        kind: ErrorKind::Cancelled,
    }
}

fn timed_out(operation: &'static str, id: &str, limit: Duration) -> Error {
    plugin_error(
        operation,
        id,
        format!(
            "the call did not finish within {} s",
            limit.as_secs_f64().ceil()
        ),
    )
}
