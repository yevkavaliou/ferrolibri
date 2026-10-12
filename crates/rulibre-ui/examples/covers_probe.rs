//! Item 9: 5,000 catalog entries backed by real, read-only JPEG covers.
//! Pass a library folder and a persistent, isolated cache folder.
use iced::{Element, Subscription, Task};
use rulibre_core::{book::BookId, scanner::scan_library};
use rulibre_ui::{
    app::{self, Filter, Message, ScanStatus, Screen, State},
    catalog::Catalog,
    services::{RealServices, Services},
    theme,
    widgets::{book_grid, book_list},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
struct Probe {
    app: State,
    start: Instant,
    offset: f32,
    stage: usize,
    max_handles: usize,
    max_workers: usize,
    heartbeat: Instant,
    delays: Vec<f64>,
    screenshots: usize,
    measuring: bool,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Drive,
    Heartbeat,
    Capture,
    Shot(Box<iced::window::Screenshot>),
    Saved,
}
fn delay(event: Event, milliseconds: u64) -> Task<Event> {
    Task::perform(
        async move { tokio::time::sleep(Duration::from_millis(milliseconds)).await },
        move |_| event.clone(),
    )
}
fn apply(p: &mut Probe, m: Message, services: &dyn Services) -> Task<Event> {
    app::update(&mut p.app, m, services).map(|m| Event::App(Box::new(m)))
}
fn view(p: &Probe) -> Element<'_, Event> {
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
fn main() -> iced::Result {
    env_logger::init();
    let args: Vec<_> = std::env::args_os().collect();
    let library = PathBuf::from(args.get(1).expect("library folder argument"));
    let cache = PathBuf::from(args.get(2).expect("isolated cache folder argument"));
    let report = scan_library(&library).expect("read-only scan");
    let sources: Vec<_> = report
        .books
        .into_iter()
        .filter(|book| {
            rulibre_core::cover::find_cover(&book.path)
                .ok()
                .flatten()
                .is_some()
        })
        .collect();
    assert!(!sources.is_empty(), "library needs at least one real cover");
    println!(
        "real_cover_sources={} catalog_entries=5000 cache_before={}",
        sources.len(),
        std::fs::read_dir(&cache).map(|d| d.count()).unwrap_or(0)
    );
    let books: Vec<_> = (0..5000)
        .map(|i| {
            let mut book = sources[i % sources.len()].clone();
            book.id = BookId::Calibre(i as u64);
            book.author = "Cover benchmark".into();
            book.title = format!("{i:04} · {}", book.title);
            book
        })
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let services = Arc::new(
        RealServices::with_paths(temp.path().join("config.toml"), temp.path().join("ui.toml"))
            .unwrap()
            .with_cover_cache(cache.clone()),
    );
    let worker = services.clone();
    std::fs::create_dir_all("docs/ui-item-9").unwrap();
    let run = iced::application(move || {
        (Probe { app: {let mut state=State::default();state.screen=Screen::Home{filter:Filter::All};state.scan_status=ScanStatus::Ready;state.catalog=Catalog::new(books.clone());state}, start: Instant::now(), offset: 0.0, stage: 0, max_handles: 0, max_workers: 0, heartbeat: Instant::now(), delays: vec![], screenshots: 0, measuring: false }, delay(Event::Drive, 20))
    }, move |p: &mut Probe, event| {
        p.max_handles = p.max_handles.max(p.app.covers.len());
        p.max_workers = p.max_workers.max(p.app.covers.running.len());
        assert!(p.max_handles <= 300 && p.max_workers <= 4);
        match event {
            Event::App(m) => apply(p, *m, worker.as_ref()),
            Event::Heartbeat => {
                let now = Instant::now();
                if p.measuring { p.delays.push(now.duration_since(p.heartbeat).as_secs_f64() * 1000.0); }
                p.heartbeat = now;
                Task::none()
            }
            Event::Drive => {
                assert!(p.start.elapsed() < Duration::from_secs(600));
                if p.app.covers.wanted.is_empty() || p.app.covers.wanted.iter().any(|id| !p.app.covers.contains(id)) {
                    return delay(Event::Drive, 20);
                }
                if p.stage == 0 {
                    p.stage = 1;
                    return Task::done(Event::Capture);
                }
                let geometry = book_grid::Geometry::for_state(&p.app);
                let cols = geometry.columns(p.app.viewport_width);
                let total = 5000usize.div_ceil(cols) as f32 * geometry.row_height;
                if p.offset + p.app.list_height < total {
                    p.offset += p.app.list_height.max(geometry.row_height);
                    return Task::batch([iced::widget::operation::scroll_to(book_list::id(), iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: p.offset }), delay(Event::Drive, 40)]);
                }
                p.delays.sort_by(f64::total_cmp);
                let n = p.delays.len();
                println!("completed elapsed_s={:.3} max_handles={} max_workers={} heartbeat_n={} mean_ms={:.3} p95_ms={:.3} max_ms={:.3}", p.start.elapsed().as_secs_f64(), p.max_handles, p.max_workers, n, p.delays.iter().sum::<f64>() / n as f64, p.delays[n*95/100], p.delays[n-1]);
                p.app.list_offset = 0.0;
                p.measuring = false;
                let list = apply(p, Message::ShowList, worker.as_ref());
                p.stage = 2;
                Task::batch([list, iced::window::latest().then(|id| iced::window::resize(id.unwrap(), iced::Size::new(760.0, 520.0))), delay(Event::Capture, 1000)])
            }
            Event::Capture => iced::window::latest().then(|id| iced::window::screenshot(id.unwrap()).map(|s| Event::Shot(Box::new(s)))),
            Event::Shot(s) => {
                let label = if p.stage == 1 { "grid-1100" } else { "list-760" };
                p.screenshots += 1;
                Task::perform(async move { tokio::task::spawn_blocking(move || image::save_buffer(format!("docs/ui-item-9/{label}.png"), &s.rgba, s.size.width, s.size.height, image::ColorType::Rgba8).unwrap()).await.unwrap(); }, |_| Event::Saved)
            }
            Event::Saved => {
                if p.stage == 1 { p.measuring = true; p.heartbeat = Instant::now(); delay(Event::Drive, 20) } else { iced::window::latest().then(|id| iced::window::close(id.unwrap())) }
            }
        }
    }, view).title("Rulibre · item 9 covers probe").theme(theme::dark()).window(iced::window::Settings { size: iced::Size::new(1100.0, 700.0), ..Default::default() }).subscription(|p| Subscription::batch([app::subscription(&p.app).map(|m| Event::App(Box::new(m))), iced::time::every(Duration::from_millis(16)).map(|_| Event::Heartbeat)])).run();
    println!(
        "cache_after={}",
        std::fs::read_dir(&cache).map(|d| d.count()).unwrap_or(0)
    );
    services.shutdown();
    run
}
