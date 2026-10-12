#[path = "../src/test_support.rs"]
mod test_support;

use rulibre_core::{
    Error, ErrorKind,
    config::{self, Config},
    converter,
    device::{DeviceKind, DeviceState},
    metadata, scanner,
};
use std::{fs, io, path::Path};
use test_support::Fixture;

fn assert_io(error: &Error, path: &Path) {
    assert_eq!(error.path.as_deref(), Some(path));
    assert!(matches!(error.kind, ErrorKind::Io(_)));
    assert!(std::error::Error::source(error).is_some());
}

#[test]
fn missing_and_non_directory_roots_return_errors() {
    let fixture = Fixture::new();
    let missing = fixture.root.join("missing");
    let error = scanner::scan_library(&missing).unwrap_err();
    assert_io(&error, &missing);
    assert!(matches!(error.kind, ErrorKind::Io(e) if e.kind() == io::ErrorKind::NotFound));
    let file = fixture.library.join("metadata.db");
    assert_io(&scanner::scan_library(&file).unwrap_err(), &file);
    assert_eq!(
        scanner::scan_library(&fixture.library).unwrap().books.len(),
        2
    );
}

#[test]
fn empty_directory_is_success_but_unreadable_directory_is_error() {
    let fixture = Fixture::new();
    let empty = fixture.root.join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(scanner::scan_library(&empty).unwrap().books.is_empty());
    assert!(scanner::scan_files(&empty).unwrap().is_empty());
    assert!(converter::find_source_file(&scanner::scan_files(&empty).unwrap()).is_none());
    let missing = empty.join("missing");
    assert_io(&scanner::scan_files(&missing).unwrap_err(), &missing);
}

#[test]
fn config_distinguishes_absence_bad_toml_and_io_failure() {
    let fixture = Fixture::new();
    let path = fixture.root.join("config.toml");
    assert!(Config::load_from(&path).unwrap().is_none());
    fs::write(&path, "library_path = [").unwrap();
    let error = Config::load_from(&path).err().unwrap();
    assert_eq!(error.path.as_deref(), Some(path.as_path()));
    assert!(matches!(error.kind, ErrorKind::InvalidData(_)));
    assert_io(
        &Config::load_from(&fixture.root).err().unwrap(),
        &fixture.root,
    );
    let config = Config {
        library_path: fixture.library.clone(),
    };
    config.save_to(&path).unwrap();
    assert_eq!(
        Config::load_from(&path).unwrap().unwrap().library_path,
        config.library_path
    );
    assert_io(&config.save_to(&fixture.root).unwrap_err(), &fixture.root);
    assert_io(
        &config.save_to(&path.join("blocked.toml")).unwrap_err(),
        &path,
    );
}

#[test]
fn opf_distinguishes_absence_corruption_structure_and_io_failure() {
    let fixture = Fixture::new();
    let opf = fixture.book.join("metadata.opf");
    fs::remove_file(&opf).unwrap();
    assert!(metadata::parse_opf(&fixture.book).unwrap().is_none());
    for bytes in [b"<broken".as_slice(), b"<package/>".as_slice()] {
        fs::write(&opf, bytes).unwrap();
        let error = metadata::parse_opf(&fixture.book).unwrap_err();
        assert_eq!(error.path.as_deref(), Some(opf.as_path()));
        assert!(matches!(error.kind, ErrorKind::InvalidData(_)));
    }
    fs::write(&opf, [0xff]).unwrap();
    assert_io(&metadata::parse_opf(&fixture.book).unwrap_err(), &opf);
    fs::remove_file(&opf).unwrap();
    fs::create_dir(&opf).unwrap();
    assert_io(&metadata::parse_opf(&fixture.book).unwrap_err(), &opf);
}

#[test]
fn device_errors_retain_context_instead_of_strings() {
    let fixture = Fixture::new();
    let disconnected = DeviceState::default();
    assert!(matches!(
        disconnected
            .send_book(&fixture.book, "Author")
            .unwrap_err()
            .kind,
        ErrorKind::Unavailable
    ));
    let connected = DeviceState {
        connected: Some(test_support::device(DeviceKind::Kindle, &fixture.kindle)),
    };
    let missing = fixture.root.join("missing");
    assert_io(
        &rulibre_core::device::prepare_file(
            &missing,
            connected.connected.as_ref().unwrap(),
            Default::default(),
        )
        .unwrap_err(),
        &missing,
    );
    fs::remove_dir(fixture.kindle.join("documents")).unwrap();
    fs::write(fixture.kindle.join("documents"), b"blocked").unwrap();
    let error = connected.send_book(&fixture.book, "Author").unwrap_err();
    assert_eq!(error.path.as_deref(), Some(fixture.kindle.as_path()));
    assert!(matches!(error.kind, ErrorKind::InvalidData(_)));
}

#[test]
fn single_quotes_do_not_panic() {
    for input in ["\"", "'", "", " "] {
        let _ = config::sanitize_path(input);
    }
}

#[cfg(windows)]
#[test]
fn locked_opf_returns_access_error() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let opf = fixture.book.join("metadata.opf");
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&opf)
        .unwrap();
    let error = metadata::parse_opf(&fixture.book).unwrap_err();
    assert_io(&error, &opf);
    assert!(matches!(error.kind, ErrorKind::Io(e) if e.raw_os_error() == Some(32)));
}
