//! UI counterpart of `docs/READINESS.md`: saved configuration → real scan job → selection →
//! send through the real coordinator and executor to a fixture reader, with the library
//! compared byte for byte afterwards. Only volume enumeration is injected; no physical reader.
#[path = "../../rulibre-core/src/test_support.rs"]
mod test_support;
use iced::futures::StreamExt;
use rulibre_core::{
    device,
    jobs::{JobExecutor, JobOutput, JobState},
    volumes::{MountedVolume, VolumeReport},
};
use rulibre_ui::{
    app::{self, Message, ScanStatus, Screen, State},
    services::{
        RealServices, Services,
        devices::DeviceUpdate,
        jobs::{JobId, JobKind},
    },
    widgets::jobs::unfinished,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

fn kobo(fixture: &test_support::Fixture) -> MountedVolume {
    MountedVolume {
        id: "fixture-kobo".into(),
        mount_point: fixture.kobo.clone(),
        label: Some("KOBOeReader".into()),
        filesystem: "FAT32".into(),
        read_only: false,
    }
}
/// Runs `message` and every task it spawns, the way the iced runtime would.
async fn drive(state: &mut State, message: Message, services: &dyn Services) {
    let mut tasks = VecDeque::from([app::update(state, message, services)]);
    while let Some(task) = tasks.pop_front() {
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(action) = stream.next().await {
                match action {
                    iced_runtime::Action::Output(message) => {
                        tasks.push_back(app::update(state, message, services))
                    }
                    iced_runtime::Action::Widget(_) => {}
                    _ => panic!("Unexpected non-widget runtime effect"),
                }
            }
        }
    }
}
async fn finish_scan(state: &mut State, services: &dyn Services) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.scan_status == ScanStatus::Running {
        drive(state, Message::Tick, services).await;
        assert!(Instant::now() < deadline, "scan timed out");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(state.scan_status, ScanStatus::Ready);
}
async fn finish_jobs(state: &mut State, services: &dyn Services) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while state.jobs.values().any(unfinished) {
        drive(state, Message::Clock(Instant::now()), services).await;
        assert!(Instant::now() < deadline, "jobs timed out");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
async fn reader_connected(state: &mut State, volume: &MountedVolume, services: &dyn Services) {
    let reader = device::detect_devices_from_volumes(std::slice::from_ref(volume))
        .devices
        .remove(0);
    drive(
        state,
        Message::Devices(DeviceUpdate {
            revision: 0,
            event: Some(device::DeviceEvent::Snapshot {
                devices: vec![reader],
                selected_id: None,
                changes: vec![],
                warnings: vec![],
            }),
            space: vec![],
        }),
        services,
    )
    .await;
}
/// Saves the library path, then loads settings and library the way startup does.
async fn start(
    fixture: &test_support::Fixture,
    services: &RealServices,
    volume: &MountedVolume,
) -> State {
    services
        .save_library(fixture.library.clone())
        .await
        .unwrap();
    let mut state = State::default();
    drive(&mut state, Message::LoadSettings, services).await;
    drive(&mut state, Message::LoadLibrary, services).await;
    finish_scan(&mut state, services).await;
    assert_eq!(state.library.as_deref(), Some(fixture.library.as_path()));
    assert!(matches!(state.screen, Screen::Home { .. }));
    state.settings.prefer_kepub = false;
    reader_connected(&mut state, volume, services).await;
    state
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn select_all(state: &mut State) {
    let order = state.catalog.order.clone();
    for (index, id) in order.iter().enumerate() {
        let modifiers = if index == 0 {
            iced::keyboard::Modifiers::empty()
        } else {
            iced::keyboard::Modifiers::CTRL
        };
        state.selection.click(id.clone(), modifiers, &order);
    }
}
fn send_ids(state: &State) -> Vec<JobId> {
    state
        .jobs
        .iter()
        .filter(|(_, job)| job.kind == JobKind::Send)
        .map(|(id, _)| *id)
        .collect()
}

#[test]
fn saved_config_scan_select_send_then_resend_conflicts_and_library_never_changes() {
    let fixture = test_support::Fixture::new();
    std::fs::write(
        fixture
            .library
            .join("Zed/Second book (43)/Second book.epub"),
        b"second ebook",
    )
    .unwrap();
    let library_before = test_support::snapshot(&fixture.library);
    let volume = kobo(&fixture);
    let feed = volume.clone();
    let executor = JobExecutor::with_volume_source(2, 16, move || {
        Ok(VolumeReport {
            volumes: vec![feed.clone()],
            warnings: vec![],
        })
    })
    .unwrap();
    let services = RealServices::with_paths_and_executor(
        fixture.root.join("config.toml"),
        fixture.root.join("ui.toml"),
        executor,
    )
    .unwrap();
    runtime().block_on(async {
        let mut state = start(&fixture, &services, &volume).await;
        assert_eq!(state.catalog.order.len(), 2);
        assert_eq!(test_support::snapshot(&fixture.library), library_before);
        select_all(&mut state);
        drive(&mut state, Message::Send, &services).await;
        finish_jobs(&mut state, &services).await;
        let ids = send_ids(&state);
        assert_eq!(ids.len(), 2);
        assert!(
            ids.iter()
                .all(|id| state.jobs[id].state == JobState::Succeeded)
        );
        for id in &ids {
            let Ok(JobOutput::Sent(path)) = state.results[id].as_ref() else {
                panic!("send output")
            };
            assert!(path.is_file());
            assert!(!path.to_string_lossy().ends_with(".part"));
            let source = library_before
                .iter()
                .find(|(source, _)| source.file_name() == path.file_name())
                .expect("sent file comes from the library");
            assert_eq!(std::fs::read(path).unwrap(), *source.1);
        }
        assert_eq!(test_support::snapshot(&fixture.library), library_before);
        // Conflict: the same books are already on the reader; nothing is overwritten.
        let reader_before = test_support::snapshot(&fixture.kobo);
        drive(&mut state, Message::Send, &services).await;
        finish_jobs(&mut state, &services).await;
        assert_eq!(
            state
                .jobs
                .values()
                .filter(|j| j.kind == JobKind::Send && j.state == JobState::Failed)
                .count(),
            2
        );
        assert!(
            state
                .toasts
                .iter()
                .filter(|t| t.error)
                .all(|t| t.message.contains("Destination already exists"))
        );
        assert_eq!(test_support::snapshot(&fixture.kobo), reader_before);
        assert_eq!(test_support::snapshot(&fixture.library), library_before);
    });
    services.shutdown();
}

#[test]
fn cancelling_a_running_and_a_waiting_send_leaves_reader_and_library_untouched() {
    let fixture = test_support::Fixture::new();
    std::fs::write(
        fixture.book.join("A small book.kepub.epub"),
        vec![8u8; 512 * 1024],
    )
    .unwrap();
    std::fs::write(
        fixture
            .library
            .join("Zed/Second book (43)/Second book.epub"),
        b"second ebook",
    )
    .unwrap();
    let library_before = test_support::snapshot(&fixture.library);
    let volume = kobo(&fixture);
    let feed = volume.clone();
    // Hold the worker at revalidation after the first copy chunk so the cancels land
    // while the first job is copying and the second is still waiting.
    let release = Arc::new(AtomicBool::new(false));
    let gate = release.clone();
    let entered = Arc::new(AtomicBool::new(false));
    let entry = entered.clone();
    let calls = AtomicUsize::new(0);
    let executor = JobExecutor::with_volume_source(1, 4, move || {
        if calls.fetch_add(1, Ordering::SeqCst) >= 5 {
            entry.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !gate.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(VolumeReport {
            volumes: vec![feed.clone()],
            warnings: vec![],
        })
    })
    .unwrap();
    let services = RealServices::with_paths_and_executor(
        fixture.root.join("config.toml"),
        fixture.root.join("ui.toml"),
        executor,
    )
    .unwrap();
    runtime().block_on(async {
        let mut state = start(&fixture, &services, &volume).await;
        select_all(&mut state);
        let reader_before = test_support::snapshot(&fixture.kobo);
        drive(&mut state, Message::Send, &services).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "copy never started");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let ids = send_ids(&state);
        assert_eq!(ids.len(), 2);
        drive(&mut state, Message::Cancel(ids[1]), &services).await;
        drive(&mut state, Message::Cancel(ids[0]), &services).await;
        release.store(true, Ordering::SeqCst);
        finish_jobs(&mut state, &services).await;
        assert!(
            ids.iter()
                .all(|id| state.jobs[id].state == JobState::Cancelled)
        );
        assert_eq!(test_support::snapshot(&fixture.kobo), reader_before);
        assert_eq!(test_support::snapshot(&fixture.library), library_before);
    });
    services.shutdown();
}
