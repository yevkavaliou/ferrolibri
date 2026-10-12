//! Item 15 performance pass: a real window, the real services and a synthetic
//! 20,000-book library on disk. Isolated config and preferences.
//! Run: cargo run --release -p rulibre-ui --example perf_probe --locked
//! Measures: startup to first paint, scan, scan result to first paint, search
//! round trip per keystroke, keyboard cursor update cost, list and grid scrolling.
use iced::{Element, Subscription, Task};
use rulibre_ui::{
    app::{self, Filter, Message, ScanStatus, Screen, State},
    services::{RealServices, Services},
    settings::ViewMode,
};
use std::{
    cell::Cell,
    sync::Arc,
    time::{Duration, Instant},
};

const BOOKS: usize = 20_000;
const SCROLL_FRAMES: usize = 240;
const QUERY: &str = "Book (12345";

struct Probe {
    app: State,
    start: Instant,
    first_paint: Option<Instant>,
    scan_started: Option<Instant>,
    ready_at: Option<Instant>,
    view_saw_ready: Cell<Option<Instant>>,
    paint_after_ready: Option<Duration>,
    stage: Stage,
    typed: usize,
    sent_at: Option<Instant>,
    round_trips: Vec<f64>,
    frame: usize,
    last_frame: Option<Instant>,
    intervals: Vec<f64>,
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Loading,
    Typing,
    Keyboard,
    ListScroll,
    GridScroll,
    Done,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Frame,
    Type,
}
fn view(p: &Probe) -> Element<'_, Event> {
    if p.app.scan_status == ScanStatus::Ready && p.view_saw_ready.get().is_none() {
        p.view_saw_ready.set(Some(Instant::now()));
    }
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
fn stats(name: &str, values: &mut [f64]) {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    println!(
        "{name} n={n} mean_ms={:.3} p95_ms={:.3} max_ms={:.3}",
        values.iter().sum::<f64>() / n as f64,
        values[(n * 95 / 100).min(n - 1)],
        values[n - 1]
    );
}
fn key(named: iced::keyboard::key::Named, modifiers: iced::keyboard::Modifiers) -> Message {
    Message::Keyboard(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Named(named),
        modified_key: iced::keyboard::Key::Named(named),
        physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::Escape),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}
