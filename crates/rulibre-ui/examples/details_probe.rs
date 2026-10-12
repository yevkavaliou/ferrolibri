//! Scripted native Settings captures using isolated config and library fixtures.
//! No personal configuration, physical input or reader is used.
use iced::{Element, Task};
use rulibre_ui::{
    app::{self, Message, Screen, State},
    services::{RealServices, Services},
    theme,
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
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-item-13");
    std::fs::create_dir_all(&out).unwrap();
    let names = [
        "details-single-760",
        "details-metadata-760",
        "details-multi-760",
        "details-error-760",
        "details-export-760",
    ];
    let result = iced::application(
        move || {
            let mut state = State::default();
            state.screen = Screen::Home {filter: app::Filter::All};
            state.scan_status = app::ScanStatus::Ready;
            state.catalog = rulibre_ui::catalog::Catalog::new((1..=8).map(|id| rulibre_core::book::Book {
                id: rulibre_core::book::BookId::Calibre(id), title: format!("Cyberiada · Кибериада {id}"), author: "Stanisław Lem".into(),
                path: library.join(format!("Author/Book ({id})")), files: vec![rulibre_core::book::BookFile {path: library.join("Book.txt"), format: rulibre_core::book::Format::Txt}],
            }).collect());
            let cover_id = rulibre_core::book::BookId::Calibre(1);
            state.covers.request(vec![cover_id.clone()]);
            let request = state.covers.next_requests().remove(0);
            let pixels: Vec<u8> = (0..192*264).flat_map(|i| { let y = i/192; [40 + (y/3) as u8, 70, 130, 255] }).collect();
            state.covers.finish(cover_id, request.1, Some(iced::widget::image::Handle::from_rgba(192, 264, pixels)));
            state.selection.selected.insert(rulibre_core::book::BookId::Calibre(1));
            state.metadata = Some(rulibre_core::metadata::Metadata {
                title: "Cyberiada · Кибериада".into(), authors: vec!["Stanisław Lem".into()], publisher: "Wydawnictwo Literackie".into(), date: "1965-01-01".into(), language: "pl".into(), series: "Fables".into(), series_index: "1".into(), rating: "8".into(), subjects: vec!["Science fiction".into()], identifiers: vec![("ISBN".into(),"978-83-08-01234-5".into())], description: "Fables for the cybernetic age.\n\nТрурль и Клапауций создают удивительные машины.".into(), ..Default::default()
            });
            state.backends = vec![rulibre_core::converter::Backend {kind: rulibre_core::converter::BackendKind::EbookConvert, path: Some("fixture-converter".into()), version: None, status: rulibre_core::converter::BackendStatus::Ready, diagnostic: "No KEPUB output support".into(), kepub_output: false}];
            state.library = Some(library.clone());
            (Probe { app: state, stage: 0 }, delay())
        },
        move |probe: &mut Probe, event| match event {
            Event::App(message) => app::update(&mut probe.app, *message, svc.as_ref()).map(|message| Event::App(Box::new(message))),
            Event::Capture => {
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
                        iced::widget::operation::scroll_to(iced::widget::Id::new("book-details-content"), iced::widget::scrollable::AbsoluteOffset {x: 0.0, y: 10000.0}).chain(delay())
                    }
                    2 => {
                        probe.app.selection.selected.extend(probe.app.catalog.order.clone());
                        probe.app.convert_menu = true;
                        iced::widget::operation::scroll_to(iced::widget::Id::new("book-details-content"), iced::widget::scrollable::AbsoluteOffset {x: 0.0, y: 0.0}).chain(delay())
                    }
                    3 => {
                        probe.app.selection.selected.clear();
                        probe.app.selection.selected.insert(rulibre_core::book::BookId::Calibre(1));
                        probe.app.covers.clear();
                        probe.app.metadata = None;
                        probe.app.metadata_error = Some("Malformed OPF: invalid XML at line 12".into());
                        probe.app.backends[0].status = rulibre_core::converter::BackendStatus::Missing;
                        delay()
                    }
                    4 => {
                        probe.app.convert_menu = false;
                        probe.app.export_prompt = true;
                        probe.app.conversion_export = Some(export.clone());
                        delay()
                    }
                    _ => iced::exit(),
                }
            }
        },
        view,
    ).theme(|_: &Probe| theme::dark()).title("Rulibre item 13 fixture probe").window_size((760.0, 520.0)).run();
    services.shutdown();
    result
}
