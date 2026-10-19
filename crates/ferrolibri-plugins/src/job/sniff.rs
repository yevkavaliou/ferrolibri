//! Decides from the bytes of a downloaded file whether it is the book that was promised.
//!
//! `Content-Type` is not looked at: catalogs send books as `application/zip` and errors as
//! `text/html`, and some send an error page with status 200 (seen with real OPDS catalogs).
//! Nothing is unpacked; a ZIP is judged by its table of contents.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use ferrolibri_core::book::Format;

use super::zipdir::{self, ZipError};
use crate::Archive;

/// How much of the beginning is looked at for text formats.
const HEAD_BYTES: usize = 64 * 1024;
/// Where an HTML page announces itself.
const HTML_WINDOW: usize = 1024;
const MOBI_MAGIC_OFFSET: usize = 60;
const MOBI_MAGIC: &[u8] = b"BOOKMOBI";

#[derive(Debug)]
pub enum Verdict {
    Book,
    /// Not what the offer promised; the text says what arrived instead ("HTML, 150 bytes").
    NotBook(String),
}

/// Checks `path` against what the offer promised: `format`, wrapped in `archive` if it says so.
/// `max_unpacked` caps the size an archive may claim to unpack to.
pub fn check(
    path: &Path,
    format: Format,
    archive: Option<Archive>,
    max_unpacked: u64,
) -> io::Result<Verdict> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = Vec::new();
    file.by_ref()
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)?;

    let accepted = match (format, archive) {
        (Format::Fb2, Some(Archive::Zip)) => match zip_contents(&mut file, &head)? {
            Ok(entries) => single_fb2(&entries, max_unpacked),
            Err(()) => false,
        },
        (Format::Epub, _) => has_entry(&mut file, &head, b"mimetype")?,
        (Format::Docx, _) => has_entry(&mut file, &head, b"[Content_Types].xml")?,
        (Format::Pdf, _) => head.starts_with(b"%PDF"),
        (Format::Mobi | Format::Azw3, _) => {
            head.get(MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + MOBI_MAGIC.len()) == Some(MOBI_MAGIC)
        }
        (Format::Fb2, None) => !looks_like_html(&head) && contains(&head, b"<FictionBook"),
        (Format::Txt, _) => !looks_like_html(&head),
        // An offer is never KEPUB (the contract refuses it); nothing here can vouch for one.
        (Format::Kepub, _) => false,
    };
    Ok(if accepted {
        Verdict::Book
    } else {
        Verdict::NotBook(describe(&head, size))
    })
}

/// The table of contents of the archive in `file`, or `Err(())` if it is not a usable ZIP.
fn zip_contents(file: &mut File, head: &[u8]) -> io::Result<Result<Vec<zipdir::Entry>, ()>> {
    if !head.starts_with(b"PK\x03\x04") {
        return Ok(Err(()));
    }
    match zipdir::read_directory(file) {
        Ok(entries) => Ok(Ok(entries)),
        Err(ZipError::Io(error)) => Err(error),
        Err(_) => Ok(Err(())),
    }
}

fn has_entry(file: &mut File, head: &[u8], name: &[u8]) -> io::Result<bool> {
    Ok(zip_contents(file, head)?.is_ok_and(|entries| entries.iter().any(|e| e.name == name)))
}

/// Exactly one file, an `.fb2`, and the unpacked size within the limit. Directory entries
/// are not files; a name that climbs out of its folder is not accepted even though nothing is
/// unpacked here.
fn single_fb2(entries: &[zipdir::Entry], max_unpacked: u64) -> bool {
    let mut files = entries.iter().filter(|e| !e.is_directory());
    let (Some(file), None) = (files.next(), files.next()) else {
        return false;
    };
    let name = file.name.as_slice();
    file.has_extension("fb2")
        && file.size <= max_unpacked
        && !name.starts_with(b"/")
        && !name.starts_with(b"\\")
        && !name.iter().any(|b| *b == 0 || *b == b'\\')
        && name.split(|b| *b == b'/').all(|part| part != b"..")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// An HTML page: it starts like one, or names `<html`, `<head` or `<body` near the top.
fn looks_like_html(head: &[u8]) -> bool {
    let text = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let trimmed = text.trim_ascii_start();
    let starts = |prefix: &[u8]| {
        trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix)
    };
    if starts(b"<!doctype html") || starts(b"<html") {
        return true;
    }
    let window = &text[..text.len().min(HTML_WINDOW)];
    [&b"<html"[..], b"<head", b"<body"]
        .iter()
        .any(|tag| contains_ignore_case(window, tag))
}

