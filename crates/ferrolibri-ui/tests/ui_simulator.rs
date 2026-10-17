//! UI-level tests through `iced_test`'s headless simulator: real `view`, real `update`,
//! synthetic fixture library. The simulator renders with tiny-skia, so no GPU is needed.
#[path = "../../ferrolibri-core/src/test_support.rs"]
mod test_support;
use ferrolibri_ui::{
    app::{self, Filter, Message, ScanStatus, Screen, State},
    catalog::Catalog,
    services::{RealServices, Services},
};
use iced::keyboard;

fn headless() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // `call_once` blocks other tests until the variable is set, and the simulator only
    // reads it afterwards, so no thread reads the environment while it is written.
    ONCE.call_once(|| unsafe { std::env::set_var("ICED_TEST_BACKEND", "tiny-skia") });
}
fn services(fixture: &test_support::Fixture) -> RealServices {
    RealServices::with_paths(
        fixture.root.join("config.toml"),
        fixture.root.join("ui.toml"),
    )
    .unwrap()
}
fn home(fixture: &test_support::Fixture) -> State {
    let mut state = State::default();
    state.screen = Screen::Home {
        filter: Filter::All,
    };
    state.library = Some(fixture.library.clone());
    state.catalog = Catalog::new(
        ferrolibri_core::scanner::scan_library(&fixture.library)
            .unwrap()
            .books,
    );
    state.scan_status = ScanStatus::Ready;
    state.window_width = 1100.0;
    state.viewport_width = 700.0;
    state.list_height = 500.0;
    state
}
/// Builds the real view, lets `act` drive it, then feeds every produced message to `update`.
fn interact<F>(state: &mut State, services: &RealServices, act: F)
where
    F: FnOnce(&mut iced_test::Simulator<'_, Message>),
{
    let messages: Vec<Message> = {
        let mut ui = iced_test::simulator(app::view(state));
        act(&mut ui);
        ui.into_messages().collect()
    };
    for message in messages {
        let _ = app::update(state, message, services);
    }
}
fn modifiers(state: &mut State, services: &RealServices, modifiers: keyboard::Modifiers) {
    let _ = app::update(
        state,
        Message::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
        services,
    );
}

#[test]
fn the_real_view_renders_and_the_simulator_finds_its_widgets() {
    headless();
    let fixture = test_support::Fixture::new();
    let state = home(&fixture);
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(ui.find("Home").is_ok());
    assert!(ui.find("Search").is_ok());
    assert!(ui.find("Settings").is_ok());
    assert!(ui.find("Second book").is_ok());
    assert!(ui.find("No such text anywhere").is_err());
}

fn is_send(job: &ferrolibri_ui::services::jobs::JobView) -> bool {
    job.kind == ferrolibri_ui::services::jobs::JobKind::Send
}
fn first_book_title(state: &State, index: usize) -> String {
    state
        .catalog
        .get(&state.catalog.order[index])
        .unwrap()
        .book
        .title
        .clone()
}

#[test]
fn clicking_sidebar_items_changes_the_screen_and_the_rendered_content() {
    headless();
    let fixture = test_support::Fixture::new();
    let services = services(&fixture);
    let mut state = home(&fixture);
    interact(&mut state, &services, |ui| {
        ui.click("Settings").unwrap();
    });
    assert_eq!(state.screen, Screen::Settings);
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(ui.find("Export folder").is_ok());
    assert!(ui.find("Second book").is_err(), "settings hides the books");
    drop(ui);
    interact(&mut state, &services, |ui| {
        ui.click("Jobs").unwrap();
    });
    assert_eq!(state.screen, Screen::Jobs);
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(ui.find("Clear finished").is_ok());
    drop(ui);
    interact(&mut state, &services, |ui| {
        ui.click("Search").unwrap();
    });
    assert_eq!(state.screen, Screen::Search);
    interact(&mut state, &services, |ui| {
        ui.click("Home").unwrap();
    });
    assert_eq!(
        state.screen,
        Screen::Home {
            filter: Filter::All
        }
    );
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(ui.find("Second book").is_ok());
    services.shutdown();
}

#[test]
fn click_selects_a_book_and_ctrl_click_adds_the_next_one() {
    headless();
    let fixture = test_support::Fixture::new();
    let services = services(&fixture);
    let mut state = home(&fixture);
    let (first, second) = (first_book_title(&state, 0), first_book_title(&state, 1));
    assert_ne!(first, second);
    interact(&mut state, &services, |ui| {
        ui.click(first.as_str()).unwrap();
    });
    assert_eq!(state.selection.selected.len(), 1);
    assert!(state.selection.selected.contains(&state.catalog.order[0]));
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(
        ui.find("Convert").is_ok(),
        "convert control is in the header"
    );
    drop(ui);
    // The app learns about Ctrl from the keyboard subscription, not from the click.
    modifiers(&mut state, &services, keyboard::Modifiers::CTRL);
    interact(&mut state, &services, |ui| {
        ui.click(second.as_str()).unwrap();
    });
    assert_eq!(state.selection.selected.len(), 2);
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(ui.find("2 selected").is_ok());
    drop(ui);
    // A second Ctrl+click on a selected book removes it again.
    interact(&mut state, &services, |ui| {
        ui.click(second.as_str()).unwrap();
    });
    assert_eq!(state.selection.selected.len(), 1);
    services.shutdown();
}

#[test]
fn shift_click_selects_the_range_between_two_books() {
    headless();
    let fixture = test_support::Fixture::new();
    let services = services(&fixture);
    let mut state = home(&fixture);
    let last = state.catalog.order.len() - 1;
    assert!(last >= 1);
    let (first, end) = (first_book_title(&state, 0), first_book_title(&state, last));
    interact(&mut state, &services, |ui| {
        ui.click(first.as_str()).unwrap();
    });
    modifiers(&mut state, &services, keyboard::Modifiers::SHIFT);
    interact(&mut state, &services, |ui| {
        ui.click(end.as_str()).unwrap();
    });
    assert_eq!(state.selection.selected.len(), last + 1);
    services.shutdown();
}

#[test]
fn the_details_toggle_in_the_toolbar_hides_and_restores_the_pane() {
    headless();
    let fixture = test_support::Fixture::new();
    let services = services(&fixture);
    let mut state = home(&fixture);
    interact(&mut state, &services, |ui| {
        ui.click(first_title(&fixture).as_str()).unwrap();
    });
    assert!(state.visible_settings().details_visible);
    let toolbar_y = {
        let mut ui = iced_test::simulator(app::view(&state));
        ui.find("FerroLibri").unwrap().bounds().center().y
    };
    let toggle = iced::Point::new(1024.0 - 18.0, toolbar_y);
    interact(&mut state, &services, |ui| {
        ui.point_at(toggle);
        let _ = ui.simulate(iced_test::simulator::click());
    });
    assert!(!state.visible_settings().details_visible);
    assert!(!ferrolibri_ui::widgets::shell::Layout::for_state(&state).details);
    interact(&mut state, &services, |ui| {
        ui.point_at(toggle);
        let _ = ui.simulate(iced_test::simulator::click());
    });
    assert!(state.visible_settings().details_visible);
    assert!(ferrolibri_ui::widgets::shell::Layout::for_state(&state).details);
    services.shutdown();
}

#[test]
fn convert_menu_opens_from_the_header_and_closes_on_click_away() {
    headless();
    let fixture = test_support::Fixture::new();
    let services = services(&fixture);
    let mut state = home(&fixture);
    {
        let mut ui = iced_test::simulator(app::view(&state));
        assert!(ui.find("Convert").is_ok());
    }
    interact(&mut state, &services, |ui| {
        ui.click("Convert").unwrap();
    });
    assert!(!state.convert_menu, "nothing is selected yet");
    interact(&mut state, &services, |ui| {
        ui.click(first_title(&fixture).as_str()).unwrap();
    });
    interact(&mut state, &services, |ui| {
        ui.click("Convert").unwrap();
    });
    assert!(state.convert_menu);
    {
        let mut ui = iced_test::simulator(app::view(&state));
        assert!(ui.find("Convert to").is_ok());
        assert!(ui.find("Refresh converters").is_ok());
    }
    let _ = app::update(&mut state, Message::CloseConvertMenu, &services);
    let mut ui = iced_test::simulator(app::view(&state));
    assert!(ui.find("Convert to").is_err());
    services.shutdown();
}
fn first_title(fixture: &test_support::Fixture) -> String {
    home(fixture)
        .catalog
        .get(&home(fixture).catalog.order[0])
        .unwrap()
        .book
        .title
        .clone()
}

#[test]
fn clicking_send_queues_one_job_per_selected_book_and_both_reach_the_reader() {
    use ferrolibri_core::{
        device,
        jobs::{JobExecutor, JobState},
        volumes::{MountedVolume, VolumeReport},
    };
    use ferrolibri_ui::{services::devices::DeviceUpdate, widgets::jobs::unfinished};
    headless();
    let fixture = test_support::Fixture::new();
    std::fs::write(
        fixture
            .library
            .join("Zed/Second book (43)/Second book.epub"),
        b"second ebook",
    )
    .unwrap();
    let library_before = test_support::snapshot(&fixture.library);
    let volume = MountedVolume {
        id: "fixture-kobo".into(),
        mount_point: fixture.kobo.clone(),
        label: Some("KOBOeReader".into()),
        filesystem: "FAT32".into(),
        read_only: false,
    };
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
    let mut state = home(&fixture);
    state.settings.prefer_kepub = false;
    let reader = device::detect_devices_from_volumes(std::slice::from_ref(&volume))
        .devices
        .remove(0);
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
        &services,
    );
    // Nothing is selected: the button is disabled and a click queues nothing.
    interact(&mut state, &services, |ui| {
        ui.click("Send to reader").unwrap();
    });
    assert!(!state.jobs.values().any(is_send));
    let (first, second) = (first_book_title(&state, 0), first_book_title(&state, 1));
    interact(&mut state, &services, |ui| {
        ui.click(first.as_str()).unwrap();
    });
    modifiers(&mut state, &services, keyboard::Modifiers::CTRL);
    interact(&mut state, &services, |ui| {
        ui.click(second.as_str()).unwrap();
    });
    interact(&mut state, &services, |ui| {
        ui.click("Send 2 to reader").unwrap();
    });
    assert_eq!(
        state.jobs.values().filter(|j| is_send(j)).count(),
        2,
        "one send job per selected book"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while state.jobs.values().any(unfinished) {
        let _ = app::update(
            &mut state,
            Message::Clock(std::time::Instant::now()),
            &services,
        );
        assert!(
            std::time::Instant::now() < deadline,
            "fixture send timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        state
            .jobs
            .values()
            .filter(|j| is_send(j))
            .all(|j| j.state == JobState::Succeeded)
    );
    let on_reader = test_support::snapshot(&fixture.kobo);
    assert!(
        on_reader
            .keys()
            .filter(|p| p.extension().is_some_and(|e| e == "epub"))
            .count()
            >= 2
    );
    assert_eq!(test_support::snapshot(&fixture.library), library_before);
    services.shutdown();
}

// Library choice screen (plugins plan item 3, design gap G1).
mod library_states {
    use super::*;
    use ferrolibri_core::config::LibraryPathState;
    use ferrolibri_ui::services::LibraryInspection;

    fn setup_state(
        state: LibraryPathState,
        files: &[&str],
        calibre_available: bool,
    ) -> (State, test_support::Fixture) {
        let fixture = test_support::Fixture::new();
        let path = fixture.root.join("chosen");
        let mut app = State::default();
        app.screen = Screen::Setup;
        app.setup_path = path.display().to_string();
        let _ = app::update(
            &mut app,
            Message::LibraryInspected {
                generation: 0,
                result: Ok(LibraryInspection {
                    path,
                    state,
                    files: files.iter().map(|name| name.to_string()).collect(),
                    calibre_available,
                }),
            },
            &services(&fixture),
        );
        (app, fixture)
    }
    fn texts(state: &State, present: &[&str], absent: &[&str]) {
        headless();
        let mut ui = iced_test::simulator(app::view(state));
        for text in present {
            assert!(ui.find(*text).is_ok(), "{text} should be shown");
        }
        for text in absent {
            assert!(ui.find(*text).is_err(), "{text} must not be shown");
        }
    }

    #[test]
    fn a_missing_folder_offers_retry_and_choose_another_but_never_creation() {
        let (mut state, fixture) = setup_state(LibraryPathState::Missing, &[], true);
        texts(
            &state,
            &["Folder unavailable", "Try again", "Choose another…"],
            &["Create library", "No Calibre library here"],
        );
        // Even a forced message cannot create anything for a missing folder.
        let services = services(&fixture);
        let _ = app::update(&mut state, Message::SetupCreateLibrary, &services);
        assert!(!state.setup_busy && !state.setup_creating);
        interact(&mut state, &services, |ui| {
            ui.click("Choose another…").unwrap();
        });
        assert!(state.setup_path.is_empty() && state.setup_prompt.is_none());
        texts(&state, &["Continue"], &["Folder unavailable"]);
    }

    #[test]
    fn an_empty_folder_asks_before_creating_and_blocks_input_while_creating() {
        let (mut state, fixture) = setup_state(LibraryPathState::NotLibraryEmpty, &[], true);
        texts(
            &state,
            &[
                "No Calibre library here",
                "Create library",
                "Choose another…",
            ],
            &[
                "Folder unavailable",
                "Calibre is needed to create a library",
            ],
        );
        let services = services(&fixture);
        interact(&mut state, &services, |ui| {
            ui.click("Create library").unwrap();
        });
        assert!(state.setup_busy && state.setup_creating);
        texts(&state, &["Creating the library…"], &[]);
        // A second click while the first creation runs does nothing.
        let before = state.setup_creating;
        let _ = app::update(&mut state, Message::SetupCreateLibrary, &services);
        assert_eq!(state.setup_creating, before);
    }

    #[test]
    fn a_folder_with_files_needs_a_separate_confirmation_and_lists_them() {
        let (mut state, fixture) = setup_state(
            LibraryPathState::NotLibraryNotEmpty,
            &["Photos", "notes.txt"],
            true,
        );
        texts(
            &state,
            &[
                "This folder has files but no Calibre library",
                "• Photos",
                "• notes.txt",
                "Create library",
            ],
            &["No Calibre library here"],
        );
        let services = services(&fixture);
        // Unconfirmed: the button is disabled and the message is ignored.
        interact(&mut state, &services, |ui| {
            let _ = ui.click("Create library");
        });
        assert!(!state.setup_busy);
        let _ = app::update(&mut state, Message::SetupCreateLibrary, &services);
        assert!(!state.setup_busy);
        interact(&mut state, &services, |ui| {
            ui.click("Create a library in this folder anyway").unwrap();
        });
        assert!(state.setup_prompt.as_ref().unwrap().confirmed);
        interact(&mut state, &services, |ui| {
            ui.click("Create library").unwrap();
        });
        assert!(state.setup_busy && state.setup_creating);
    }

    #[test]
    fn missing_calibre_replaces_creation_with_the_install_command_and_check_again() {
        for (library_state, files) in [
            (LibraryPathState::NotLibraryEmpty, vec![]),
            (LibraryPathState::NotLibraryNotEmpty, vec!["notes.txt"]),
        ] {
            let (mut state, fixture) = setup_state(library_state, &files, false);
            texts(
                &state,
                &[
                    "Calibre is needed to create a library",
                    app::calibre_install_hint(),
                    "Check again",
                    "Choose another…",
                ],
                &["Create library"],
            );
            let _ = app::update(&mut state, Message::SetupCreateLibrary, &services(&fixture));
            assert!(!state.setup_busy, "creation needs Calibre");
            interact(&mut state, &services(&fixture), |ui| {
                ui.click("Check again").unwrap();
            });
            assert!(state.setup_busy, "Check again inspects the folder again");
            assert!(state.setup_prompt.is_none());
        }
    }

    #[test]
    fn a_file_instead_of_a_folder_is_an_error_without_a_card() {
        let (state, _fixture) = setup_state(LibraryPathState::NotDirectory, &[], true);
        assert!(state.setup_prompt.is_none());
        assert_eq!(
            state.setup_error.as_deref(),
            Some("This path is a file, not a folder. Choose a folder.")
        );
        texts(
            &state,
            &["This path is a file, not a folder. Choose a folder."],
            &["Create library", "Folder unavailable"],
        );
    }

    #[test]
    fn editing_the_path_dismisses_the_card_and_a_ready_folder_shows_none() {
        let (mut state, fixture) = setup_state(LibraryPathState::NotLibraryEmpty, &[], true);
        let services = services(&fixture);
        let _ = app::update(
            &mut state,
            Message::SetupPathChanged("other".into()),
            &services,
        );
        assert!(state.setup_prompt.is_none());
        let ready = LibraryInspection {
            path: fixture.library.clone(),
            state: LibraryPathState::Ready,
            files: vec![],
            calibre_available: false,
        };
        let generation = 0;
        let _ = app::update(
            &mut state,
            Message::LibraryInspected {
                generation,
                result: Ok(ready),
            },
            &services,
        );
        assert!(state.setup_prompt.is_none() && state.setup_error.is_none());
    }
}
