//! Item 8: native rendering, real fixture scan, asynchronous search timings.
use iced::{Element, Task};
use rulibre_core::book::{Book, BookId, Format};
use rulibre_ui::{
    app::{self, FacetGroup, Filter, Message, ScanStatus, Screen, State},
    catalog::Catalog,
    services::{RealServices, Services},
    theme,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

struct Probe {
    app: State,
    stage: usize,
    query_started: Option<Instant>,
    timings: Vec<f64>,
    typed: usize,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Capture,
    Shot(Box<iced::window::Screenshot>),
    Next,
    Type,
}
fn later(event: Event, ms: u64) -> Task<Event> {
    Task::perform(
        async move { tokio::time::sleep(Duration::from_millis(ms)).await },
        move |_| event.clone(),
    )
}
fn apply(p: &mut Probe, m: Message, services: &dyn Services) -> Task<Event> {
    app::update(&mut p.app, m, services).map(|m| Event::App(Box::new(m)))
}
fn view(p: &Probe) -> Element<'_, Event> {
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
const QUERIES: [&str; 9] = [
    "К",
    "Кн",
    "Кни",
    "Книга",
    "Книга 1",
    "Ł",
    "Łó",
    "Łódź",
    "Математика",
];
fn main() -> iced::Result {
    std::fs::create_dir_all("docs/ui-item-8").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("Calibre Library");
    let mut snapshot = Vec::new();
    for (id, author, title, extension) in [
        (1, "Łukasz", "Żółw", "epub"),
        (2, "Łukasz", "Математика", "pdf"),
        (3, "ALICE", "First", "epub"),
        (4, "alice", "Second", "pdf"),
        (5, "Морис", "Книга", "epub"),
        (6, "Морис", "Другая", "pdf"),
    ] {
        let folder = library.join(author).join(format!("{title} ({id})"));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("book.{extension}"));
        let bytes = format!("fixture {id}").into_bytes();
        std::fs::write(&path, &bytes).unwrap();
        snapshot.push((path, bytes));
    }
    let db = library.join("metadata.db");
    std::fs::write(&db, b"fixture marker").unwrap();
    snapshot.push((db, b"fixture marker".to_vec()));
    let services = Arc::new(
        RealServices::with_paths(temp.path().join("config.toml"), temp.path().join("ui.toml"))
            .unwrap(),
    );
    let boot = services.clone();
    let updater = services.clone();
    let result = iced::application(
        move || {
            let mut p = Probe {
                app: State::default(),
                stage: 0,
                query_started: None,
                timings: vec![],
                typed: 0,
            };
            p.app.library = Some(library.clone());
            p.app.screen = Screen::Home {
                filter: Filter::All,
            };
            p.app.settings.details_visible = false;
            let _ = apply(
                &mut p,
                Message::ToggleFacetGroup(FacetGroup::Formats),
                boot.as_ref(),
            );
            let _ = apply(
                &mut p,
                Message::ToggleFacetGroup(FacetGroup::Authors),
                boot.as_ref(),
            );
            let scan = apply(&mut p, Message::Rescan, boot.as_ref());
            (p, Task::batch([scan, later(Event::Capture, 500)]))
        },
        move |p: &mut Probe, event| {
            match event {
                Event::App(m) => {
                    if matches!(*m, Message::SearchFinished { .. })
                        && p.stage == 9
                        && let Some(started) = p.query_started.take()
                    {
                        let ms = started.elapsed().as_secs_f64() * 1000.0;
                        p.timings.push(ms);
                        println!("search_round_trip query={} ms={ms:.3}", p.app.search_query);
                    }
                    apply(p, *m, updater.as_ref())
                }
                Event::Capture => {
                    if p.app.scan_status == ScanStatus::Running || p.app.search_running.is_some() {
                        return later(Event::Capture, 100);
                    }
                    assert_eq!(p.app.scan_status, ScanStatus::Ready);
                    if p.stage == 0 {
                        assert_eq!(p.app.catalog.entries.len(), 6);
                        assert_eq!(p.app.catalog.formats[&Format::Epub], 3);
                        assert_eq!(p.app.catalog.authors["alice"].1, 2);
                        // Exercise every format and author produced by the real scan.
                        let filters: Vec<_> = p
                            .app
                            .catalog
                            .formats
                            .keys()
                            .map(|f| Filter::Format(*f))
                            .chain(
                                p.app
                                    .catalog
                                    .authors
                                    .values()
                                    .map(|(a, _)| Filter::Author(a.clone())),
                            )
                            .collect();
                        for filter in filters {
                            let _ = apply(
                                p,
                                Message::Navigate(Screen::Home {
                                    filter: filter.clone(),
                                }),
                                updater.as_ref(),
                            );
                            assert_eq!(p.app.visible_order(), p.app.catalog.filter(&filter));
                            assert!(!p.app.visible_order().is_empty());
                        }
                        let _ = apply(
                            p,
                            Message::Navigate(Screen::Home {
                                filter: Filter::All,
                            }),
                            updater.as_ref(),
                        );
                        println!(
                            "real fixture scan: six books; every format/author navigation verified"
                        );
                    }
                    if p.stage == 4 {
                        assert_eq!(p.app.search_order.len(), 1);
                    }
                    if p.stage == 5 || p.stage == 8 {
                        assert_eq!(p.app.search_query, "ŻÓŁW");
                        assert_eq!(p.app.search_order.len(), 1);
                    }
                    iced::window::latest().then(|id| {
                        iced::window::screenshot(id.unwrap()).map(|s| Event::Shot(Box::new(s)))
                    })
                }
                Event::Shot(s) => {
                    let names = [
                        "home",
                        "format",
                        "author",
                        "search-empty",
                        "search-cyrillic",
                        "search-polish",
                        "jobs",
                        "settings",
                        "search-restored",
                        "search-20k",
                    ];
                    let path = format!("docs/ui-item-8/{}.png", names[p.stage]);
                    image::save_buffer(
                        &path,
                        &s.rgba,
                        s.size.width,
                        s.size.height,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                    println!("capture {path} {}x{}", s.size.width, s.size.height);
                    later(Event::Next, 10)
                }
                Event::Next => {
                    p.stage += 1;
                    let task = match p.stage {
                        1 => apply(
                            p,
                            Message::Navigate(Screen::Home {
                                filter: Filter::Format(Format::Epub),
                            }),
                            updater.as_ref(),
                        ),
                        2 => apply(
                            p,
                            Message::Navigate(Screen::Home {
                                filter: Filter::Author("Łukasz".into()),
                            }),
                            updater.as_ref(),
                        ),
                        3 => apply(p, Message::Navigate(Screen::Search), updater.as_ref()),
                        4 => apply(p, Message::SearchChanged("МАТЕМ".into()), updater.as_ref()),
                        5 => apply(p, Message::SearchChanged("ŻÓŁW".into()), updater.as_ref()),
                        6 => apply(p, Message::Navigate(Screen::Jobs), updater.as_ref()),
                        7 => apply(p, Message::Navigate(Screen::Settings), updater.as_ref()),
                        8 => apply(p, Message::Navigate(Screen::Search), updater.as_ref()),
                        9 => {
                            p.app.catalog = Catalog::new(
                                (0..20_000)
                                    .map(|i| Book {
                                        id: BookId::Calibre(i),
                                        title: format!("Книга {i:05} · Математика"),
                                        author: "Автор Łódź".into(),
                                        files: vec![],
                                        path: "synthetic".into(),
                                    })
                                    .collect(),
                            );
                            // Collapse author list during performance phase (one synthetic author).
                            return Task::done(Event::Type);
                        }
                        _ => {
                            assert_eq!(p.timings.len(), QUERIES.len());
                            p.timings.sort_by(f64::total_cmp);
                            println!(
                                "20k search_round_trip n={} mean_ms={:.3} max_ms={:.3}",
                                p.timings.len(),
                                p.timings.iter().sum::<f64>() / p.timings.len() as f64,
                                p.timings.last().unwrap()
                            );
                            return iced::window::latest()
                                .then(|id| iced::window::close(id.unwrap()));
                        }
                    };
                    let resize = if p.stage == 1 {
                        iced::window::latest().then(|id| {
                            iced::window::resize(id.unwrap(), iced::Size::new(760.0, 520.0))
                        })
                    } else {
                        Task::none()
                    };
                    Task::batch([task, resize, later(Event::Capture, 300)])
                }
                Event::Type => {
                    if p.app.search_running.is_some() {
                        return later(Event::Type, 10);
                    }
                    if p.typed == QUERIES.len() {
                        assert_eq!(p.app.search_order.len(), 20_000);
                        return later(Event::Capture, 300);
                    }
                    let query = QUERIES[p.typed];
                    p.typed += 1;
                    p.query_started = Some(Instant::now());
                    Task::batch([
                        apply(p, Message::SearchChanged(query.into()), updater.as_ref()),
                        later(Event::Type, 30),
                    ])
                }
            }
        },
        view,
    )
    .title("Rulibre · item 8 probe")
    .theme(theme::dark())
    .window(iced::window::Settings {
        size: iced::Size::new(1100.0, 700.0),
        ..Default::default()
    })
    .subscription(|p| app::subscription(&p.app).map(|m| Event::App(Box::new(m))))
    .run();
    services.shutdown();
    for (path, bytes) in snapshot {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    println!("fixture library contents unchanged; no physical reader used");
    result
}
