use rulibre_ui::{
    app,
    services::{RealServices, Services},
};
mod screens;

fn main() -> iced::Result {
    rulibre_ui::blur::prepare_environment();
    env_logger::init();
    let services = std::sync::Arc::new(RealServices::new().expect("initialize UI services"));
    let update_services = services.clone();
    let boot_services = services.clone();
    let subscription_services = services.clone();

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
            app::update(state, message, update_services.as_ref())
        },
        app::view,
    )
    .title("Rulibre")
    .window(iced::window::Settings {
        size: iced::Size::new(1100.0, 700.0),
        min_size: Some(iced::Size::new(760.0, 520.0)),
        // With a transparent window the toolbar doubles as the title bar.
        decorations: !rulibre_ui::blur::ready(),
        transparent: rulibre_ui::blur::ready(),
        #[cfg(target_os = "windows")]
        platform_specific: iced::window::settings::PlatformSpecific {
            undecorated_shadow: rulibre_ui::blur::ready(),
            ..Default::default()
        },
        exit_on_close_request: false,
        ..Default::default()
    })
    .theme(rulibre_ui::theme::dark())
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
