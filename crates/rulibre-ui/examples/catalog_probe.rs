//! Native renderer probe for item 7. Synthetic catalog, isolated preferences.
use iced::{Element, Subscription, Task};
use rulibre_core::book::{Book, BookFile, BookId, Format};
use rulibre_ui::{
    app::{self, Filter, Message, ScanStatus, Screen, State},
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
    frame: usize,
    last: Option<Instant>,
    intervals: Vec<f64>,
    stage: usize,
}
#[derive(Clone, Debug)]
enum Event {
    App(Box<Message>),
    Frame,
    Capture,
    Shot(Box<iced::window::Screenshot>),
    Next,
}
fn later() -> Task<Event> {
    Task::perform(
        async { tokio::time::sleep(Duration::from_millis(500)).await },
        |_| Event::Capture,
    )
}
fn view(p: &Probe) -> Element<'_, Event> {
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
fn main() -> iced::Result {
    std::fs::create_dir_all("docs/ui-item-7").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let services = Arc::new(
        RealServices::with_paths(temp.path().join("config.toml"), temp.path().join("ui.toml"))
            .unwrap(),
    );
    let update_services = services.clone();
    let books: Vec<_> = (0..20_000)
        .map(|i| Book {
            id: BookId::Calibre(i),
            title: format!("Математика Łódź · книга {i:05} with a very long title"),
            author: format!("Автор {:03}", i % 100),
            files: vec![
                BookFile {
                    format: Format::Epub,
                    path: "book.epub".into(),
                },
                BookFile {
                    format: Format::Pdf,
                    path: "book.pdf".into(),
                },
            ],
            path: format!("book{i}").into(),
        })
        .collect();
    let result=iced::application(move || {
        let start=Instant::now();let catalog=Catalog::new(books.clone());println!("catalog_20k_ms={:.3}",start.elapsed().as_secs_f64()*1000.0);
        (Probe {app:{let mut state=State::default();state.screen=Screen::Home{filter:Filter::All};state.settings.view=rulibre_ui::settings::ViewMode::List;state.catalog=catalog;state.books=books.clone();state.scan_status=ScanStatus::Ready;state},frame:0,last:None,intervals:vec![],stage:0},Task::none())
    },move |p:&mut Probe,event|match event {
        Event::App(m)=>app::update(&mut p.app,*m,update_services.as_ref()).map(|m|Event::App(Box::new(m))),
        Event::Frame if p.frame<240=>{
            let now=Instant::now();if let Some(last)=p.last.replace(now) && p.frame>30 {p.intervals.push(now.duration_since(last).as_secs_f64()*1000.0);}
            p.frame+=1;
            if p.frame==240 {
                p.intervals.sort_by(f64::total_cmp);let n=p.intervals.len();
                println!("scroll_redraw_interval n={n} mean_ms={:.3} p95_ms={:.3} max_ms={:.3}",p.intervals.iter().sum::<f64>()/n as f64,p.intervals[n*95/100],p.intervals[n-1]);
                p.app.list_offset=0.0;
                return Task::batch([iced::widget::operation::scroll_to(rulibre_ui::widgets::book_list::id(),iced::widget::scrollable::AbsoluteOffset{x:0.0,y:0.0}),later()]);
            }
            // Continuous 15 px per redraw scroll through the real Scrollable operation.
            iced::widget::operation::scroll_to(rulibre_ui::widgets::book_list::id(),iced::widget::scrollable::AbsoluteOffset{x:0.0,y:p.frame as f32*15.0})
        }
        Event::Frame=>Task::none(),
        Event::Capture=>iced::window::latest().then(|id|iced::window::screenshot(id.unwrap()).map(|s|Event::Shot(Box::new(s)))),
        Event::Shot(s)=>{
            let names=["list-1100","list-760-selected","warnings-760","empty-760","grid-placeholder-760"];
            let path=format!("docs/ui-item-7/{}.png",names[p.stage]);
            image::save_buffer(&path,&s.rgba,s.size.width,s.size.height,image::ColorType::Rgba8).unwrap();println!("capture {path} {}x{}",s.size.width,s.size.height);Task::done(Event::Next)
        }
        Event::Next=>{
            p.stage+=1;
            match p.stage {
                1=>{let _=app::update(&mut p.app,Message::BookClicked(BookId::Calibre(0)),update_services.as_ref());p.app.modifiers=iced::keyboard::Modifiers::CTRL;let _=app::update(&mut p.app,Message::BookClicked(BookId::Calibre(100)),update_services.as_ref());Task::batch([iced::window::latest().then(|id|iced::window::resize(id.unwrap(),iced::Size::new(760.0,520.0))),later()])}
                2=>{p.app.scan_warnings=vec!["Could not read book folder (fixture warning). Other books remain available.".into()];p.app.show_warnings=true;later()}
                3=>{p.app.show_warnings=false;p.app.scan_warnings.clear();p.app.catalog=Catalog::default();p.app.books.clear();p.app.selection.clear();later()}
                4=>{p.app.catalog=Catalog::new(vec![Book{id:BookId::Calibre(1),title:"One book".into(),author:"Author".into(),files:vec![],path:"book".into()}]);p.app.settings.view=rulibre_ui::settings::ViewMode::Grid;later()}
                _=>iced::window::latest().then(|id|iced::window::close(id.unwrap())),
            }
        }
    },view).title("Rulibre · item 7 probe").theme(theme::dark()).window(iced::window::Settings{size:iced::Size::new(1100.0,700.0),..Default::default()}).subscription(|p|Subscription::batch([app::subscription(&p.app).map(|m|Event::App(Box::new(m))),iced::window::frames().map(|_|Event::Frame)])).run();
    services.shutdown();
    result
}
