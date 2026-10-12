//! Real core/coordinator IO, injected volume enumeration; no physical readers.
#[path = "../../rulibre-core/src/test_support.rs"]
mod test_support;
use rulibre_core::{
    device,
    jobs::{JobExecutor, JobState},
    volumes::{MountedVolume, VolumeReport},
};
use rulibre_ui::{
    app::{self, Filter, Message, Screen, State},
    catalog::Catalog,
    services::{RealServices, Services, devices::DeviceUpdate},
    widgets::jobs::{percentage, unfinished},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

fn volume(fixture: &test_support::Fixture) -> MountedVolume {
    MountedVolume {
        id: "fixture-kobo".into(),
        mount_point: fixture.kobo.clone(),
        label: Some("KOBOeReader".into()),
        filesystem: "FAT32".into(),
        read_only: false,
    }
}
fn state(
    fixture: &test_support::Fixture,
    volume: &MountedVolume,
    services: &RealServices,
) -> State {
    let reader = device::detect_devices_from_volumes(std::slice::from_ref(volume))
        .devices
        .remove(0);
    let mut state = State::default();
    state.screen = Screen::Home {
        filter: Filter::All,
    };
    state.library = Some(fixture.library.clone());
    state.settings.prefer_kepub = false;
    state.catalog = Catalog::new(
        rulibre_core::scanner::scan_library(&fixture.library)
            .unwrap()
            .books,
    );
    let _ = app::update(
        &mut state,
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
    );
    state
}
fn wait(state: &mut State, services: &RealServices) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while state.jobs.values().any(unfinished) {
        let _ = app::update(state, Message::Clock(Instant::now()), services);
        assert!(Instant::now() < deadline, "fixture send timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn three_sends_complete_report_copy_bytes_and_preserve_every_library_file() {
    let fixture = test_support::Fixture::new();
    // Every book has a directly compatible file; no external converter required.
    std::fs::write(
        fixture
            .library
            .join("Zed/Second book (43)/Second book.epub"),
        b"second ebook",
    )
    .unwrap();
    let third = fixture.library.join("Third/Third book (44)");
    std::fs::create_dir_all(&third).unwrap();
    std::fs::write(third.join("Third.epub"), vec![7u8; 256 * 1024]).unwrap();
    let before = test_support::snapshot(&fixture.library);
    let mounted = volume(&fixture);
    let feed = mounted.clone();
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
    let mut state = state(&fixture, &mounted, &services);
    assert_eq!(state.catalog.order.len(), 3);
    // First, third, second: the result identities reflect selection chronology.
    let order = state.catalog.order.clone();
    for index in [0, 2, 1] {
        state.selection.click(
            order[index].clone(),
            iced::keyboard::Modifiers::CTRL,
            &order,
        );
    }
    let selected = state.selection.ordered();
    let titles: Vec<_> = selected
        .iter()
        .map(|id| state.catalog.get(id).unwrap().book.title.clone())
        .collect();
    let _ = app::update(&mut state, Message::Send, &services);
    assert_eq!(state.jobs.len(), 3);
    assert_eq!(
        state
            .jobs
            .values()
            .map(|j| j.title.clone())
            .collect::<Vec<_>>(),
        titles
    );
    wait(&mut state, &services);
    assert!(state.jobs.values().all(|j| j.state == JobState::Succeeded));
    assert!(state.jobs.values().all(|j| percentage(j) == Some(100.0)));
    for result in state.results.values() {
        let Ok(rulibre_core::jobs::JobOutput::Sent(path)) = result.as_ref() else {
            panic!("sent output")
        };
        assert!(path.starts_with(std::fs::canonicalize(&fixture.kobo).unwrap()));
        assert!(path.is_file());
        let source = before
            .iter()
            .find(|(source, _)| source.file_name() == path.file_name())
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), *source.1);
        assert!(!path.to_string_lossy().ends_with(".part"));
    }
    assert_eq!(state.toasts.len(), 3);
    assert!(
        state
            .toasts
            .iter()
            .all(|t| !t.error && t.message == "Sent to Kobo")
    );
    assert_eq!(test_support::snapshot(&fixture.library), before);
    let reader_before = test_support::snapshot(&fixture.kobo);
    // Sending the same selection produces conflict diagnostics without overwrites.
    let _ = app::update(&mut state, Message::Send, &services);
    wait(&mut state, &services);
    assert_eq!(
        state
            .jobs
            .values()
            .filter(|j| j.state == JobState::Failed)
            .count(),
        3
    );
    assert!(
        state
            .toasts
            .iter()
            .filter(|t| t.error)
            .all(|t| t.message.contains("Destination already exists"))
    );
    assert_eq!(test_support::snapshot(&fixture.kobo), reader_before);
    assert_eq!(test_support::snapshot(&fixture.library), before);
    services.shutdown();
}
#[test]
fn reader_disappears_after_first_copy_chunk_and_partial_path_reaches_jobs_and_toast() {
    let fixture = test_support::Fixture::new();
    std::fs::write(
        fixture.book.join("A small book.kepub.epub"),
        vec![8u8; 512 * 1024],
    )
    .unwrap();
    let before = test_support::snapshot(&fixture.library);
    let mounted = volume(&fixture);
    let feed = mounted.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let executor = JobExecutor::with_volume_source(1, 4, move || {
        let count = counter.fetch_add(1, Ordering::SeqCst);
        Ok(VolumeReport {
            volumes: if count < 5 {
                vec![feed.clone()]
            } else {
                vec![]
            },
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
    let mut state = state(&fixture, &mounted, &services);
    let id = state
        .catalog
        .entries
        .iter()
        .find(|e| e.book.path == fixture.book)
        .unwrap()
        .book
        .id
        .clone();
    state
        .selection
        .click(id, iced::keyboard::Modifiers::empty(), &state.catalog.order);
    let _ = app::update(&mut state, Message::Send, &services);
    wait(&mut state, &services);
    let job = state.jobs.values().next().unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert!(
        job.progress
            .is_some_and(|(done, total)| done > 0 && done < total)
    );
    assert!(job.error.as_ref().unwrap().contains(".part"));
    assert!(state.toasts[0].error);
    assert!(state.toasts[0].message.contains("partial file may remain"));
    assert!(
        test_support::snapshot(&fixture.kobo)
            .keys()
            .any(|p| p.to_string_lossy().ends_with(".part"))
    );
    assert_eq!(test_support::snapshot(&fixture.library), before);
    services.shutdown();
}
#[test]
fn running_cancel_stops_copy_and_waiting_cancel_never_writes() {
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
    let before = test_support::snapshot(&fixture.library);
    let mounted = volume(&fixture);
    let feed = mounted.clone();
    // Hold the worker at revalidation after the first copy chunk, until both
    // cancels have been requested. This verifies cancellation during copying.
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
    let mut state = state(&fixture, &mounted, &services);
    let first = state
        .catalog
        .entries
        .iter()
        .find(|entry| entry.book.path == fixture.book)
        .unwrap()
        .book
        .id
        .clone();
    state.selection.click(
        first.clone(),
        iced::keyboard::Modifiers::empty(),
        &state.catalog.order,
    );
    for id in &state.catalog.order {
        if id != &first {
            state.selection.click(
                id.clone(),
                iced::keyboard::Modifiers::CTRL,
                &state.catalog.order,
            );
        }
    }
    let reader_before = test_support::snapshot(&fixture.kobo);
    let _ = app::update(&mut state, Message::Send, &services);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entered.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let ids: Vec<_> = state.jobs.keys().copied().collect();
    let _ = app::update(&mut state, Message::Cancel(ids[1]), &services);
    assert!(
        state.jobs[&ids[0]]
            .progress
            .is_some_and(|(done, total)| done > 0 && done < total)
    );
    let _ = app::update(&mut state, Message::Cancel(ids[0]), &services);
    release.store(true, Ordering::SeqCst);
    wait(&mut state, &services);
    assert!(state.jobs.values().all(|j| j.state == JobState::Cancelled));
    assert!(
        state
            .toasts
            .iter()
            .all(|t| !t.error && t.message.starts_with("Cancelled"))
    );
    assert_eq!(test_support::snapshot(&fixture.kobo), reader_before);
    assert_eq!(test_support::snapshot(&fixture.library), before);
    services.shutdown();
}
