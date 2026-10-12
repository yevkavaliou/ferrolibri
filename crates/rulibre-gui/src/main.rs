#![cfg_attr(
    all(not(debug_assertions), not(target_arch = "wasm32")),
    windows_subsystem = "windows"
)]

#[cfg(not(target_arch = "wasm32"))]
mod commands;
#[cfg(not(target_arch = "wasm32"))]
mod device_watch;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod components;
#[cfg(target_arch = "wasm32")]
mod ipc;
#[cfg(not(target_arch = "wasm32"))]
mod job_service;
#[cfg(target_arch = "wasm32")]
mod keys;
mod response_guard;
#[cfg(target_arch = "wasm32")]
mod state;
mod types;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct DeviceWatchState(
    pub std::sync::Arc<std::sync::Mutex<Option<rulibre_core::device::Device>>>,
);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct DeviceWatcherState(
    pub std::sync::Mutex<Option<rulibre_core::device_watch::DeviceWatcher>>,
);

#[cfg(target_arch = "wasm32")]
fn main() {
    #[cfg(not(feature = "ipc-smoke"))]
    {
        console_error_panic_hook::set_once();
        leptos::mount::mount_to_body(app::App);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--config") {
        return open_config_in_editor();
    }
    run();
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        .manage(DeviceWatchState::default())
        .manage(job_service::JobService::new().expect("create job workers"))
        .manage(DeviceWatcherState::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let shared: tauri::State<DeviceWatchState> = app.state();
            let shared = shared.0.clone();
            let watcher = device_watch::start(handle, shared)?;
            *app.state::<DeviceWatcherState>()
                .0
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))? = Some(watcher);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_config,
            commands::save_config,
            commands::validate_library,
            commands::scan_library,
            commands::parse_metadata,
            commands::available_backends,
            commands::target_formats,
            commands::convert_book,
            commands::send_to_device,
            commands::current_device,
            commands::poll_job,
            commands::cancel_job,
        ])
        .build(tauri::generate_context!())
        .expect("error while building rulibre GUI")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Ok(mut watcher) = app.state::<DeviceWatcherState>().0.lock() {
                    watcher.take();
                }
                app.state::<job_service::JobService>().shutdown();
            }
        });
}

#[cfg(not(target_arch = "wasm32"))]
fn open_config_in_editor() -> std::io::Result<()> {
    use rulibre_core::config::Config;
    let config_path = Config::path().map_err(std::io::Error::other)?;
    std::fs::create_dir_all(config_path.parent().unwrap())?;
    if !config_path.exists() {
        Config {
            library_path: std::path::PathBuf::new(),
        }
        .save()
        .map_err(std::io::Error::other)?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()?;
    std::process::exit(status.code().unwrap_or(0));
}
