//! Lazy, metadata-only discovery of Calibre covers.
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::Error;

/// A cover location and the metadata needed to invalidate an image cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub len: u64,
}

/// Find `cover.jpg` (ASCII case-insensitive) without reading its contents.
///
/// Directories and links resolving outside the book directory are ignored.
/// Internal links are followed. I/O failures, including broken links, retain
/// their path context. Multiple matching names are considered in native path
/// order, so discovery does not depend on directory enumeration order.
pub fn find_cover(book_dir: &Path) -> Result<Option<CoverFile>, Error> {
    find_cover_with_metadata(book_dir, |path| fs::metadata(path))
}

fn find_cover_with_metadata(
    book_dir: &Path,
    mut metadata: impl FnMut(&Path) -> io::Result<fs::Metadata>,
) -> Result<Option<CoverFile>, Error> {
    let entries =
        fs::read_dir(book_dir).map_err(|e| Error::io("read cover directory", book_dir, e))?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("read cover directory entry", book_dir, e))?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("cover.jpg"))
        {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    if candidates.is_empty() {
        return Ok(None);
    }
    let book_root = fs::canonicalize(book_dir)
        .map_err(|e| Error::io("resolve cover directory", book_dir, e))?;
    for path in candidates {
        let resolved =
            fs::canonicalize(&path).map_err(|e| Error::io("resolve cover path", &path, e))?;
        if !resolved.starts_with(&book_root) {
            continue;
        }
        let metadata =
            metadata(&resolved).map_err(|e| Error::io("read cover metadata", &path, e))?;
        if metadata.is_file() {
            return Ok(Some(CoverFile {
                modified: metadata
                    .modified()
                    .map_err(|e| Error::io("read cover modification time", &path, e))?,
                len: metadata.len(),
                // Use the checked target, not the original link, for later decoding.
                path: resolved,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ErrorKind,
        test_support::{Fixture, snapshot},
    };

    #[test]
    fn present_cover_metadata_and_library_remain_unchanged() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.library);
        let path = fixture.book.join("cover.jpg");
        let cover = find_cover(&fixture.book).unwrap().unwrap();
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(cover.path, fs::canonicalize(path).unwrap());
        assert_eq!(cover.len, metadata.len());
        assert_eq!(cover.modified, metadata.modified().unwrap());
        assert_eq!(before, snapshot(&fixture.library));
    }

    #[test]
    fn missing_directory_cover_and_unrelated_names() {
        let fixture = Fixture::new();
        let path = fixture.book.join("cover.jpg");
        fs::remove_file(&path).unwrap();
        fs::write(fixture.book.join("cover.jpeg"), b"ignored").unwrap();
        assert_eq!(find_cover(&fixture.book).unwrap(), None);
        fs::create_dir(path).unwrap();
        assert_eq!(find_cover(&fixture.book).unwrap(), None);
    }

    #[test]
    fn mixed_case_name_is_found() {
        let fixture = Fixture::new();
        let path = fixture.book.join("CoVeR.JpG");
        fs::remove_file(fixture.book.join("cover.jpg")).unwrap();
        fs::write(&path, b"not an image: discovery does not decode").unwrap();
        assert_eq!(
            find_cover(&fixture.book).unwrap().unwrap().path,
            fs::canonicalize(path).unwrap()
        );
    }

    #[test]
    fn large_cover_only_needs_metadata() {
        let fixture = Fixture::new();
        let path = fixture.book.join("cover.jpg");
        let file = fs::File::create(path).unwrap();
        let len = 512 * 1024 * 1024;
        file.set_len(len).unwrap();
        assert_eq!(find_cover(&fixture.book).unwrap().unwrap().len, len);
    }

    #[test]
    fn metadata_error_keeps_candidate_path() {
        let fixture = Fixture::new();
        let error = find_cover_with_metadata(&fixture.book, |_| {
            Err(io::ErrorKind::PermissionDenied.into())
        })
        .unwrap_err();
        assert_eq!(error.path, Some(fixture.book.join("cover.jpg")));
        assert!(matches!(error.kind, ErrorKind::Io(e)
            if e.kind() == io::ErrorKind::PermissionDenied));
    }

    #[test]
    fn missing_book_directory_is_an_error() {
        let fixture = Fixture::new();
        let path = fixture.book.join("missing");
        let error = find_cover(&path).unwrap_err();
        assert_eq!(error.path, Some(path));
        assert!(matches!(error.kind, ErrorKind::Io(e)
            if e.kind() == io::ErrorKind::NotFound));
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_contents_do_not_prevent_metadata_discovery() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let path = fixture.book.join("cover.jpg");
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(fs::read(&path).is_err());
        assert!(find_cover(&fixture.book).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_confined_to_book_directory() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let path = fixture.book.join("cover.jpg");
        fs::remove_file(&path).unwrap();
        let outside = fixture.root.join("outside.jpg");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, &path).unwrap();
        assert_eq!(find_cover(&fixture.book).unwrap(), None);
        fs::remove_file(&path).unwrap();
        let inside = fixture.book.join("inside.jpg");
        fs::write(&inside, b"inside").unwrap();
        symlink(&inside, &path).unwrap();
        assert_eq!(
            find_cover(&fixture.book).unwrap().unwrap().path,
            fs::canonicalize(&inside).unwrap()
        );
        fs::remove_file(inside).unwrap();
        assert!(matches!(find_cover(&fixture.book).unwrap_err().kind,
            ErrorKind::Io(e) if e.kind() == io::ErrorKind::NotFound));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_contents_do_not_prevent_metadata_discovery() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let path = fixture.book.join("cover.jpg");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = find_cover(&fixture.book);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(result.unwrap().is_some());
    }
}
