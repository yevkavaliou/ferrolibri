//! Windows window-blur probe: the real app view in a transparent window with the
//! acrylic backdrop, opacity from PROBE_OPACITY (default 60). Capture the desktop
//! with scripts/capture-blur.ps1; iced's own screenshots cannot show the backdrop.
use iced::{Element, Task};
use rulibre_core::book::{Book, BookFile, BookId, Format};
use rulibre_ui::{
    app::{self, Message, Screen, State},
    services::RealServices,
    theme,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Quit,
}
fn view(state: &State) -> Element<'_, Event> {
    app::view(state).map(|m| Event::App(Box::new(m)))
}
fn main() -> iced::Result {
    rulibre_ui::blur::prepare_environment();
    let opacity: u8 = std::env::var("PROBE_OPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let fixture = tempfile::tempdir().unwrap();
    let services = Arc::new(
        RealServices::with_paths(
            fixture.path().join("config.toml"),
            fixture.path().join("ui.toml"),
        )
        .unwrap(),
    );
    let books: Vec<Book> = (1..=6u64)
        .map(|id| Book {
            id: BookId::Calibre(id),
            title: format!("Blur probe book {id}"),
            author: "Fixture Author".into(),
            path: fixture.path().join(format!("b{id}")),
            files: vec![BookFile {
                path: fixture.path().join("x.epub"),
                format: Format::Epub,
            }],
        })
        .collect();
    let update_services = services.clone();
    iced::application(
        move || {
            let mut state = State::default();
            state.screen = Screen::Home {
                filter: app::Filter::All,
            };
            state.scan_status = app::ScanStatus::Ready;
            state.catalog = rulibre_ui::catalog::Catalog::new(books.clone());
            state.library = Some(fixture.path().to_owned());
            state.settings.window_opacity = opacity;
            state.selection.selected.insert(BookId::Calibre(2));
            let task = app::update(&mut state, Message::Ignore, services.as_ref());
            (
                state,
                Task::batch([
                    task.map(|m| Event::App(Box::new(m))),
                    Task::perform(
                        async { tokio::time::sleep(Duration::from_secs(9)).await },
                        |_| Event::Quit,
                    ),
                ]),
            )
        },
        move |state: &mut State, event| match event {
            Event::App(message) => app::update(state, *message, update_services.as_ref())
                .map(|m| Event::App(Box::new(m))),
            Event::Quit => iced::exit(),
        },
        view,
    )
    .theme(|_: &State| theme::dark())
    .style(app::style)
    .window(iced::window::Settings {
        size: iced::Size::new(1000.0, 640.0),
        position: iced::window::Position::Specific(iced::Point::new(500.0, 150.0)),
        decorations: !rulibre_ui::blur::ready(),
        transparent: rulibre_ui::blur::ready(),
        #[cfg(target_os = "windows")]
        platform_specific: iced::window::settings::PlatformSpecific {
            undecorated_shadow: rulibre_ui::blur::ready(),
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}
