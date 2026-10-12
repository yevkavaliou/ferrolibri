//! Real iced shell captures with isolated UI preferences. No library or reader IO.
//! cargo run -p rulibre-ui --example shell_probe --locked
use iced::{Element, Task};
use rulibre_ui::{
    app::{self, Filter, Message, Screen, State},
    services::{RealServices, Services},
    theme,
};
use std::{sync::Arc, time::Duration};

struct Probe {
    app: State,
    stage: usize,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Capture,
    Shot(Box<iced::window::Screenshot>),
    Next,
}
const NAMES: [&str; 8] = [
    "home-1100",
    "home-760",
    "sidebar-hidden-760",
    "panes-hidden-760",
    "search-760",
    "jobs-760",
    "settings-760",
    "setup-760",
];
fn delayed_capture() -> Task<Event> {
    Task::perform(
        async { tokio::time::sleep(Duration::from_millis(700)).await },
        |_| Event::Capture,
    )
}
fn view(probe: &Probe) -> Element<'_, Event> {
    app::view(&probe.app).map(|m| Event::App(Box::new(m)))
}
fn main() -> iced::Result {
    std::fs::create_dir_all("docs/ui-item-6").unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let settings_path = fixture.path().join("ui.toml");
    let services = Arc::new(
        RealServices::with_paths(fixture.path().join("config.toml"), settings_path.clone())
            .unwrap(),
    );
    let update_services = services.clone();
    let result = iced::application(
        || {
            let mut state = State::default();
            state.screen = Screen::Home {
                filter: Filter::All,
            };
            (
                Probe {
                    app: state,
                    stage: 0,
                },
                delayed_capture(),
            )
        },
        move |probe: &mut Probe, event| match event {
            Event::App(m) => app::update(&mut probe.app, *m, update_services.as_ref())
                .map(|m| Event::App(Box::new(m))),
            Event::Capture => iced::window::latest().then(|id| {
                iced::window::screenshot(id.unwrap()).map(|shot| Event::Shot(Box::new(shot)))
            }),
            Event::Shot(shot) => {
                let path = format!("docs/ui-item-6/{}.png", NAMES[probe.stage]);
                image::save_buffer(
                    &path,
                    &shot.rgba,
                    shot.size.width,
                    shot.size.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                println!(
                    "{path} {}x{} scale={}",
                    shot.size.width, shot.size.height, shot.scale_factor
                );
                Task::done(Event::Next)
            }
            Event::Next => {
                probe.stage += 1;
                let mut tasks = vec![delayed_capture()];
                let message = match probe.stage {
                    1 => {
                        tasks.push(iced::window::latest().then(|id| {
                            iced::window::resize(id.unwrap(), iced::Size::new(760.0, 520.0))
                        }));
                        None
                    }
                    2 => Some(Message::ToggleSidebar),
                    3 => Some(Message::ToggleDetails),
                    4 => {
                        tasks.push(
                            app::update(
                                &mut probe.app,
                                Message::ToggleSidebar,
                                update_services.as_ref(),
                            )
                            .map(|m| Event::App(Box::new(m))),
                        );
                        Some(Message::Navigate(Screen::Search))
                    }
                    5 => Some(Message::Navigate(Screen::Jobs)),
                    6 => Some(Message::Navigate(Screen::Settings)),
                    7 => Some(Message::Navigate(Screen::Setup)),
                    _ => {
                        let persisted =
                            rulibre_ui::settings::UiSettings::load_from(&settings_path).unwrap();
                        assert!(persisted.sidebar_visible && !persisted.details_visible);
                        println!(
                            "toggles persisted to isolated ui.toml; no library or reader involved"
                        );
                        return iced::window::latest().then(|id| iced::window::close(id.unwrap()));
                    }
                };
                if let Some(message) = message {
                    tasks.push(
                        app::update(&mut probe.app, message, update_services.as_ref())
                            .map(|m| Event::App(Box::new(m))),
                    );
                }
                Task::batch(tasks)
            }
        },
        view,
    )
    .title("Rulibre · shell probe")
    .theme(theme::dark())
    .window(iced::window::Settings {
        size: iced::Size::new(1100.0, 700.0),
        min_size: Some(iced::Size::new(760.0, 520.0)),
        ..Default::default()
    })
    .run();
    services.shutdown();
    result
}
