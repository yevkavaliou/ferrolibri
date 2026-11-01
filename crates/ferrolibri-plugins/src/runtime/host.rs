//! The `host` global of a plugin: the only door out of the sandbox.

use std::{cell::Cell, rc::Rc};

use rquickjs::{
    Ctx, Exception, Function, Object, Result, Value,
    convert::Coerced,
    function::{Opt, Rest},
};
use serde_json::Value as Json;
use url::Url;

use super::{
    LogLevel, LogRecord, LogSink, {html, xml},
};
use crate::{
    HostError, HttpSession,
    limits::{MAX_LOG_ENTRIES_PER_CALL, MAX_LOG_MESSAGE_CHARS},
};

/// What the `host` functions of one call share. Lives and dies on the call's thread.
pub(super) struct HostState {
    pub plugin_id: String,
    pub session: HttpSession,
    pub sink: LogSink,
    logged: Cell<u32>,
}

impl HostState {
    pub fn new(plugin_id: String, session: HttpSession, sink: LogSink) -> Self {
        Self {
            plugin_id,
            session,
            sink,
            logged: Cell::new(0),
        }
    }

    /// Writes one message to the plugin's log, within the per-call allowance.
    pub fn log(&self, level: LogLevel, message: &str) {
        let count = self.logged.get();
        if count > MAX_LOG_ENTRIES_PER_CALL {
            return;
        }
        self.logged.set(count + 1);
        let message = if count == MAX_LOG_ENTRIES_PER_CALL {
            format!(
                "further messages of this call are dropped (at most {MAX_LOG_ENTRIES_PER_CALL})"
            )
        } else {
            cut(message, MAX_LOG_MESSAGE_CHARS)
        };
        (self.sink)(LogRecord {
            plugin: self.plugin_id.clone(),
            level,
            message,
        });
    }
}

fn cut(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Defines `globalThis.host`.
pub(super) fn install<'js>(ctx: &Ctx<'js>, state: &Rc<HostState>) -> Result<()> {
    let host = Object::new(ctx.clone())?;

    let fetch_state = Rc::clone(state);
    host.set(
        "fetch",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, url: String, options: Opt<Value<'js>>| {
                fetch(&ctx, &fetch_state, &url, options.0)
            },
        )?
        .with_name("fetch")?,
    )?;

    let url = Object::new(ctx.clone())?;
    url.set(
        "resolve",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, base: String, reference: String| resolve(&ctx, &base, &reference),
        )?
        .with_name("resolve")?,
    )?;
    host.set("url", url)?;

    let html_api = Object::new(ctx.clone())?;
    html_api.set(
        "select",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String, selector: String| {
                let found = html::select(&text, &selector).map_err(|e| type_error(&ctx, &e))?;
                to_js(&ctx, &Json::Array(found))
            },
        )?
        .with_name("select")?,
    )?;
    host.set("html", html_api)?;

    let xml_api = Object::new(ctx.clone())?;
    xml_api.set(
        "parse",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
            let tree = xml::parse(&text).map_err(|e| type_error(&ctx, &e))?;
            to_js(&ctx, &tree)
        })?
        .with_name("parse")?,
    )?;
    host.set("xml", xml_api)?;

    let log_state = Rc::clone(state);
    host.set(
        "log",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  level: String,
                  message: Opt<Coerced<String>>,
                  _rest: Rest<Value<'js>>| {
                let Some(level) = LogLevel::parse(&level) else {
                    return Err(type_error(
                        &ctx,
                        "the level must be one of \"debug\", \"info\", \"warn\", \"error\"",
                    ));
                };
                log_state.log(level, &message.0.map(|m| m.0).unwrap_or_default());
                Ok(())
            },
        )?
        .with_name("log")?,
    )?;

    ctx.globals().set("host", host)
}

fn fetch<'js>(
    ctx: &Ctx<'js>,
    state: &HostState,
    url: &str,
    options: Option<Value<'js>>,
) -> Result<Object<'js>> {
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(options) = options.filter(|o| !o.is_undefined() && !o.is_null()) {
        let Some(options) = options.into_object() else {
            return Err(type_error(ctx, "the options must be an object"));
        };
        if let Some(method) = options.get::<_, Option<String>>("method")?
            && !method.eq_ignore_ascii_case("GET")
        {
            return Err(type_error(
                ctx,
                "only GET requests are supported (host.fetch takes no method other than \"GET\")",
            ));
        }
        let body: Value = options.get("body")?;
        if !body.is_undefined() && !body.is_null() {
            return Err(type_error(
                ctx,
                "a request body is not supported (only GET requests are)",
            ));
        }
        if let Some(given) = options.get::<_, Option<Object>>("headers")? {
            for entry in given.props::<String, Coerced<String>>() {
                let (name, value) = entry?;
                headers.push((name, value.0));
            }
        }
    }
    let pairs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let response = match state.session.fetch(url, &pairs) {
        Ok(response) => response,
        Err(error) => return Err(host_error(ctx, &error)),
    };
    for name in &response.head.dropped_request_headers {
        state.log(
            LogLevel::Warn,
            &format!("the request header {name:?} was not sent (host.fetch allows only Accept, Accept-Language, Content-Type, Referer and Range)"),
        );
    }

    let result = Object::new(ctx.clone())?;
    result.set("status", response.head.status)?;
    result.set("url", response.head.url.as_str())?;
    let reply_headers = Object::new(ctx.clone())?;
    for (name, value) in &response.head.headers {
        // A header sent several times reads as one comma-separated value, as in `fetch`.
        let joined = match reply_headers.get::<_, Option<String>>(name.as_str())? {
            Some(earlier) => format!("{earlier}, {value}"),
            None => value.clone(),
        };
        reply_headers.set(name.as_str(), joined)?;
    }
    result.set("headers", reply_headers)?;
    // Binary bodies are not supported in v1: undecodable bytes become U+FFFD.
    result.set("body", String::from_utf8_lossy(&response.body).as_ref())?;
    Ok(result)
}

fn resolve(ctx: &Ctx<'_>, base: &str, reference: &str) -> Result<String> {
    let base = Url::parse(base)
        .map_err(|e| type_error(ctx, &format!("the base is not an absolute URL: {e}")))?;
    base.join(reference)
        .map(String::from)
        .map_err(|e| type_error(ctx, &format!("cannot resolve {reference:?}: {e}")))
}

/// Throws the `host.fetch` failure as an `Error` whose `kind` is the `HostErrorKind` name.
fn host_error(ctx: &Ctx<'_>, error: &HostError) -> rquickjs::Error {
    let thrown = Exception::from_message(ctx.clone(), &error.message).and_then(|exception| {
        exception
            .as_object()
            .set("kind", error.kind.as_str())
            .map(|()| exception)
    });
    match thrown {
        Ok(exception) => exception.throw(),
        Err(e) => e,
    }
}

pub(super) fn type_error(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    Exception::throw_type(ctx, message)
}

fn to_js<'js>(ctx: &Ctx<'js>, value: &Json) -> Result<Value<'js>> {
    let text = serde_json::to_string(value).map_err(|_| rquickjs::Error::Unknown)?;
    ctx.json_parse(text)
}
