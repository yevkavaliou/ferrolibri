//! What happens on the thread of one plugin call: build the runtime, load the plugin, call it.
//!
//! **No `eval` from strings (decision of item 6, shared.md).** The plugin's source is compiled to
//! bytecode in a throw-away context that has the evaluator, and run in a context that does not:
//! there `eval`, `new Function(…)` and the `AsyncFunction`/`GeneratorFunction` constructors fail
//! with "eval is not supported" and `import()` finds no loader. Nothing a plugin legitimately
//! does needs them, so nothing is lost. The compile context never runs the plugin's code.

use std::{
    cell::OnceCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use ferrolibri_core::converter::CancellationToken;
use rquickjs::{
    CatchResultExt, CaughtError, Context, Ctx, Module, Runtime, Value, context::intrinsic,
    module::WriteOptions,
};
use serde_json::{Value as Json, json};

use super::{
    LoadedPlugin, LogLevel, LogSink, PluginFunction,
    host::{self, HostState},
};
use crate::{
    AllowList, Http, Identity, SettingKind, SettingValue,
    limits::{
        MAX_FETCH_REQUESTS_PER_CALL, MAX_RESPONSE_BYTES, PLUGIN_MEMORY_BYTES, PLUGIN_STACK_BYTES,
    },
};

/// Why a call produced no value.
#[derive(Debug)]
pub(super) enum Failure {
    Cancelled,
    TimedOut,
    /// The plugin, or its answer, broke the contract; the text is for the user.
    Plugin(String),
}

pub(super) struct Job {
    pub plugin: LoadedPlugin,
    pub function: PluginFunction,
    pub arg: Json,
    pub http: Http,
    pub cancel: CancellationToken,
    pub deadline: Instant,
    pub sink: LogSink,
    /// Set by the caller that stopped waiting; the call ends at its next interrupt check.
    pub abandoned: Arc<AtomicBool>,
}

/// The JavaScript globals a plugin has besides the language itself. No `Eval`.
type RunIntrinsics = (
    intrinsic::Date,
    intrinsic::RegExpCompiler,
    intrinsic::RegExp,
    intrinsic::Json,
    intrinsic::Proxy,
    intrinsic::MapSet,
    intrinsic::TypedArrays,
    intrinsic::Promise,
    intrinsic::WeakRef,
);

pub(super) fn run(job: Job) -> Result<Json, Failure> {
    let Job {
        plugin,
        function,
        arg,
        http,
        cancel,
        deadline,
        sink,
        abandoned,
    } = job;

    let entered = entered_addresses(&plugin);
    let allow = AllowList::for_manifest(
        &plugin.manifest,
        &entered.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .map_err(|e| Failure::Plugin(format!("the plugin's network settings are invalid: {e}")))?;
    let session = http
        .session(
            Identity::Plugin(plugin.manifest.id.clone()),
            allow,
            cancel.clone(),
        )
        .with_request_budget(MAX_FETCH_REQUESTS_PER_CALL)
        .with_deadline(deadline);
    let state = Rc::new(HostState::new(
        plugin.manifest.id.clone(),
        session,
        Arc::clone(&sink),
    ));

    // Declared before the runtime so that it is dropped after it: the bytecode is loaded without
    // copying its strings and must outlive everything that has read it.
    let bytecode_slot: OnceCell<Vec<u8>> = OnceCell::new();

    let runtime = Runtime::new().map_err(internal)?;
    runtime.set_memory_limit(PLUGIN_MEMORY_BYTES);
    runtime.set_max_stack_size(PLUGIN_STACK_BYTES);
    let timed_out = Arc::new(AtomicBool::new(false));
    {
        let (cancel, timed_out) = (cancel.clone(), Arc::clone(&timed_out));
        runtime.set_interrupt_handler(Some(Box::new(move || {
            if cancel.is_cancelled() || abandoned.load(Ordering::Relaxed) {
                return true;
            }
            if Instant::now() >= deadline {
                timed_out.store(true, Ordering::Relaxed);
                return true;
            }
            false
        })));
    }
    // An interrupted call fails with a generic exception; this tells why it really ended.
    let outcome = |failure: Failure| {
        if cancel.is_cancelled() {
            Failure::Cancelled
        } else if timed_out.load(Ordering::Relaxed) {
            Failure::TimedOut
        } else {
            failure
        }
    };

    let entry = plugin.manifest.entry.as_str();
    let compiler = Context::full(&runtime).map_err(internal)?;
    let compiled = compiler
        .with(|ctx| {
            Module::declare(ctx.clone(), entry, plugin.source.as_str())
                .and_then(|module| module.write(WriteOptions::default()))
                .catch(&ctx)
                .map_err(|e| format!("{entry} cannot be compiled: {}", describe(&state, e)))
        })
        .map_err(|message| outcome(Failure::Plugin(message)))?;
    drop(compiler);
    let bytecode = bytecode_slot.get_or_init(|| compiled);

    let context = Context::custom::<RunIntrinsics>(&runtime).map_err(internal)?;
    context
        .with(|ctx| call(&ctx, &state, &plugin, function, &arg, bytecode))
        .map_err(|message| outcome(Failure::Plugin(message)))
}

/// Loads the compiled plugin into `ctx` and calls `function`; errors are texts for the user.
fn call<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<HostState>,
    plugin: &LoadedPlugin,
    function: PluginFunction,
    arg: &Json,
    bytecode: &[u8],
) -> Result<Json, String> {
    let entry = plugin.manifest.entry.as_str();
    let name = function.js_name();
    host::install(ctx, state).map_err(|e| format!("the script engine failed: {e}"))?;

    // SAFETY: the bytes are the output of `Module::write` in this very call, unchanged.
    let module = unsafe { Module::load(ctx.clone(), bytecode) }
        .catch(ctx)
        .map_err(|e| format!("{entry} cannot be loaded: {}", describe(state, e)))?;
    let (module, promise) = module
        .eval()
        .catch(ctx)
        .map_err(|e| format!("{entry} failed while loading: {}", describe(state, e)))?;
    promise
        .finish::<Value>()
        .catch(ctx)
        .map_err(|e| format!("{entry} failed while loading: {}", describe(state, e)))?;

    let exported: Value = module
        .get(name)
        .catch(ctx)
        .map_err(|e| describe(state, e))?;
    let Some(exported) = exported.into_function() else {
        return match function {
            // `details` is optional: "nothing to add".
            PluginFunction::Details => Ok(Json::Null),
            PluginFunction::Search | PluginFunction::ResolveDownload => {
                Err(format!("{entry} does not export a function {name}"))
            }
        };
    };

    let arguments = (to_js(ctx, arg)?, to_js(ctx, &context_value(plugin))?);
    let value: Value = exported
        .call(arguments)
        .catch(ctx)
        .map_err(|e| describe(state, e))?;
    to_json(ctx, value, name)
}

/// The addresses the user typed into `url` settings that open the network (D4). A default of the
/// manifest is not an entered address: only what the settings form stored counts.
fn entered_addresses(plugin: &LoadedPlugin) -> Vec<String> {
    plugin
        .manifest
        .settings
        .iter()
        .filter(|s| s.kind == SettingKind::Url && s.network)
        .filter_map(|s| match plugin.settings.get(&s.key) {
            Some(SettingValue::Text(address)) => Some(address.clone()),
            _ => None,
        })
        .collect()
}

/// The `ctx` argument: the settings the plugin declared, the user's value or else the default.
fn context_value(plugin: &LoadedPlugin) -> Json {
    let mut settings = BTreeMap::new();
    for setting in &plugin.manifest.settings {
        let value = plugin
            .settings
            .get(&setting.key)
            .or(setting.default.as_ref());
        if let Some(value) = value {
            settings.insert(setting.key.clone(), json!(value));
        }
    }
    json!({ "settings": settings })
}

fn to_js<'js>(ctx: &Ctx<'js>, value: &Json) -> Result<Value<'js>, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    ctx.json_parse(text).map_err(|e| e.to_string())
}

