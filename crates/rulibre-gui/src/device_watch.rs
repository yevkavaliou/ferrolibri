use rulibre_core::{device::Device, device_watch::DeviceWatcher};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter};

pub fn start(app: AppHandle, shared: Arc<Mutex<Option<Device>>>) -> std::io::Result<DeviceWatcher> {
    DeviceWatcher::start(Duration::from_secs(2), move |event| {
        let Ok(mut selected) = shared.lock() else {
            return false;
        };
        *selected = event.selected_device().cloned();
        drop(selected);
        if let Err(error) = app.emit("device-event", event) {
            eprintln!("Device event delivery: {error}");
        }
        true
    })
}
