//! Scripted native Settings captures using isolated config and library fixtures.
//! No personal configuration, physical input or reader is used.
use iced::{Element, Task};
use rulibre_ui::{
    app::{self, Message, Screen, State},
    services::{RealServices, Services},
    theme,
    widgets::toasts,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

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
fn delay() -> Task<Event> {
    Task::perform(
        async { tokio::time::sleep(Duration::from_millis(500)).await },
        |_| Event::Capture,
    )
}
fn view(probe: &Probe) -> Element<'_, Event> {
    app::view(&probe.app).map(|message| Event::App(Box::new(message)))
}
fn main() -> iced::Result {
    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("Calibre Library");
    std::fs::create_dir(&library).unwrap();
    std::fs::write(library.join("metadata.db"), b"fixture marker").unwrap();
    let export = fixture.path().join("Exports/Экспорт książki/Long folder name for testing path wrapping at minimum window width");
    std::fs::create_dir_all(&export).unwrap();
    let services = Arc::new(
        RealServices::with_paths(
            fixture.path().join("config.toml"),
            fixture.path().join("ui.toml"),
        )
        .unwrap(),
    );
    let svc = services.clone();
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-item-12");
    std::fs::create_dir_all(&out).unwrap();
    let names = [
        "settings-default-760",
        "settings-saved-760",
        "settings-error-760",
    ];
    let result = iced::application(
        move || {
            let mut state = State::default();
            state.screen = Screen::Settings;
            state.library = Some(library.clone());
            (Probe { app: state, stage: 0 }, delay())
        },
        move |probe: &mut Probe, event| match event {
            Event::App(message) => app::update(&mut probe.app, *message, svc.as_ref()).map(|message| Event::App(Box::new(message))),
            Event::Capture => {
                if probe.stage > 0 {
                    assert_eq!(probe.app.settings.window_opacity, 30);
                    assert!(!probe.app.settings.prefer_kepub);
                    assert_eq!(probe.app.settings.export_folder, Some(export.clone()));
                }
                iced::window::latest().then(|id| iced::window::screenshot(id.unwrap()).map(|shot| Event::Shot(Box::new(shot))))
            }
            Event::Shot(shot) => {
                let path = out.join(format!("{}.png", names[probe.stage]));
                image::save_buffer(&path, &shot.rgba, shot.size.width, shot.size.height, image::ColorType::Rgba8).unwrap();
                println!("{} {}x{}", path.display(), shot.size.width, shot.size.height);
                Task::done(Event::Next)
            }
            Event::Next => {
                probe.stage += 1;
                match probe.stage {
                    1 => {
                        let mut settings = probe.app.settings.clone();
                        settings.window_opacity = 30;
                        settings.prefer_kepub = false;
                        settings.export_folder = Some(export.clone());
                        app::update(&mut probe.app, Message::SaveSettings(settings), svc.as_ref()).map(|message| Event::App(Box::new(message))).chain(delay())
                    }
                    2 => {
                        toasts::push(&mut probe.app, "Choose an existing writable export folder outside Calibre libraries. D:\\Calibre Library\\Exports cannot be used.".into(), true);
                        delay()
                    }
                    _ => iced::exit(),
                }
            }
        },
        view,
    ).theme(|_: &Probe| theme::dark()).title("Rulibre item 12 fixture probe").window_size((760.0, 520.0)).run();
    services.shutdown();
    result
}
