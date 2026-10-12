mod app;
mod views;

use std::{io::stdout, sync::mpsc, time::Duration};

use crossterm::{
    ExecutableCommand,
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use rulibre_core::config;
use rulibre_core::device_watch::DeviceWatcher;

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--config") {
        return open_config_in_editor();
    }
    run()
}

fn run() -> std::io::Result<()> {
    let cfg = config::Config::load().map_err(std::io::Error::other)?;
    let mut app = app::App::new(cfg).map_err(std::io::Error::other)?;

    let (device_sender, device_receiver) = mpsc::channel();
    let mut watcher = DeviceWatcher::start(Duration::from_secs(2), move |event| {
        device_sender.send(event).is_ok()
    })?;
    // used to have complete control of terminal text
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(EnableMouseCapture)?;
    let mut terminal = ratatui::init();

    let result = app.run(&mut terminal, device_receiver);
    let stopped = watcher.stop();

    ratatui::restore();
    stdout().execute(DisableMouseCapture)?;
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    stopped.map_err(|_| std::io::Error::other("device watcher panicked"))?;
    result
}

fn open_config_in_editor() -> std::io::Result<()> {
    let config_path = config::Config::path().map_err(std::io::Error::other)?;
    std::fs::create_dir_all(config_path.parent().unwrap())?;
    if !config_path.exists() {
        let default = config::Config {
            library_path: std::path::PathBuf::new(),
        };
        default.save().map_err(std::io::Error::other)?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()?;
    std::process::exit(status.code().unwrap_or(0));
}
