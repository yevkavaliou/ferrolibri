#[path = "../src/test_support.rs"]
mod test_support;

use ferrolibri_core::{
    ErrorKind,
    config::{self, Config, LibraryPathState},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use test_support::Fixture;

#[test]
fn user_paths_handle_empty_quotes_spaces_unicode_and_relative_paths() {
    for input in ["", "  ", "\"\"", "''", "\"  \""] {
        assert!(matches!(
            config::sanitize_path(input).unwrap_err().kind,
            ErrorKind::InvalidInput
        ));
    }
    for input in ["'", "\"", "Книги с пробелами", "~someone/books"] {
        assert_eq!(
            config::sanitize_path(input).unwrap(),
            std::path::absolute(input).unwrap()
        );
    }
    let expected = std::path::absolute("Книги с пробелами").unwrap();
    for input in [
        "  Книги с пробелами  ",
        " 'Книги с пробелами' ",
        "\"Книги с пробелами\"",
    ] {
        assert_eq!(config::sanitize_path(input).unwrap(), expected);
    }
    let home = dirs::home_dir().unwrap();
    assert_eq!(config::sanitize_path("~").unwrap(), home);
    for input in ["~/Книги", "~\\Книги"] {
        assert_eq!(config::sanitize_path(input).unwrap(), home.join("Книги"));
    }
}

#[test]
fn legacy_toml_and_replacement_keep_the_same_schema() {
    let fixture = Fixture::new();
    let path = fixture.root.join("config.toml");
    fs::write(&path, "library_path = 'relative library'\n").unwrap();
    let config = Config::load_from(&path).unwrap().unwrap();
    assert_eq!(
        config.library_path,
        std::path::absolute("relative library").unwrap()
    );
    let config = Config {
        library_path: fixture.library.clone(),
    };
    config.save_to(&path).unwrap();
    assert_eq!(
        Config::load_from(&path).unwrap().unwrap().library_path,
        fixture.library
    );
    Config {
        library_path: fixture.book.clone(),
    }
    .save_to(&path)
    .unwrap();
    assert_eq!(
        Config::load_from(&path).unwrap().unwrap().library_path,
        fixture.book
    );
    assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ferrolibri-config-")
    }));
}

#[test]
fn saved_library_is_rechecked_and_empty_editor_template_is_allowed() {
    let fixture = Fixture::new();
    fs::write(fixture.library.join("metadata.db"), b"synthetic marker").unwrap();
    let path = fixture.root.join("config.toml");
    Config {
        library_path: fixture.library.clone(),
    }
    .save_to(&path)
    .unwrap();
    let loaded = Config::load_from(&path).unwrap().unwrap();
    assert!(config::is_calibre_library(&loaded.library_path).unwrap());
    fs::remove_file(fixture.library.join("metadata.db")).unwrap();
    assert!(!config::is_calibre_library(&loaded.library_path).unwrap());
    Config {
        library_path: PathBuf::new(),
    }
    .save_to(&path)
    .unwrap();
    assert!(
        Config::load_from(&path)
            .unwrap()
            .unwrap()
            .library_path
            .as_os_str()
            .is_empty()
    );
    assert!(!config::is_calibre_library(Path::new("")).unwrap());
}

#[cfg(windows)]
#[test]
fn windows_locked_config_survives_failed_replacement() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let path = fixture.root.join("config.toml");
    let config = Config {
        library_path: fixture.library.clone(),
    };
    config.save_to(&path).unwrap();
    let before = fs::read(&path).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let error = Config {
        library_path: fixture.book.clone(),
    }
    .save_to(&path)
    .unwrap_err();
    assert_eq!(error.operation, "replace config");
    assert!(matches!(error.kind, ErrorKind::Io(_)));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ferrolibri-config-")
    }));
    drop(lock);
    config.save_to(&path).unwrap();
}

#[cfg(unix)]
#[test]
fn non_unicode_path_serialization_fails_without_loss_or_overwrite() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let path = fixture.root.join("config.toml");
    fs::write(&path, b"previous config").unwrap();
    let config = Config {
        library_path: fixture.root.join(std::ffi::OsString::from_vec(vec![0xff])),
    };
    assert!(config.save_to(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"previous config");
}

#[test]
fn library_path_states_distinguish_missing_empty_and_foreign_folders() {
    let fixture = Fixture::new();
    let inspect = |path: &Path| config::inspect_library_path(path).unwrap();
    let empty = fixture.root.join("empty");
    fs::create_dir(&empty).unwrap();
    assert_eq!(inspect(&empty), LibraryPathState::NotLibraryEmpty);
    assert_eq!(
        inspect(&fixture.root.join("unplugged/Calibre Library")),
        LibraryPathState::Missing
    );
    let documents = fixture.root.join("documents");
    fs::create_dir_all(documents.join("Photos")).unwrap();
    fs::write(documents.join("notes.txt"), b"x").unwrap();
    assert_eq!(inspect(&documents), LibraryPathState::NotLibraryNotEmpty);
    let file = fixture.root.join("book.epub");
    fs::write(&file, b"x").unwrap();
    assert_eq!(inspect(&file), LibraryPathState::NotDirectory);
    // A directory named metadata.db is not a database.
    let fake = fixture.root.join("fake");
    fs::create_dir_all(fake.join("metadata.db")).unwrap();
    assert_eq!(inspect(&fake), LibraryPathState::NotLibraryNotEmpty);
    assert!(!config::is_calibre_library(&fake).unwrap());
    fs::write(fixture.library.join("metadata.db"), b"synthetic marker").unwrap();
    assert_eq!(inspect(&fixture.library), LibraryPathState::Ready);
    assert!(config::is_calibre_library(&fixture.library).unwrap());
    // A saved library whose database disappeared is no longer ready.
    fs::remove_file(fixture.library.join("metadata.db")).unwrap();
    assert_ne!(inspect(&fixture.library), LibraryPathState::Ready);
}

#[test]
fn inspecting_a_library_path_never_writes_and_rejects_empty_input() {
    let fixture = Fixture::new();
    let empty = fixture.root.join("empty");
    fs::create_dir(&empty).unwrap();
    config::inspect_library_path(&empty).unwrap();
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
    assert!(matches!(
        config::inspect_library_path(Path::new(""))
            .unwrap_err()
            .kind,
        ErrorKind::InvalidInput
    ));
    assert!(!config::is_calibre_library(&fixture.root.join("missing")).unwrap());
}

#[test]
fn first_entries_are_sorted_and_limited() {
    let fixture = Fixture::new();
    let folder = fixture.root.join("documents");
    fs::create_dir_all(folder.join("Photos")).unwrap();
    for name in ["b.txt", "a.txt", "c.txt"] {
        fs::write(folder.join(name), b"x").unwrap();
    }
    assert_eq!(
        config::first_entries(&folder, 10).unwrap(),
        ["Photos", "a.txt", "b.txt", "c.txt"]
    );
    assert_eq!(config::first_entries(&folder, 2).unwrap().len(), 2);
    assert!(config::first_entries(&fixture.root.join("missing"), 3).is_err());
}
