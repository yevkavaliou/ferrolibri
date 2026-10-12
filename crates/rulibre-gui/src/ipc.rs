use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

use crate::types::{
    Backend, BookFile, Config, ConvertOk, Device, DeviceEvent, JobId, JobReply, JobValue, Metadata,
    ScanReport,
};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "event"], js_name = "listen")]
    async fn listen_event(event: &str, handler: &js_sys::Function) -> Result<JsValue, JsValue>;
}

async fn call_result<R: for<'de> Deserialize<'de>>(
    cmd: &str,
    args: impl Serialize,
) -> Result<R, String> {
    let args = to_value(&args).map_err(|e| e.to_string())?;
    let result = invoke(cmd, args)
        .await
        .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;
    from_value(result).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize)]
struct PathArg {
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BookPathArg {
    book_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryPathArg {
    library_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetFormatsArgs {
    files: Vec<BookFile>,
    backends: Vec<Backend>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConvertArgs {
    files: Vec<BookFile>,
    book_path: String,
    target: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendArgs {
    book_path: String,
    author: String,
}

pub async fn load_config() -> Result<Option<Config>, String> {
    call_result("load_config", Empty {}).await
}

pub async fn save_config(library_path: String) -> Result<(), String> {
    call_result("save_config", LibraryPathArg { library_path }).await
}

pub async fn validate_library(path: String) -> Result<String, String> {
    call_result("validate_library", PathArg { path }).await
}

pub async fn scan_library(path: String) -> Result<ScanReport, String> {
    let id = call_result("scan_library", PathArg { path }).await?;
    match wait_job(id, |_| {}).await? {
        JobValue::Scan(report) => Ok(report),
        _ => Err("Unexpected scan response".into()),
    }
}

pub async fn parse_metadata(book_path: String) -> Result<Option<Metadata>, String> {
    call_result("parse_metadata", BookPathArg { book_path }).await
}

pub async fn available_backends() -> Result<Vec<Backend>, String> {
    call_result("available_backends", Empty {}).await
}

pub async fn target_formats(
    files: Vec<BookFile>,
    backends: Vec<Backend>,
) -> Result<Vec<(String, String)>, String> {
    call_result("target_formats", TargetFormatsArgs { files, backends }).await
}

pub async fn convert_book(
    book_path: String,
    files: Vec<BookFile>,
    target: String,
    on_started: impl FnOnce(JobId),
    on_progress: impl FnMut(crate::types::JobEvent),
) -> Result<ConvertOk, String> {
    let id = call_result(
        "convert_book",
        ConvertArgs {
            book_path,
            files,
            target,
        },
    )
    .await?;
    on_started(id);
    match wait_job(id, on_progress).await? {
        JobValue::Conversion(result) => Ok(result),
        _ => Err("Unexpected conversion response".into()),
    }
}

pub async fn send_to_device(
    book_path: String,
    author: String,
    on_progress: impl FnMut(crate::types::JobEvent),
) -> Result<String, String> {
    let id = call_result("send_to_device", SendArgs { book_path, author }).await?;
    match wait_job(id, on_progress).await? {
        JobValue::Sent(message) => Ok(message),
        _ => Err("Unexpected transfer response".into()),
    }
}

pub async fn current_device() -> Result<Option<Device>, String> {
    call_result("current_device", Empty {}).await
}

#[derive(Deserialize)]
struct EventPayload {
    payload: DeviceEvent,
}

/// Subscribe to `device-event` from the Tauri backend.
/// The closure is leaked into the JS environment for the lifetime of the app —
/// acceptable here since the listener should run as long as the window is open.
pub fn listen_device_events<F>(on_event: F)
where
    F: FnMut(Result<DeviceEvent, String>) + 'static,
{
    let callback = std::rc::Rc::new(std::cell::RefCell::new(on_event));
    let event_callback = callback.clone();
    let closure = Closure::<dyn FnMut(JsValue)>::new(move |js: JsValue| {
        event_callback.borrow_mut()(
            from_value::<EventPayload>(js)
                .map(|event| event.payload)
                .map_err(|e| e.to_string()),
        );
    });
    let function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
    closure.forget();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = listen_event("device-event", &function).await {
            callback.borrow_mut()(Err(error
                .as_string()
                .unwrap_or_else(|| format!("{error:?}"))));
        }
    });
}

#[derive(Serialize)]
struct JobArg {
    id: JobId,
}
async fn wait_job(
    id: JobId,
    mut on_event: impl FnMut(crate::types::JobEvent),
) -> Result<JobValue, String> {
    loop {
        let reply: JobReply = call_result("poll_job", JobArg { id }).await?;
        if reply.id != id {
            return Err("Mismatched job response".into());
        }
        for event in reply.events {
            on_event(event);
        }
        if let Some(result) = reply.result {
            return result;
        }
        gloo_timers::future::TimeoutFuture::new(100).await;
    }
}

pub async fn cancel_job(id: JobId) -> Result<(), String> {
    call_result("cancel_job", JobArg { id }).await
}

// Exports used by scripts/check-wasm-ipc.cjs; absent from production builds.
#[cfg(feature = "ipc-smoke")]
#[wasm_bindgen]
pub async fn smoke_config(cmd: String) -> Result<JsValue, JsValue> {
    let config: Config = call_result(&cmd, Empty {}).await.map_err(JsValue::from)?;
    to_value(&config).map_err(|e| JsValue::from(e.to_string()))
}

#[cfg(feature = "ipc-smoke")]
#[wasm_bindgen]
pub async fn smoke_scan() -> Result<JsValue, JsValue> {
    let report = scan_library("fixture/library".into())
        .await
        .map_err(JsValue::from)?;
    to_value(&report).map_err(|e| JsValue::from(e.to_string()))
}

#[cfg(feature = "ipc-smoke")]
#[wasm_bindgen]
pub async fn smoke_bad_args() -> Result<JsValue, JsValue> {
    struct BadArgs;
    impl Serialize for BadArgs {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("fixture serialization failure"))
        }
    }
    call_result::<Config>("must_not_invoke", BadArgs)
        .await
        .map_err(JsValue::from)?;
    Ok(JsValue::NULL)
}
