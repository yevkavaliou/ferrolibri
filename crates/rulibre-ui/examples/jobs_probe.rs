//! Render synthetic job states at minimum width. Real fixture sends are tested
//! separately in tests/send_jobs.rs. No personal configuration or readers used.
use iced::{Element, Task};
use rulibre_core::{
    device,
    jobs::{JobState, Stage},
    volumes::MountedVolume,
};
use rulibre_ui::{
    app::{self, Filter, Message, Screen, State},
    services::{
        RealServices, Services,
        jobs::{JobId, JobKind, JobView},
    },
    theme,
    widgets::toasts,
};
use std::{sync::Arc, time::Duration};

const NAMES: [&str; 6] = [
    "preparing-760",
    "copying-760",
    "jobs-760",
    "cancelled-760",
    "success-760",
    "error-760",
];
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
        async { tokio::time::sleep(Duration::from_millis(350)).await },
        |_| Event::Capture,
    )
}
fn view(p: &Probe) -> Element<'_, Event> {
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
fn main() -> iced::Result {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-item-11");
    std::fs::create_dir_all(&out).unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let mount = fixture.path().join("Kobo");
    std::fs::create_dir_all(mount.join(".kobo")).unwrap();
    std::fs::write(
        mount.join(".kobo/version"),
        b"SYNTHETIC-KOBO,unused,4.38.21908,00000000-0000-0000-0000-000000000000\n",
    )
    .unwrap();
    std::fs::write(
        mount.join(".kobo/KoboReader.sqlite"),
        b"SQLite format 3\0fixture",
    )
    .unwrap();
    let reader = device::detect_devices_from_volumes(&[MountedVolume {
        id: "fixture".into(),
        mount_point: mount,
        label: Some("KOBOeReader".into()),
        filesystem: "FAT32".into(),
        read_only: false,
    }])
    .devices
    .remove(0);
    let services = Arc::new(
        RealServices::with_paths(
            fixture.path().join("config.toml"),
            fixture.path().join("ui.toml"),
        )
        .unwrap(),
    );
    let svc = services.clone();
    let result=iced::application(move || {
        let mut state=State::default();state.screen=Screen::Home {filter:Filter::All};
        state.readers.devices=vec![reader.clone()];state.readers.selected_id=Some(reader.id.clone());
        for (n,title) in [(1,"Математика. Утрата определённости"),(2,"Harry Potter i kamień filozoficzny"),(3,"Cyberiada")] {
            state.selection.selected.insert(rulibre_core::book::BookId::Calibre(n));
            state.send_devices.insert(JobId(n),reader.id.clone());
            state.jobs.insert(JobId(n),JobView {id:JobId(n),title:title.into(),kind:JobKind::Send,state:if n==1 {JobState::Running} else {JobState::Waiting},stage:if n==1 {Some(Stage::Preparing)} else {None},progress:None,error:None});
        }
        (Probe {app:state,stage:0},delay())
    },move |p:&mut Probe,event| match event {
        Event::App(m)=>app::update(&mut p.app,*m,svc.as_ref()).map(|m|Event::App(Box::new(m))),
        Event::Capture=>iced::window::latest().then(|id|iced::window::screenshot(id.unwrap()).map(|s|Event::Shot(Box::new(s)))),
        Event::Shot(s)=>{let path=out.join(format!("{}.png",NAMES[p.stage]));image::save_buffer(&path,&s.rgba,s.size.width,s.size.height,image::ColorType::Rgba8).unwrap();println!("{} {}x{}",path.display(),s.size.width,s.size.height);Task::done(Event::Next)},
        Event::Next=>{p.stage+=1;match p.stage {
            1=>{let job=p.app.jobs.get_mut(&JobId(1)).unwrap();job.stage=Some(Stage::Copying);job.progress=Some((42,100));},
            2=>p.app.screen=Screen::Jobs,
            3=>{let job=p.app.jobs.get_mut(&JobId(2)).unwrap();job.state=JobState::Cancelled;p.app.finished_jobs.push_back(JobId(2));},
            4=>{let job=p.app.jobs.get_mut(&JobId(1)).unwrap();job.state=JobState::Succeeded;job.progress=Some((100,100));p.app.finished_jobs.push_back(JobId(1));p.app.job_messages.insert(JobId(1),"Sent to Kobo".into());toasts::push(&mut p.app,"Sent to Kobo".into(),false);},
            5=>{let job=p.app.jobs.get_mut(&JobId(3)).unwrap();job.state=JobState::Failed;let error="Transfer interrupted. Reader is unavailable. A partial file may remain at E:\\Books\\Cyberiada.epub.part. Reconnect the reader and check this file before retrying.".to_owned();job.error=Some(error.clone());p.app.finished_jobs.push_back(JobId(3));toasts::push(&mut p.app,error,true);},
            _=>return iced::exit(),
        }delay()},
    },view).theme(|_: &Probe|theme::dark()).window_size((760.0,520.0)).title("Rulibre item 11 fixture probe").run();
    services.shutdown();
    result
}