/// The plugin's return value as JSON, within the response limit.
fn to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>, function: &str) -> Result<Json, String> {
    if value.is_promise() {
        return Err(format!(
            "{function} returned a Promise; plugin functions are synchronous (do not use async or await)"
        ));
    }
    let text = match ctx.json_stringify(value) {
        Ok(Some(text)) => text.to_string().map_err(|e| e.to_string())?,
        // `undefined`, a function or a symbol: nothing to say.
        Ok(None) => return Ok(Json::Null),
        Err(error) => {
            let caught = CaughtError::from_error(ctx, error);
            return Err(format!(
                "the result of {function} cannot be converted to JSON: {}",
                message_of(&caught)
            ));
        }
    };
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(format!(
            "the result of {function} is larger than {MAX_RESPONSE_BYTES} bytes of JSON"
        ));
    }
    serde_json::from_str(&text).map_err(|e| {
        format!("the result of {function} is not usable JSON: {e} (nested too deeply?)")
    })
}

/// The text for the user; the JavaScript stack goes to the plugin's log instead.
fn describe(state: &HostState, caught: CaughtError<'_>) -> String {
    if let CaughtError::Exception(exception) = &caught
        && let Some(stack) = exception.stack().filter(|s| !s.trim().is_empty())
    {
        state.log(
            LogLevel::Error,
            &format!("{}\n{stack}", message_of(&caught)),
        );
    }
    let message = message_of(&caught);
    if message.contains("out of memory") {
        return format!(
            "the plugin used more memory than the {} MiB allowed",
            PLUGIN_MEMORY_BYTES / (1024 * 1024)
        );
    }
    message
}

/// `Name: message`, or `kind: message` for a `host.fetch` failure nobody caught.
fn message_of(caught: &CaughtError<'_>) -> String {
    match caught {
        CaughtError::Exception(exception) => {
            let object = exception.as_object();
            let message = exception.message().unwrap_or_default();
            if let Ok(Some(kind)) = object.get::<_, Option<String>>("kind") {
                return format!("network error ({kind}): {message}");
            }
            match object.get::<_, Option<String>>("name") {
                Ok(Some(name)) if !name.is_empty() => format!("{name}: {message}"),
                _ => message,
            }
        }
        CaughtError::Value(value) => match value.as_string().map(|s| s.to_string()) {
            Some(Ok(text)) => text,
            // The engine reports some out-of-memory failures (inside promise jobs) as `null`.
            _ if value.is_null() || value.is_undefined() => {
                "the plugin threw null or undefined (or ran out of memory)".into()
            }
            _ => "the plugin threw a value that is not an Error".into(),
        },
        CaughtError::Error(error) => error.to_string(),
    }
}

fn internal(error: rquickjs::Error) -> Failure {
    Failure::Plugin(format!("the script engine failed: {error}"))
}
