mod app;
mod catalog;
mod icons;
mod screens;
mod selection;
mod services;
mod settings;
mod theme;
mod widgets;

fn main() -> iced::Result {
    env_logger::init();

    iced::application(app::State::default, app::update, app::view)
        .title("Rulibre")
        .window(iced::window::Settings {
            size: iced::Size::new(1100.0, 700.0),
            min_size: Some(iced::Size::new(760.0, 520.0)),
            decorations: true,
            ..Default::default()
        })
        .theme(iced::Theme::Dark)
        .run()
}
