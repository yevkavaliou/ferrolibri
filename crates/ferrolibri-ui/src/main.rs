// Release builds on Windows are a GUI-subsystem program: no console window.
// Debug builds keep the console so `RUST_LOG` output stays visible.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use ferrolibri_ui::{
    app,
    services::{RealServices, Services},
};

fn main() -> iced::Result {
    ferrolibri_ui::blur::prepare_environment();
    // Warnings the UI does not show (cover decoding, device polling, metadata, scan, a plugin
    // failing) are logged to a file next to ui.toml; RUST_LOG overrides the default level.
    ferrolibri_ui::logging::init();
    let services = match RealServices::new() {
        Ok(services) => std::sync::Arc::new(services),
        Err(error) => {
            // Without a console (release build on Windows) a panic would vanish.
            log::error!("could not initialize services: {error}");
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("FerroLibri")
                .set_description(format!("FerroLibri could not start: {error}"))
                .show();
            std::process::exit(1);
        }
    };
    let update_services = services.clone();
    let boot_services = services.clone();
    let subscription_services = services.clone();

    let window_path = ferrolibri_ui::settings::WindowSize::path().ok();
    let window_size = window_path
        .as_deref()
        .map(ferrolibri_ui::settings::WindowSize::load_from)
        .unwrap_or_default();

    let result = iced::application(
        move || {
            let mut state = app::State::default();
            let settings = app::update(
                &mut state,
                app::Message::LoadSettings,
                boot_services.as_ref(),
            );
            let library = app::update(
                &mut state,
                app::Message::LoadLibrary,
                boot_services.as_ref(),
            );
            let backends = app::update(
                &mut state,
                app::Message::RefreshBackends,
                boot_services.as_ref(),
            );
            (state, iced::Task::batch([settings, library, backends]))
        },
        move |state: &mut app::State, message| {
            if matches!(message, app::Message::Close(_))
                && let (Some(path), Some(size)) = (
                    window_path.as_deref(),
                    ferrolibri_ui::settings::WindowSize::new(
                        state.window_width,
                        state.window_height,
                    )
                    .filter(|_| state.window_width > 0.0 && state.window_height > 0.0),
                )
                && let Err(error) = size.save_to(path)
            {
                log::warn!("could not save the window size: {error}");
            }
            app::update(state, message, update_services.as_ref())
        },
        app::view,
    )
    .title("FerroLibri")
    .window(iced::window::Settings {
        size: iced::Size::new(window_size.width, window_size.height),
        min_size: Some(iced::Size::new(
            ferrolibri_ui::settings::WINDOW_MIN_SIZE.0,
            ferrolibri_ui::settings::WINDOW_MIN_SIZE.1,
        )),
        // With a transparent window the toolbar doubles as the title bar.
        decorations: !ferrolibri_ui::blur::ready(),
        transparent: ferrolibri_ui::blur::ready(),
        #[cfg(target_os = "windows")]
        platform_specific: iced::window::settings::PlatformSpecific {
            undecorated_shadow: ferrolibri_ui::blur::ready(),
            ..Default::default()
        },
        exit_on_close_request: false,
        ..Default::default()
    })
    .theme(ferrolibri_ui::theme::dark())
    .style(app::style)
    .subscription(move |state: &app::State| {
        iced::Subscription::batch([
            app::subscription(state),
            if state.closing {
                iced::Subscription::none()
            } else {
                subscription_services
                    .device_subscription()
                    .map(app::Message::Devices)
            },
        ])
    })
    .run();
    services.shutdown();
    result
}