/// What the bytes are, for the message "the site sent HTML, 150 bytes".
fn describe(head: &[u8], size: u64) -> String {
    let kind = if looks_like_html(head) {
        "HTML"
    } else if head.starts_with(b"PK\x03\x04") {
        "a ZIP archive that is not the book promised"
    } else if head.starts_with(b"%PDF") {
        "a PDF"
    } else if head.trim_ascii_start().starts_with(b"<?xml") {
        "XML"
    } else if head.is_empty() {
        "an empty answer"
    } else if head.iter().take(512).all(|b| !matches!(b, 0..=8 | 14..=31)) {
        "text"
    } else {
        "data of another kind"
    };
    format!("{kind}, {size} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{single_file_zip, zip_declaring, zip_stored};

    fn verdict(bytes: &[u8], format: Format, archive: Option<Archive>) -> Verdict {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book");
        std::fs::write(&path, bytes).unwrap();
        check(&path, format, archive, 1000).unwrap()
    }

    fn accepted(bytes: &[u8], format: Format, archive: Option<Archive>) -> bool {
        matches!(verdict(bytes, format, archive), Verdict::Book)
    }

    fn found(bytes: &[u8], format: Format, archive: Option<Archive>) -> String {
        match verdict(bytes, format, archive) {
            Verdict::NotBook(found) => found,
            Verdict::Book => panic!("accepted"),
        }
    }

    const HTML: &[u8] =
        b"<html><head><title>502 Bad Gateway</title></head><body>nginx</body></html>";

    #[test]
    fn an_html_page_is_not_a_book_in_any_format() {
        for format in [
            Format::Epub,
            Format::Pdf,
            Format::Mobi,
            Format::Azw3,
            Format::Fb2,
            Format::Txt,
            Format::Docx,
        ] {
            assert!(!accepted(HTML, format, None), "{format:?}");
        }
        assert!(!accepted(HTML, Format::Fb2, Some(Archive::Zip)));
        assert_eq!(
            found(HTML, Format::Epub, None),
            format!("HTML, {} bytes", HTML.len())
        );
    }

    #[test]
    fn html_is_recognised_whatever_its_capitalisation_bom_or_leading_blanks() {
        for page in [
            &b"<!DOCTYPE html><p>no</p>"[..],
            b"\xEF\xBB\xBF  \r\n<HTML>",
            b"<?xml version=\"1.0\"?><!-- c --><html>",
            b"\n\n<BODY onload=x>",
        ] {
            assert!(
                !accepted(page, Format::Txt, None),
                "{:?}",
                String::from_utf8_lossy(page)
            );
        }
        assert!(accepted("Глава 1\n\nТекст.".as_bytes(), Format::Txt, None));
        assert!(accepted(b"", Format::Txt, None)); // an empty TXT is not HTML; Calibre decides
    }

    #[test]
    fn epub_needs_a_zip_with_a_mimetype_entry() {
        let epub = zip_stored(&[
            ("mimetype", b"application/epub+zip"),
            ("OEBPS/c.xhtml", b"x"),
        ]);
        assert!(accepted(&epub, Format::Epub, None));
        // A zip without the entry (what a catalog sends for a book in another packaging).
        let other = zip_stored(&[("book.fb2", b"<FictionBook/>")]);
        assert!(!accepted(&other, Format::Epub, None));
        assert!(found(&other, Format::Epub, None).starts_with("a ZIP archive"));
        // The signature alone is not enough, nor is a cut-off archive.
        assert!(!accepted(
            b"PK\x03\x04 and nothing else",
            Format::Epub,
            None
        ));
        assert!(!accepted(&epub[..epub.len() - 10], Format::Epub, None));
    }

    #[test]
    fn docx_needs_its_content_types_entry() {
        let docx = zip_stored(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("word/document.xml", b"x"),
        ]);
        assert!(accepted(&docx, Format::Docx, None));
        assert!(!accepted(
            &zip_stored(&[("a.txt", b"x")]),
            Format::Docx,
            None
        ));
    }

    #[test]
    fn pdf_and_mobi_are_recognised_by_their_magic() {
        assert!(accepted(b"%PDF-1.7\n%...", Format::Pdf, None));
        assert!(!accepted(b" %PDF-1.7", Format::Pdf, None));
        let mut mobi = vec![0u8; 80];
        mobi[60..68].copy_from_slice(b"BOOKMOBI");
        assert!(accepted(&mobi, Format::Mobi, None));
        assert!(accepted(&mobi, Format::Azw3, None));
        assert!(!accepted(&mobi[..64], Format::Mobi, None));
        assert!(!accepted(&[0u8; 80], Format::Azw3, None));
    }

    #[test]
    fn fb2_must_be_xml_with_a_fiction_book_root() {
        let fb2 = b"<?xml version=\"1.0\" encoding=\"windows-1251\"?>\n<FictionBook xmlns=\"x\"></FictionBook>";
        assert!(accepted(fb2, Format::Fb2, None));
        assert!(!accepted(
            b"<?xml version=\"1.0\"?><feed/>",
            Format::Fb2,
            None
        ));
        assert!(!accepted(
            b"<html><!-- <FictionBook --></html>",
            Format::Fb2,
            None
        ));
    }

    #[test]
    fn a_zipped_fb2_is_one_fb2_file_and_nothing_else() {
        let good = single_file_zip("book.fb2", b"<FictionBook/>");
        assert!(accepted(&good, Format::Fb2, Some(Archive::Zip)));
        assert!(accepted(
            &zip_stored(&[("BOOK.FB2", b"x")]),
            Format::Fb2,
            Some(Archive::Zip)
        ));
        // No fb2, two files, an fb2 and something else: refused.
        for archive in [
            zip_stored(&[("book.txt", b"x")]),
            zip_stored(&[("a.fb2", b"x"), ("b.fb2", b"y")]),
            zip_stored(&[("a.fb2", b"x"), ("cover.jpg", b"y")]),
            zip_stored(&[]),
        ] {
            assert!(!accepted(&archive, Format::Fb2, Some(Archive::Zip)));
        }
        // A folder entry beside the file does not count as a file.
        assert!(accepted(
            &zip_stored(&[("books/", b""), ("books/a.fb2", b"x")]),
            Format::Fb2,
            Some(Archive::Zip)
        ));
    }

    #[test]
    fn a_zipped_fb2_must_not_unpack_beyond_the_limit() {
        // The limit in these tests is 1000 bytes.
        assert!(accepted(
            &zip_declaring(&[("a.fb2", 1000)]),
            Format::Fb2,
            Some(Archive::Zip)
        ));
        assert!(!accepted(
            &zip_declaring(&[("a.fb2", 1001)]),
            Format::Fb2,
            Some(Archive::Zip)
        ));
        assert!(!accepted(
            &zip_declaring(&[("a.fb2", u32::MAX - 1)]),
            Format::Fb2,
            Some(Archive::Zip)
        ));
    }

    #[test]
    fn a_zipped_fb2_whose_name_climbs_out_of_its_folder_is_refused() {
        for name in ["../a.fb2", "x/../../a.fb2", "/etc/a.fb2", "..\\a.fb2"] {
            assert!(
                !accepted(
                    &single_file_zip(name, b"x"),
                    Format::Fb2,
                    Some(Archive::Zip)
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn a_zipped_fb2_that_is_really_html_says_what_it_is() {
        assert_eq!(
            found(HTML, Format::Fb2, Some(Archive::Zip)),
            format!("HTML, {} bytes", HTML.len())
        );
        assert_eq!(found(b"", Format::Pdf, None), "an empty answer, 0 bytes");
        assert_eq!(
            found(b"\x00\x01\x02", Format::Pdf, None),
            "data of another kind, 3 bytes"
        );
        assert_eq!(found(b"just words", Format::Pdf, None), "text, 10 bytes");
    }

    #[test]
    fn an_archive_with_no_end_record_or_with_zip64_markers_is_not_read() {
        let mut zip64 = single_file_zip("a.fb2", b"x");
        let end = zip64.len() - 22;
        zip64[end + 10..end + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(!accepted(&zip64, Format::Fb2, Some(Archive::Zip)));
        assert!(!accepted(&zip64[..30], Format::Fb2, Some(Archive::Zip)));
    }
}
