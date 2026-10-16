use std::{
    fs,
    path::{Path, PathBuf},
};

pub use crate::book::{Book, BookFile, BookId, Format};

// Files at the library root are ignored automatically.
const SKIP_TOP_LEVEL_FOLDERS: &[&str] = &[".caltrash", ".calnotes", ".DS_Store", "downloaded"];

/// Keep every recognized regular file, ordered by format then exact native path.
pub fn scan_files(book_dir: &Path) -> Result<Vec<BookFile>, crate::Error> {
    let mut files = Vec::new();
    for path in crate::error::read_files(book_dir)? {
        if let Some(format) = extract_path_format(&path) {
            files.push(BookFile { format, path });
        }
    }
    files.sort_by(|a, b| a.format.cmp(&b.format).then_with(|| a.path.cmp(&b.path)));
    Ok(files)
}

/// Root failures are fatal; subtree failures are warnings and scanning continues.
#[derive(Debug)]
pub struct ScanReport {
    pub books: Vec<Book>,
    pub warnings: Vec<crate::Error>,
}

pub fn scan_library(library_path: &Path) -> Result<ScanReport, crate::Error> {
    scan_library_cancellable(
        library_path,
        &crate::converter::CancellationToken::default(),
    )
}

pub fn scan_library_cancellable(
    library_path: &Path,
    cancel: &crate::converter::CancellationToken,
) -> Result<ScanReport, crate::Error> {
    scan_library_inner(library_path, scan_files, cancel)
}

#[cfg(test)]
fn scan_library_with_files(
    library_path: &Path,
    files: impl FnMut(&Path) -> Result<Vec<BookFile>, crate::Error>,
) -> Result<ScanReport, crate::Error> {
    scan_library_inner(
        library_path,
        files,
        &crate::converter::CancellationToken::default(),
    )
}

fn check_cancel(cancel: &crate::converter::CancellationToken) -> Result<(), crate::Error> {
    if cancel.is_cancelled() {
        return Err(crate::Error {
            operation: "scan library",
            path: None,
            kind: crate::ErrorKind::Cancelled,
        });
    }
    Ok(())
}

