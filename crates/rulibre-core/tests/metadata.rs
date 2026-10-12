#[path = "../src/test_support.rs"]
mod test_support;

use rulibre_core::{ErrorKind, metadata, scanner};
use std::fs;
use test_support::Fixture;

fn opf(fixture: &Fixture, fields: &str) {
    fs::write(fixture.book.join("metadata.opf"), format!(
        r#"<o:package xmlns:o="http://www.idpf.org/2007/opf"><o:metadata xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:x="urn:custom">{fields}</o:metadata></o:package>"#
    )).unwrap();
}

#[test]
fn namespaced_fields_multiple_authors_series_identifiers_and_diagnostics() {
    let fixture = Fixture::new();
    opf(
        &fixture,
        r#"
        <d:title>  OPF title  </d:title><d:title>Alternate title</d:title>
        <d:creator> One </d:creator><d:creator> </d:creator><d:creator>Два</d:creator>
        <d:date>2020-01-02T12:30:00Z</d:date>
        <d:identifier o:scheme="ISBN"> 978123 </d:identifier>
        <d:identifier>urn:test:123</d:identifier><d:identifier o:scheme="uuid">internal</d:identifier>
        <o:meta name="calibre:series" content="Series"/>
        <o:meta name="calibre:series_index" content="2.5"/>
        <o:meta name="calibre:rating" content="8"/>
        <x:odd>value</x:odd><x:meta name="calibre:series" content="wrong"/>
        <o:meta name="custom" content="value"/><o:meta property="test">data</o:meta>
    "#,
    );
    let meta = metadata::parse_opf(&fixture.book).unwrap().unwrap();
    assert_eq!(meta.title, "OPF title");
    assert_eq!(meta.authors, ["One", "Два"]);
    assert_eq!(meta.date, "2020-01-02");
    assert_eq!(meta.series, "Series");
    assert_eq!(meta.series_index, "2.5");
    assert_eq!(meta.rating, "8");
    assert_eq!(
        meta.identifiers,
        [
            ("ISBN".into(), "978123".into()),
            ("ID".into(), "urn:test:123".into())
        ]
    );
    assert_eq!(
        meta.unrecognized,
        [
            "{urn:custom}odd",
            "{urn:custom}meta",
            "meta[custom] = value",
            "meta[property=test] = data"
        ]
    );
}

#[test]
fn missing_and_empty_fields_use_directory_fallback_independently() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.book.join("metadata.opf")).unwrap();
    assert!(metadata::parse_opf(&fixture.book).unwrap().is_none());
    let meta = metadata::read_metadata(&fixture.book).unwrap();
    assert_eq!(meta.title, "A small book");
    assert_eq!(meta.authors, ["Автор Example"]);
    for (fields, title, authors) in [
        (
            "<d:title> </d:title><d:creator> </d:creator>",
            "A small book",
            vec!["Автор Example"],
        ),
        ("<d:title>Title</d:title>", "Title", vec!["Автор Example"]),
        (
            "<d:creator>Author</d:creator>",
            "A small book",
            vec!["Author"],
        ),
    ] {
        opf(&fixture, fields);
        let meta = metadata::read_metadata(&fixture.book).unwrap();
        assert_eq!(meta.title, title);
        assert_eq!(meta.authors, authors);
    }
}

#[test]
fn malformed_metadata_warns_without_losing_books_or_identity() {
    let fixture = Fixture::new();
    let original = scanner::scan_library(&fixture.library).unwrap();
    let original = original
        .books
        .iter()
        .find(|b| b.path == fixture.book)
        .unwrap();
    for xml in [
        "<broken",
        "<package/>",
        "<package xmlns:x='urn:wrong'><x:metadata/></package>",
    ] {
        fs::write(fixture.book.join("metadata.opf"), xml).unwrap();
        assert!(metadata::read_metadata(&fixture.book).is_err());
        let report = scanner::scan_library(&fixture.library).unwrap();
        assert_eq!(report.books.len(), 2);
        let book = report
            .books
            .iter()
            .find(|b| b.path == fixture.book)
            .unwrap();
        assert_eq!(book.id, original.id);
        assert_eq!(book.files, original.files);
        assert_eq!(book.title, "A small book");
        assert_eq!(book.author, "Автор Example");
        assert_eq!(report.warnings.len(), 1);
        assert!(matches!(report.warnings[0].kind, ErrorKind::InvalidData(_)));
        assert_eq!(
            report.warnings[0].path.as_deref(),
            Some(fixture.book.join("metadata.opf").as_path())
        );
    }
}

#[test]
fn scanner_prefers_opf_names_and_keeps_sources_unchanged() {
    let fixture = Fixture::new();
    opf(
        &fixture,
        "<d:title>New title</d:title><d:creator>One</d:creator><d:creator>Two</d:creator>",
    );
    let path = fixture.book.join("metadata.opf");
    let before = fs::read(&path).unwrap();
    let report = scanner::scan_library(&fixture.library).unwrap();
    assert!(report.warnings.is_empty());
    let book = report
        .books
        .iter()
        .find(|b| b.path == fixture.book)
        .unwrap();
    assert_eq!(book.title, "New title");
    assert_eq!(book.author, "One, Two");
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn html_descriptions_preserve_boundaries_and_decode_entities_once() {
    let fixture = Fixture::new();
    for (html, expected) in [
        (
            "<p>First <b>bold</b>.</p><p>Second<br>line</p>",
            "First bold.\n\nSecond\nline",
        ),
        (
            "<div>A&nbsp;&copy;&mdash;&#169;&#x1F642; &amp;lt;</div>",
            "A ©—©🙂 &lt;",
        ),
        (
            "<p title='a > b'>A</p><!-- hide --><script>bad()</script><style>bad</style><p>B",
            "A\n\nB",
        ),
        ("<ul><li>One</li><li>Two</li></ul>", "One\nTwo"),
        ("A<br><br>B", "A\n\nB"),
        ("  a\n  b <i>c</i> d  ", "a b c d"),
        ("2 < 3 &unknown;", "2 < 3 &unknown;"),
    ] {
        opf(
            &fixture,
            &format!("<d:description><![CDATA[{html}]]></d:description>"),
        );
        assert_eq!(
            metadata::parse_opf(&fixture.book)
                .unwrap()
                .unwrap()
                .description,
            expected,
            "{html}"
        );
    }
    opf(
        &fixture,
        "<d:description>&lt;p&gt;A &amp;amp; B&lt;/p&gt;&lt;p&gt;C&lt;/p&gt;</d:description>",
    );
    assert_eq!(
        metadata::parse_opf(&fixture.book)
            .unwrap()
            .unwrap()
            .description,
        "A & B\n\nC"
    );
    opf(
        &fixture,
        "<d:description><p xmlns='http://www.w3.org/1999/xhtml'>A &amp; B</p><p>C<br/>D</p></d:description>",
    );
    assert_eq!(
        metadata::parse_opf(&fixture.book)
            .unwrap()
            .unwrap()
            .description,
        "A & B\n\nC\nD"
    );
}

#[cfg(windows)]
#[test]
fn inaccessible_opf_is_warning_and_book_remains_visible() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fixture.book.join("metadata.opf"))
        .unwrap();
    let report = scanner::scan_library(&fixture.library).unwrap();
    assert_eq!(report.books.len(), 2);
    assert_eq!(report.warnings.len(), 1);
    assert!(matches!(report.warnings[0].kind, ErrorKind::Io(_)));
}
