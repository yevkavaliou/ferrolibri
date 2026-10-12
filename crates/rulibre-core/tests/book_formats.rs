#[path = "../src/test_support.rs"]
mod test_support;

use rulibre_core::{
    book::{Book, BookFile, BookId, Format},
    converter,
    device::DeviceKind,
    scanner,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use test_support::Fixture;

#[test]
fn recognition_excludes_service_unknown_and_temporary_files() {
    for name in [
        "metadata.OPF",
        "cover.JPG",
        "cover.png",
        "a.jpeg",
        ".DS_Store",
        "metadata.db",
        "notes.xyz",
        "file",
        "book.epub.tmp",
        "book.pdf.part",
        "book.mobi.bak",
        ".hidden.epub",
        "~book.epub",
        ".epub",
        "book.kepub",
    ] {
        assert_eq!(scanner::extract_format(name), None, "{name}");
    }
    for (name, format) in [
        ("book.ePuB", Format::Epub),
        ("book.KEPUB.EpUb", Format::Kepub),
        ("book.PDF", Format::Pdf),
        ("book.MoBi", Format::Mobi),
        ("book.AZW3", Format::Azw3),
        ("book.DoCx", Format::Docx),
        ("book.tXT", Format::Txt),
    ] {
        assert_eq!(scanner::extract_format(name), Some(format), "{name}");
    }
}

#[test]
fn scan_preserves_duplicates_exact_paths_and_order_independent_of_creation() {
    let fixture = Fixture::new();
    let dir = fixture.root.join("formats");
    fs::create_dir(&dir).unwrap();
    let names = [
        "z.epub",
        "metadata.opf",
        "A.EPUB",
        "cover.JPG",
        "b.KEPUB.EPUB",
        "unknown.zip",
        "Б.pdf",
        "a.epub.tmp",
    ];
    for name in names {
        fs::write(dir.join(name), name).unwrap();
    }
    fs::create_dir(dir.join("directory.epub")).unwrap();
    let first = scanner::scan_files(&dir).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|f| (f.format, f.path.file_name().unwrap().to_str().unwrap()))
            .collect::<Vec<_>>(),
        [
            (Format::Epub, "A.EPUB"),
            (Format::Epub, "z.epub"),
            (Format::Kepub, "b.KEPUB.EPUB"),
            (Format::Pdf, "Б.pdf")
        ]
    );
    for name in names {
        fs::remove_file(dir.join(name)).unwrap();
    }
    for name in names.into_iter().rev() {
        fs::write(dir.join(name), name).unwrap();
    }
    assert_eq!(scanner::scan_files(&dir).unwrap(), first);
    let mut reversed = first.clone();
    reversed.reverse();
    assert_eq!(
        converter::find_source_file(&reversed),
        Some(dir.join("A.EPUB"))
    );
    // Selection uses the snapshot even when the directory is no longer readable.
    fs::rename(&dir, fixture.root.join("moved")).unwrap();
    assert_eq!(
        converter::find_source_file(&first),
        Some(dir.join("A.EPUB"))
    );
}

#[test]
fn recognized_converter_and_device_formats_are_separate() {
    let files = vec![BookFile {
        format: Format::Kepub,
        path: "only.kepub.epub".into(),
    }];
    assert!(converter::find_source_file(&files).is_none());
    assert!(targets(&files, true, true).is_empty());
    let epub = vec![BookFile {
        format: Format::Epub,
        path: "source.epub".into(),
    }];
    assert!(
        targets(&epub, true, false)
            .iter()
            .any(|(f, _)| *f == Format::Kepub)
    );
    assert!(
        !targets(&epub, false, true)
            .iter()
            .any(|(f, _)| *f == Format::Kepub)
    );
    assert!(targets(&[], true, true).is_empty());
    for (kind, expected) in [
        (DeviceKind::Kobo, vec![Format::Kepub, Format::Epub]),
        (DeviceKind::Kindle, vec![Format::Azw3, Format::Mobi]),
    ] {
        assert_eq!(
            test_support::device(kind, &PathBuf::new()).supported_formats(),
            expected
        );
    }
}

fn add_book(library: &Path, relative: &str) {
    let dir = library.join(relative);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("book.epub"), b"fixture").unwrap();
}