fn scan_library_inner(
    library_path: &Path,
    mut files: impl FnMut(&Path) -> Result<Vec<BookFile>, crate::Error>,
    cancel: &crate::converter::CancellationToken,
) -> Result<ScanReport, crate::Error> {
    check_cancel(cancel)?;
    let authors = fs::read_dir(library_path)
        .map_err(|e| crate::Error::io("read library root", library_path, e))?;
    let mut report = ScanReport {
        books: Vec::new(),
        warnings: Vec::new(),
    };
    for entry in authors {
        check_cancel(cancel)?;
        let entry =
            entry.map_err(|e| crate::Error::io("read library root entry", library_path, e))?;
        let author_name = entry.file_name().to_string_lossy().into_owned();
        if SKIP_TOP_LEVEL_FOLDERS.contains(&author_name.as_str()) {
            continue;
        }
        match scan_author(&entry.path(), &author_name, &mut report, &mut files, cancel) {
            Ok(()) => {}
            Err(e) => report.warnings.push(e),
        }
    }
    check_cancel(cancel)?;
    report.books.sort_by(|a, b| {
        a.author
            .to_lowercase()
            .cmp(&b.author.to_lowercase())
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    // Malformed libraries may repeat Calibre IDs. Retain every book without
    // allowing an ambiguous selection; all duplicates receive path IDs.
    let mut counts = std::collections::BTreeMap::new();
    for book in &report.books {
        if let BookId::Calibre(id) = book.id {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    for book in &mut report.books {
        if let BookId::Calibre(id) = book.id
            && counts[&id] > 1
        {
            report.warnings.push(crate::Error::invalid_data(
                "identify book",
                &book.path,
                format!("duplicate Calibre ID {id}; using relative path"),
            ));
            book.id =
                BookId::RelativePath(book.path.strip_prefix(library_path).unwrap().to_owned());
        }
    }
    check_cancel(cancel)?;
    Ok(report)
}

fn scan_author(
    path: &Path,
    author: &str,
    report: &mut ScanReport,
    files: &mut impl FnMut(&Path) -> Result<Vec<BookFile>, crate::Error>,
    cancel: &crate::converter::CancellationToken,
) -> Result<(), crate::Error> {
    if !fs::metadata(path)
        .map_err(|e| crate::Error::io("read author metadata", path, e))?
        .is_dir()
    {
        return Ok(());
    }
    let entries =
        fs::read_dir(path).map_err(|e| crate::Error::io("read author directory", path, e))?;
    for entry in entries {
        check_cancel(cancel)?;
        let result = (|| {
            let entry = entry.map_err(|e| crate::Error::io("read book entry", path, e))?;
            let book_path = entry.path();
            if !fs::metadata(&book_path)
                .map_err(|e| crate::Error::io("read book metadata", &book_path, e))?
                .is_dir()
            {
                return Ok(());
            }
            let files = files(&book_path)?;
            if !files.is_empty() {
                let mut title = strip_calibre_id(&entry.file_name().to_string_lossy());
                let mut display_author = author.to_owned();
                match crate::metadata::read_metadata(&book_path) {
                    Ok(meta) => {
                        title = meta.title;
                        display_author = meta.authors.join(", ");
                    }
                    Err(e) => report.warnings.push(e),
                }
                report.books.push(Book {
                    id: calibre_id(&entry.file_name().to_string_lossy())
                        .map(BookId::Calibre)
                        .unwrap_or_else(|| {
                            BookId::RelativePath(
                                PathBuf::from(path.file_name().unwrap_or_default())
                                    .join(entry.file_name()),
                            )
                        }),
                    title,
                    author: display_author,
                    files,
                    path: book_path,
                });
            }
            Ok(())
        })();
        if let Err(e) = result {
            report.warnings.push(e);
        }
    }
    Ok(())
}

pub fn extract_format(filename: &str) -> Option<Format> {
    extract_path_format(Path::new(filename))
}

/// Inspect ASCII extensions without converting the native file path to text.
pub fn extract_path_format(path: &Path) -> Option<Format> {
    let name = path.file_name()?;
    if matches!(name.as_encoded_bytes().first(), Some(b'.' | b'~')) {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("epub") {
        let stem = Path::new(path.file_stem()?);
        if stem
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("kepub"))
        {
            return Some(Format::Kepub);
        }
    }
    if extension.eq_ignore_ascii_case("kepub") {
        return None;
    }
    Format::from_label(extension)
}

fn calibre_id(dir_name: &str) -> Option<u64> {
    let (_, suffix) = dir_name.rsplit_once(" (")?;
    let digits = suffix.strip_suffix(')')?;
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|id| *id > 0)
}

pub(crate) fn strip_calibre_id(dir_name: &str) -> String {
    if calibre_id(dir_name).is_some() {
        return dir_name.rsplit_once(" (").unwrap().0.to_owned();
    }
    dir_name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, ErrorKind, test_support::Fixture};
    use std::io;

    #[test]
    fn scan_cancellation_before_start_and_between_books_is_fatal() {
        let fixture = Fixture::new();
        let cancel = crate::converter::CancellationToken::default();
        cancel.cancel();
        assert!(matches!(
            scan_library_cancellable(&fixture.library, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        ));
        let cancel = crate::converter::CancellationToken::default();
        let mut calls = 0;
        let error = scan_library_inner(
            &fixture.library,
            |path| {
                calls += 1;
                cancel.cancel();
                scan_files(path)
            },
            &cancel,
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(matches!(error.kind, ErrorKind::Cancelled));
    }

    #[test]
    fn book_access_failure_is_warning_and_other_books_survive() {
        let fixture = Fixture::new();
        let report = scan_library_with_files(&fixture.library, |path| {
            if path == fixture.book {
                Err(Error::io(
                    "read directory",
                    path,
                    io::ErrorKind::PermissionDenied.into(),
                ))
            } else {
                scan_files(path)
            }
        })
        .unwrap();
        assert_eq!(report.books.len(), 1);
        assert_eq!(report.books[0].author, "Zed");
        assert_eq!(report.warnings.len(), 1);
        let warning = &report.warnings[0];
        assert_eq!(warning.path.as_deref(), Some(fixture.book.as_path()));
        assert!(
            matches!(&warning.kind, ErrorKind::Io(e) if e.kind() == io::ErrorKind::PermissionDenied)
        );
    }
}
