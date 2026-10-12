//! Item 5 fixture run with a real iced window and the real service/coordinator.
//! Run: cargo run -p rulibre-ui --example setup_scan_probe --locked
//! Never loads or writes the user's config or library. No reader is involved.
use iced::{Element, Subscription, Task};
use rulibre_ui::{
    app::{self, Message, ScanStatus, Screen, State},
    services::{RealServices, Services},
};
use std::{
    cell::Cell,
    sync::Arc,
    time::{Duration, Instant},
};

struct Probe {
    app: State,
    library: String,
    started: Option<Instant>,
    last_tick: Instant,
    max_gap: Duration,
    ticks: u64,
    setup_views: Cell<u64>,
    scan_views: Cell<u64>,
    saved_run: bool,
    finished: bool,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Choose,
    Heartbeat,
    Close(Option<iced::window::Id>),
    Capture(bool),
    Screenshot(Box<iced::window::Screenshot>, bool),
}

impl Event {
    fn app(message: Message) -> Self {
        Self::App(Box::new(message))
    }
}

fn view(probe: &Probe) -> Element<'_, Event> {
    if probe.app.screen == Screen::Setup && !probe.app.library_loading {
        probe.setup_views.set(probe.setup_views.get() + 1);
    }
    if probe.app.scan_status == ScanStatus::Running {
        probe.scan_views.set(probe.scan_views.get() + 1);
    }
    app::view(&probe.app).map(Event::app)
}

fn main() -> iced::Result {
    std::fs::create_dir_all("target/ui-item-5").unwrap();
    let fixture = tempfile::tempdir().expect("temporary fixture");
    let library = fixture.path().join("Calibre Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("metadata.db"),
        b"setup marker; scanner reads folders",
    )
    .unwrap();
    let generation = Instant::now();
    for id in 0..20_000 {
        let book = library.join(format!("Author {}/Book ({id})", id / 100));
        std::fs::create_dir_all(&book).unwrap();
        std::fs::write(book.join("Book.epub"), b"synthetic fixture").unwrap();
    }
    println!(
        "fixture_books=20000 generation_ms={}",
        generation.elapsed().as_millis()
    );
    let services = Arc::new(
        RealServices::with_paths(
            fixture.path().join("config.toml"),
            fixture.path().join("ui.toml"),
        )
        .unwrap(),
    );
    let boot_services = services.clone();
    let update_services = services.clone();
    let library_text = library.display().to_string();
    let result = iced::application(
        move || {
            let mut state = State::default();
            let task = app::update(&mut state, Message::LoadLibrary, boot_services.as_ref());
            (Probe { app: state, library: library_text.clone(), started: None, last_tick: Instant::now(), max_gap: Duration::ZERO, ticks: 0, setup_views: Cell::new(0), scan_views: Cell::new(0), saved_run: false, finished: false }, task.map(Event::app))
        },
        move |probe: &mut Probe, event| {
            let mut task = Task::none();
            match event {
                Event::App(message) => {
                    let first_run = matches!(message.as_ref(), Message::LibraryLoaded { result: Ok(None), .. });
                    task = app::update(&mut probe.app, *message, update_services.as_ref()).map(Event::app);
                    if first_run {
                        task = Task::batch([task, Task::perform(async { tokio::time::sleep(Duration::from_millis(750)).await }, |_| Event::Capture(true))]);
                    }
                }
                Event::Choose => {
                    assert!(probe.setup_views.get() > 0, "Setup must reach the iced view");
                    let _ = app::update(&mut probe.app, Message::SetupPathChanged(probe.library.clone()), update_services.as_ref());
                    task = app::update(&mut probe.app, Message::ContinueSetup, update_services.as_ref()).map(Event::app);
                }
                Event::Heartbeat => {
                    if probe.started.is_some() {
                        let now = Instant::now();
                        probe.max_gap = probe.max_gap.max(now.duration_since(probe.last_tick));
                        probe.last_tick = now;
                        probe.ticks += 1;
                    }
                }
                Event::Capture(setup) => return iced::window::latest().then(move |id| {
                    iced::window::screenshot(id.expect("fixture window"))
                        .map(move |shot| Event::Screenshot(Box::new(shot), setup))
                }),
                Event::Screenshot(shot, setup) => {
                    let path = if setup { "target/ui-item-5/setup.png" } else { "target/ui-item-5/home.png" };
                    image::save_buffer(path, &shot.rgba, shot.size.width, shot.size.height, image::ColorType::Rgba8).unwrap();
                    println!("renderer_screenshot={path} scale={}", shot.scale_factor);
                    return if setup { Task::done(Event::Choose) } else { iced::window::latest().map(Event::Close) };
                }
                Event::Close(Some(id)) => return app::update(&mut probe.app, Message::Close(id), update_services.as_ref()).map(Event::app),
                Event::Close(None) => panic!("fixture window must exist"),
            }
            if probe.app.scan_status == ScanStatus::Running && probe.started.is_none() {
                probe.started = Some(Instant::now());
                probe.last_tick = Instant::now();
            }
            if probe.app.scan_status == ScanStatus::Ready && !probe.finished {
                assert_eq!(probe.app.books.len(), 20_000);
                assert!(probe.app.scan_warnings.is_empty());
                assert!(probe.ticks > 0 && probe.scan_views.get() > 0, "iced must update and render while scanning");
                probe.max_gap = probe.max_gap.max(probe.last_tick.elapsed());
                println!("mode={} books={} scan_ms={} heartbeat_ticks={} max_update_gap_ms={} setup_views={} scanning_views={}",
                    if probe.saved_run { "saved_config" } else { "first_run" }, probe.app.books.len(), probe.started.unwrap().elapsed().as_millis(), probe.ticks, probe.max_gap.as_millis(), probe.setup_views.get(), probe.scan_views.get());
                if probe.saved_run {
                    probe.finished = true;
                    return Task::perform(async { tokio::time::sleep(Duration::from_millis(150)).await }, |_| Event::Capture(false));
                }
                probe.saved_run = true;
                probe.app = State::default();
                probe.started = None;
                probe.max_gap = Duration::ZERO;
                probe.ticks = 0;
                probe.scan_views.set(0);
                return app::update(&mut probe.app, Message::LoadLibrary, update_services.as_ref()).map(Event::app);
            }
            if let ScanStatus::Failed(error) = &probe.app.scan_status { panic!("{error}"); }
            task
        },
        view,
    ).title("Rulibre · item 5 fixture probe")
        .window(iced::window::Settings { size: iced::Size::new(1100.0, 700.0), min_size: Some(iced::Size::new(760.0, 520.0)), exit_on_close_request: false, ..Default::default() })
        .theme(iced::Theme::Dark)
        .subscription(|probe: &Probe| Subscription::batch([app::subscription(&probe.app).map(Event::app), iced::time::every(Duration::from_millis(16)).map(|_| Event::Heartbeat)]))
        .run();
    services.shutdown();
    result
}
