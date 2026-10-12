//! Item 15: the shell at renderer scale 1.0, 1.5 and 2.0 with real (synthetic JPEG)
//! covers. The OS scale stays at the machine's 100%; iced's own scale factor is
//! applied (iced resizes the window by that factor), so the layout area stays 760×520 logical,
//! as it would be at 150% and 200% OS scaling.
//! Run: cargo run -p rulibre-ui --example scaling_probe --locked
//! Writes docs/ui-item-15/scale-<percent>-<grid|list>.png
use iced::{Element, Task};
use rulibre_core::scanner::scan_library;
use rulibre_ui::{
    app::{self, Filter, Message, ScanStatus, Screen, State},
    catalog::Catalog,
    services::{RealServices, Services},
    settings::ViewMode,
    theme,
};
use std::{sync::Arc, time::Duration};

const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
struct Probe {
    app: State,
    scale: f32,
    step: usize,
    armed: bool,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Step,
    Shot(Box<iced::window::Screenshot>),
}
fn delay(milliseconds: u64) -> Task<Event> {
    Task::perform(
        async move { tokio::time::sleep(Duration::from_millis(milliseconds)).await },
        |_| Event::Step,
    )
}
fn view(p: &Probe) -> Element<'_, Event> {
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
/// A cover with fine detail so a soft (upscaled) image is visible: 3 px stripes
/// every 12 px and a one pixel border.
fn cover(seed: u8) -> image::RgbImage {
    image::RgbImage::from_fn(600, 825, |x, y| {
        let edge = x < 2 || y < 2 || x >= 598 || y >= 823;
        if edge || (x % 12 < 3) != (y % 36 < 18) && x > 40 && y > 300 {
            image::Rgb([245, 245, 245])
        } else {
            image::Rgb([40 + seed * 12, 70 + (y / 8) as u8 / 2, 130])
        }
    })
}
fn main() -> iced::Result {
    std::fs::create_dir_all("docs/ui-item-15").unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let library = fixture.path().join("Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(library.join("metadata.db"), b"marker").unwrap();
    for id in 0..12u8 {
        let book = library.join(format!("Автор {id}/Kamień filozoficzny ({id})"));
        std::fs::create_dir_all(&book).unwrap();
        std::fs::write(book.join("Book.epub"), b"synthetic").unwrap();
        cover(id).save(book.join("cover.jpg")).unwrap();
    }
    let books = scan_library(&library).unwrap().books;
    let cache = fixture.path().join("cache");
    let services = Arc::new(
        RealServices::with_paths(
            fixture.path().join("config.toml"),
            fixture.path().join("ui.toml"),
        )
        .unwrap()
        .with_cover_cache(cache),
    );
    let worker = services.clone();
    let result = iced::application(
        move || {
            let mut state = State::default();
            state.screen = Screen::Home {
                filter: Filter::All,
            };
            state.scan_status = ScanStatus::Ready;
            state.catalog = Catalog::new(books.clone());
            state.books = books.clone();
            state.settings.view = ViewMode::Grid;
            state.settings.details_visible = true;
            state.window_width = 760.0;
            (
                Probe {
                    app: state,
                    scale: 1.0,
                    step: 0,
                    armed: false,
                },
                delay(300),
            )
        },
        move |p: &mut Probe, event| match event {
            Event::App(m) => {
                app::update(&mut p.app, *m, worker.as_ref()).map(|m| Event::App(Box::new(m)))
            }
            Event::Step if !p.armed => {
                // Even steps: grid with the details pane, at the next scale.
                let scale = SCALES[p.step / 2];
                p.scale = scale;
                p.armed = true;
                let size = iced::Size::new(760.0, 520.0);
                let select = Message::BookClicked(p.app.catalog.order[0].clone());
                let grid = app::update(&mut p.app, Message::ShowGrid, worker.as_ref());
                let click = app::update(&mut p.app, select, worker.as_ref());
                // Show the keyboard cursor outline in the captures.
                p.app.keyboard_focus = true;
                Task::batch([
                    grid.map(|m| Event::App(Box::new(m))),
                    click.map(|m| Event::App(Box::new(m))),
                    iced::window::latest().then(move |id| iced::window::resize(id.unwrap(), size)),
                    delay(1500),
                ])
            }
            Event::Step => {
                p.armed = false;
                iced::window::latest()
                    .then(|id| iced::window::screenshot(id.unwrap()))
                    .map(|s| Event::Shot(Box::new(s)))
            }
            Event::Shot(shot) => {
                let kind = if p.step.is_multiple_of(2) {
                    "grid"
                } else {
                    "list"
                };
                let path = format!(
                    "docs/ui-item-15/scale-{}-{kind}.png",
                    (p.scale * 100.0) as u32
                );
                image::save_buffer(
                    &path,
                    &shot.rgba,
                    shot.size.width,
                    shot.size.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                println!(
                    "capture {path} {}x{} scale={}",
                    shot.size.width, shot.size.height, shot.scale_factor
                );
                p.step += 1;
                if p.step == SCALES.len() * 2 {
                    return iced::window::latest().then(|id| iced::window::close(id.unwrap()));
                }
                if p.step % 2 == 1 {
                    p.armed = true;
                    let list = app::update(&mut p.app, Message::ShowList, worker.as_ref());
                    return Task::batch([list.map(|m| Event::App(Box::new(m))), delay(800)]);
                }
                delay(100)
            }
        },
        view,
    )
    .title("Rulibre · item 15 scaling probe")
    .theme(theme::dark())
    .scale_factor(|p: &Probe| p.scale)
    .window(iced::window::Settings {
        size: iced::Size::new(760.0, 520.0),
        ..Default::default()
    })
    .subscription(|p: &Probe| app::subscription(&p.app).map(|m| Event::App(Box::new(m))))
    .run();
    services.shutdown();
    result
}