#[test]
fn ids_and_book_order_are_stable_and_scoped_to_library() {
    let fixture = Fixture::new();
    let library = fixture.root.join("identity");
    for relative in [
        "Z/Title (42)",
        "A/Title ()",
        "A/Title (0)",
        "A/Title (99999999999999999999999999)",
        "A/Title",
        "a/title",
        "A/Same (10)",
        "A/same (11)",
    ] {
        add_book(&library, relative);
    }
    let first = scanner::scan_library(&library).unwrap();
    assert!(first.warnings.is_empty());
    let identified = first
        .books
        .iter()
        .find(|b| b.id == BookId::Calibre(42))
        .unwrap();
    assert_eq!(identified.title, "Title");
    for book in first
        .books
        .iter()
        .filter(|b| matches!(b.id, BookId::RelativePath(_)))
    {
        assert_eq!(
            book.id,
            BookId::RelativePath(book.path.strip_prefix(&library).unwrap().to_owned())
        );
    }
    assert_eq!(scanner::scan_library(&library).unwrap().books, first.books);
    let moved = fixture.root.join("identity-moved");
    fs::rename(&library, &moved).unwrap();
    assert_eq!(
        scanner::scan_library(&moved)
            .unwrap()
            .books
            .iter()
            .map(|b| &b.id)
            .collect::<Vec<_>>(),
        first.books.iter().map(|b| &b.id).collect::<Vec<_>>()
    );
    fs::rename(moved.join("Z/Title (42)"), moved.join("Z/New title (42)")).unwrap();
    assert!(
        scanner::scan_library(&moved)
            .unwrap()
            .books
            .iter()
            .any(|b| b.id == BookId::Calibre(42))
    );
}

#[test]
fn duplicate_calibre_ids_warn_and_do_not_create_ambiguous_selection() {
    let fixture = Fixture::new();
    let library = fixture.root.join("duplicates");
    add_book(&library, "A/First (7)");
    add_book(&library, "B/Second (7)");
    let report = scanner::scan_library(&library).unwrap();
    assert_eq!(report.books.len(), 2);
    assert_eq!(report.warnings.len(), 2);
    assert_ne!(report.books[0].id, report.books[1].id);
    assert!(
        report
            .books
            .iter()
            .all(|b| matches!(b.id, BookId::RelativePath(_)))
    );
    assert!(
        report
            .warnings
            .iter()
            .all(|w| w.operation == "identify book" && w.path.is_some())
    );
}

#[test]
fn model_serialization_contains_typed_files_and_tagged_identity() {
    let book = Book {
        id: BookId::Calibre(7),
        author: "Автор".into(),
        title: "Title".into(),
        path: "Author/Title (7)".into(),
        files: vec![BookFile {
            format: Format::Kepub,
            path: "Author/Title (7)/book.KEPUB.EPUB".into(),
        }],
    };
    let value = serde_json::to_value(&book).unwrap();
    assert_eq!(
        value["id"],
        serde_json::json!({"kind": "calibre", "value": 7})
    );
    assert_eq!(value["files"][0]["format"], "KEPUB");
    assert!(value.get("formats").is_none());
    assert_eq!(serde_json::from_value::<Book>(value).unwrap(), book);
    let fallback = BookId::RelativePath("Автор/Без номера".into());
    assert_eq!(
        serde_json::from_str::<BookId>(&serde_json::to_string(&fallback).unwrap()).unwrap(),
        fallback
    );
}

#[cfg(unix)]
#[test]
fn native_non_unicode_filenames_keep_exact_paths() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let fixture = Fixture::new();
    let path = fixture
        .book
        .join(OsString::from_vec(b"native-\xff.EPUB".to_vec()));
    fs::write(&path, b"fixture").unwrap();
    assert!(
        scanner::scan_files(&fixture.book)
            .unwrap()
            .iter()
            .any(|f| f.path == path && f.format == Format::Epub)
    );
}

fn targets(files: &[BookFile], kepubify: bool, calibre: bool) -> Vec<(Format, String)> {
    use rulibre_core::converter::{Backend, BackendKind, BackendStatus};
    let tools = [
        (BackendKind::Kepubify, kepubify),
        (BackendKind::EbookConvert, calibre),
    ]
    .into_iter()
    .map(|(kind, ready)| Backend {
        kind,
        path: Some(kind.program().into()),
        version: Some("fixture".into()),
        status: if ready {
            BackendStatus::Ready
        } else {
            BackendStatus::Missing
        },
        diagnostic: String::new(),
        kepub_output: kind == BackendKind::Kepubify,
    })
    .collect::<Vec<_>>();
    converter::target_formats(files, &tools)
}
