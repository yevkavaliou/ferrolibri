//! Native device-bar screenshots from core-classified folder fixtures, never real readers.
//! cargo run -p rulibre-ui --example devices_probe --locked
use iced::{Element, Task};
use rulibre_core::{
    device::{self, Device, DeviceEvent},
    volumes::MountedVolume,
};
use rulibre_ui::{
    app::{self, Filter, Message, Screen, State},
    services::{
        RealServices, Services,
        devices::{DeviceUpdate, Space},
    },
    theme,
};
use std::{sync::Arc, time::Duration};

const NAMES: [&str; 10] = [
    "none-760",
    "one-760",
    "many-760",
    "dropdown-760",
    "selected-760",
    "readonly-760",
    "warning-760",
    "poll-failed-760",
    "unknown-space-760",
    "manual-760",
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
fn snapshot(readers: Vec<Device>, warnings: Vec<String>) -> Message {
    let space = readers
        .iter()
        .map(|d| Space {
            id: d.id.clone(),
            path: d.mount_point.clone(),
            bytes: rulibre_core::volumes::free_space(&d.mount_point).ok(),
        })
        .collect();
    Message::Devices(DeviceUpdate {
        revision: 0,
        event: Some(DeviceEvent::Snapshot {
            devices: readers,
            selected_id: None,
            changes: vec![],
            warnings,
        }),
        space,
    })
}
fn view(p: &Probe) -> Element<'_, Event> {
    app::view(&p.app).map(|m| Event::App(Box::new(m)))
}
fn main() -> iced::Result {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-item-10");
    std::fs::create_dir_all(&out).unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let kobo = fixture.path().join("Kobo");
    std::fs::create_dir_all(kobo.join(".kobo")).unwrap();
    std::fs::write(
        kobo.join(".kobo/version"),
        b"SYNTHETIC-KOBO,unused,4.38.21908,00000000-0000-0000-0000-000000000000\n",
    )
    .unwrap();
    std::fs::write(
        kobo.join(".kobo/KoboReader.sqlite"),
        b"SQLite format 3\0fixture",
    )
    .unwrap();
    let kindle = fixture.path().join("Kindle");
    std::fs::create_dir_all(kindle.join("documents")).unwrap();
    std::fs::create_dir_all(kindle.join("system")).unwrap();
    std::fs::write(
        kindle.join("system/version.txt"),
        b"Kindle 5.4.0 (206145 050)\n",
    )
    .unwrap();
    let volumes = vec![
        MountedVolume {
            id: "fixture-kobo".into(),
            mount_point: kobo,
            label: Some("KOBOeReader".into()),
            filesystem: "FAT32".into(),
            read_only: false,
        },
        MountedVolume {
            id: "fixture-kindle".into(),
            mount_point: kindle,
            label: Some("Kindle".into()),
            filesystem: "FAT32".into(),
            read_only: false,
        },
    ];
    let readers = device::detect_devices_from_volumes(&volumes).devices;
    assert_eq!(readers.len(), 2);
    let services = Arc::new(
        RealServices::with_paths(
            fixture.path().join("config.toml"),
            fixture.path().join("ui.toml"),
        )
        .unwrap(),
    );
    let svc = services.clone();
    let result = iced::application(
        || {
            let mut state = State::default();
            state.screen = Screen::Home {
                filter: Filter::All,
            };
            state
                .selection
                .selected
                .insert(rulibre_core::book::BookId::Calibre(1));
            (
                Probe {
                    app: state,
                    stage: 0,
                },
                delay(),
            )
        },
        move |p: &mut Probe, event| match event {
            Event::App(m) => {
                app::update(&mut p.app, *m, svc.as_ref()).map(|m| Event::App(Box::new(m)))
            }
            Event::Capture => iced::window::latest()
                .then(|id| iced::window::screenshot(id.unwrap()).map(|s| Event::Shot(Box::new(s)))),
            Event::Shot(s) => {
                let path = out.join(format!("{}.png", NAMES[p.stage]));
                image::save_buffer(
                    &path,
                    &s.rgba,
                    s.size.width,
                    s.size.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                println!("{} {}x{}", path.display(), s.size.width, s.size.height);
                Task::done(Event::Next)
            }
            Event::Next => {
                p.stage += 1;
                let message = match p.stage {
                    1 => snapshot(vec![readers[0].clone()], vec![]),
                    2 => {
                        p.app.readers.selected_id = None;
                        snapshot(readers.clone(), vec![])
                    }
                    3 => Message::ToggleReaderMenu,
                    4 => Message::SelectReader(readers[1].id.clone()),
                    5 => {
                        let mut r = readers[1].clone();
                        r.read_only = true;
                        snapshot(vec![r], vec![])
                    }
                    6 => snapshot(
                        readers.clone(),
                        vec![
                            "Could not inspect fixture volume G:; other readers remain available."
                                .into(),
                        ],
                    ),
                    7 => Message::Devices(DeviceUpdate {
                        revision: 0,
                        event: Some(DeviceEvent::PollFailed {
                            diagnostic:
                                "Fixture volume enumeration failed. Waiting for the next poll."
                                    .into(),
                        }),
                        space: vec![],
                    }),
                    8 => {
                        let mut message = snapshot(vec![readers[0].clone()], vec![]);
                        if let Message::Devices(u) = &mut message {
                            for s in &mut u.space {
                                s.bytes = None;
                            }
                        }
                        message
                    }
                    9 => Message::ManualReader,
                    _ => {
                        svc.shutdown();
                        println!("Native fixture captures complete; no physical reader used.");
                        return iced::window::latest().then(|id| iced::window::close(id.unwrap()));
                    }
                };
                Task::batch([
                    app::update(&mut p.app, message, svc.as_ref()).map(|m| Event::App(Box::new(m))),
                    delay(),
                ])
            }
        },
        view,
    )
    .title("Rulibre · device bar fixture probe")
    .theme(theme::dark())
    .window(iced::window::Settings {
        size: iced::Size::new(760.0, 520.0),
        ..Default::default()
    })
    .run();
    services.shutdown();
    result
}