fn main() -> iced::Result {
    let fixture = tempfile::tempdir().expect("temporary fixture");
    let library = fixture.path().join("Calibre Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("metadata.db"),
        b"perf marker; scanner reads folders",
    )
    .unwrap();
    for id in 0..BOOKS {
        let book = library.join(format!("Author {}/Book ({id})", id / 100));
        std::fs::create_dir_all(&book).unwrap();
        std::fs::write(book.join("Book.epub"), b"synthetic fixture").unwrap();
    }
    let config = fixture.path().join("config.toml");
    std::fs::write(
        &config,
        format!("library_path = {:?}\n", library.display().to_string()),
    )
    .unwrap();
    let services =
        Arc::new(RealServices::with_paths(config, fixture.path().join("ui.toml")).unwrap());
    let boot = services.clone();
    let updater = services.clone();
    let start = Instant::now();
    let result = iced::application(
        move || {
            let mut state = State::default();
            let task = app::update(&mut state, Message::LoadLibrary, boot.as_ref());
            (
                Probe {
                    app: state,
                    start,
                    first_paint: None,
                    scan_started: None,
                    ready_at: None,
                    view_saw_ready: Cell::new(None),
                    paint_after_ready: None,
                    stage: Stage::Loading,
                    typed: 0,
                    sent_at: None,
                    round_trips: vec![],
                    frame: 0,
                    last_frame: None,
                    intervals: vec![],
                },
                task.map(|m| Event::App(Box::new(m))),
            )
        },
        move |p: &mut Probe, event| match event {
            Event::App(message) => {
                let finished_search = matches!(*message, Message::SearchFinished { .. });
                let task = app::update(&mut p.app, *message, updater.as_ref())
                    .map(|m| Event::App(Box::new(m)));
                if p.app.scan_status == ScanStatus::Running && p.scan_started.is_none() {
                    p.scan_started = Some(Instant::now());
                }
                if let ScanStatus::Failed(error) = &p.app.scan_status {
                    panic!("{error}");
                }
                if p.app.scan_status == ScanStatus::Ready && p.ready_at.is_none() {
                    p.ready_at = Some(Instant::now());
                    assert_eq!(p.app.books.len(), BOOKS);
                    println!(
                        "scan_ms={} books={}",
                        p.scan_started.unwrap().elapsed().as_millis(),
                        p.app.books.len()
                    );
                    return Task::batch([
                        task,
                        Task::done(Event::App(Box::new(Message::Navigate(Screen::Search)))),
                        Task::perform(
                            async { tokio::time::sleep(Duration::from_millis(500)).await },
                            |_| Event::Type,
                        ),
                    ]);
                }
                if finished_search && p.stage == Stage::Typing {
                    if let Some(sent) = p.sent_at.take() {
                        p.round_trips.push(sent.elapsed().as_secs_f64() * 1000.0);
                    }
                    return Task::batch([
                        task,
                        Task::perform(
                            async { tokio::time::sleep(Duration::from_millis(60)).await },
                            |_| Event::Type,
                        ),
                    ]);
                }
                task
            }
            Event::Type => {
                if p.stage == Stage::Loading {
                    p.stage = Stage::Typing;
                }
                if p.typed < QUERY.chars().count() {
                    p.typed += 1;
                    let query: String = QUERY.chars().take(p.typed).collect();
                    p.sent_at = Some(Instant::now());
                    return app::update(
                        &mut p.app,
                        Message::SearchChanged(query),
                        updater.as_ref(),
                    )
                    .map(|m| Event::App(Box::new(m)));
                }
                stats("search_round_trip", &mut p.round_trips);
                // Keyboard cursor cost: Down presses through the real update.
                p.stage = Stage::Keyboard;
                let _ = app::update(
                    &mut p.app,
                    Message::Navigate(Screen::Home {
                        filter: Filter::All,
                    }),
                    updater.as_ref(),
                );
                p.app.settings.view = ViewMode::List;
                p.app.list_height = 600.0;
                let mut costs = Vec::new();
                for _ in 0..300 {
                    let t = Instant::now();
                    let _ = app::update(
                        &mut p.app,
                        key(
                            iced::keyboard::key::Named::ArrowDown,
                            iced::keyboard::Modifiers::empty(),
                        ),
                        updater.as_ref(),
                    );
                    costs.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                stats("arrow_down_update", &mut costs);
                let t = Instant::now();
                let _ = app::update(
                    &mut p.app,
                    key(
                        iced::keyboard::key::Named::End,
                        iced::keyboard::Modifiers::SHIFT,
                    ),
                    updater.as_ref(),
                );
                println!(
                    "shift_end_select_range_ms={:.3} selected={}",
                    t.elapsed().as_secs_f64() * 1000.0,
                    p.app.selection.selected.len()
                );
                let _ = app::update(&mut p.app, Message::ClearSelection, updater.as_ref());
                p.stage = Stage::ListScroll;
                p.frame = 0;
                p.last_frame = None;
                p.intervals.clear();
                Task::none()
            }
            Event::Frame => {
                let now = Instant::now();
                if p.first_paint.is_none() {
                    p.first_paint = Some(now);
                    println!(
                        "first_paint_ms={:.1} (process fixture ready to first frame)",
                        now.duration_since(p.start).as_secs_f64() * 1000.0
                    );
                }
                if p.paint_after_ready.is_none()
                    && let (Some(ready), Some(seen)) = (p.ready_at, p.view_saw_ready.get())
                {
                    p.paint_after_ready = Some(now.duration_since(ready));
                    println!(
                        "scan_result_to_first_paint_ms={:.1} (view built after {:.1} ms)",
                        now.duration_since(ready).as_secs_f64() * 1000.0,
                        seen.duration_since(ready).as_secs_f64() * 1000.0
                    );
                }
                if !matches!(p.stage, Stage::ListScroll | Stage::GridScroll) {
                    return Task::none();
                }
                if let Some(last) = p.last_frame.replace(now)
                    && p.frame > 30
                {
                    p.intervals
                        .push(now.duration_since(last).as_secs_f64() * 1000.0);
                }
                p.frame += 1;
                let step = if p.stage == Stage::ListScroll {
                    15.0
                } else {
                    40.0
                };
                if p.frame == SCROLL_FRAMES {
                    stats(
                        if p.stage == Stage::ListScroll {
                            "list_scroll_redraw_interval"
                        } else {
                            "grid_scroll_redraw_interval"
                        },
                        &mut p.intervals,
                    );
                    p.intervals.clear();
                    p.frame = 0;
                    p.last_frame = None;
                    if p.stage == Stage::ListScroll {
                        p.stage = Stage::GridScroll;
                        let task = app::update(&mut p.app, Message::ShowGrid, updater.as_ref())
                            .map(|m| Event::App(Box::new(m)));
                        return task;
                    }
                    p.stage = Stage::Done;
                    return iced::window::latest().then(|id| iced::window::close(id.unwrap()));
                }
                iced::widget::operation::scroll_to(
                    rulibre_ui::widgets::book_list::id(),
                    iced::widget::scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: p.frame as f32 * step,
                    },
                )
            }
        },
        view,
    )
    .title("Rulibre · item 15 performance probe")
    .theme(rulibre_ui::theme::dark())
    .window(iced::window::Settings {
        size: iced::Size::new(1100.0, 700.0),
        min_size: Some(iced::Size::new(760.0, 520.0)),
        ..Default::default()
    })
    .subscription(|p: &Probe| {
        Subscription::batch([
            app::subscription(&p.app).map(|m| Event::App(Box::new(m))),
            iced::window::frames().map(|_| Event::Frame),
        ])
    })
    .run();
    services.shutdown();
    result
}
