#[path = "../src/test_support.rs"]
mod test_support;

use rulibre_core::{
    converter,
    device::{DeviceKind, DeviceState},
    metadata, scanner,
};
use test_support::Fixture;

#[test]
fn scans_synthetic_library_and_reads_opf() {
    let fixture = Fixture::new();
    let books = scanner::scan_library(&fixture.library).unwrap().books;
    assert_eq!(books.len(), 2);
    let book = books.iter().find(|book| book.path == fixture.book).unwrap();
    assert_eq!(book.title, "A small book");
    assert_eq!(book.author, "Автор Example, Second Author");
    assert_eq!(
        book.files
            .iter()
            .map(|f| f.format.label())
            .collect::<Vec<_>>(),
        ["EPUB", "KEPUB", "MOBI", "PDF"]
    );
    let metadata = metadata::parse_opf(&book.path).unwrap().unwrap();
    assert_eq!(metadata.title, "A small book");
    assert_eq!(metadata.authors, ["Автор Example", "Second Author"]);
    assert_eq!(metadata.series, "Fixture series");
    assert_eq!(
        metadata.identifiers,
        [("ISBN".into(), "9780000000000".into())]
    );
    assert_eq!(
        converter::find_source_file(&book.files).unwrap(),
        book.path.join("A small book.epub")
    );
}

#[test]
fn public_send_rejects_synthetic_directories_without_mounted_volume() {
    let fixture = Fixture::new();
    let before = test_support::snapshot(&fixture.library);
    for (kind, mount, filename, destination) in [
        (
            DeviceKind::Kobo,
            &fixture.kobo,
            "A small book.kepub.epub",
            fixture.kobo.join("Автор Example"),
        ),
        (
            DeviceKind::Kindle,
            &fixture.kindle,
            "A small book.mobi",
            fixture.kindle.join("documents"),
        ),
    ] {
        let state = DeviceState {
            connected: Some(test_support::device(kind, mount)),
        };
        let before_device = test_support::snapshot(mount);
        assert!(state.send_book(&fixture.book, "Автор Example").is_err());
        assert!(!destination.join(filename).exists());
        assert_eq!(test_support::snapshot(mount), before_device);
    }
    assert_eq!(test_support::snapshot(&fixture.library), before);
}

#[test]
fn temporary_fixture_is_removed_on_drop() {
    let root = {
        let fixture = Fixture::new();
        fixture.root.clone()
    };
    assert!(!root.exists());
}
