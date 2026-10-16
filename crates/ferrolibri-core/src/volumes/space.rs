//! Available bytes for the calling user, without creating a probe file.
use std::{io, path::Path};

/// Query the filesystem containing an existing directory. This is a snapshot,
/// not a reservation; transfers still have to handle an exhausted filesystem.
pub fn free_space(mount_point: &Path) -> Result<u64, crate::Error> {
    query_with(mount_point, platform_space)
}

fn query_with(
    path: &Path,
    query: impl FnOnce(&Path) -> io::Result<u64>,
) -> Result<u64, crate::Error> {
    let operation = "query available filesystem space";
    if path.as_os_str().is_empty() {
        return Err(crate::Error::io(
            operation,
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "directory path is empty"),
        ));
    }
    let metadata = std::fs::metadata(path).map_err(|e| crate::Error::io(operation, path, e))?;
    if !metadata.is_dir() {
        return Err(crate::Error::io(
            operation,
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "path is not a directory"),
        ));
    }
    query(path).map_err(|e| crate::Error::io(operation, path, e))
}

#[cfg(windows)]
fn platform_space(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"));
    }
    wide.push(0);
    let mut available = 0;
    // SAFETY: the input is NUL-terminated, the output is a writable u64 and
    // both remain alive for the call. Optional outputs can be null.
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(available)
    }
}

#[cfg(unix)]
fn platform_space(path: &Path) -> io::Result<u64> {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is a valid C string and stats points to writable storage.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful statvfs call initializes the output structure.
    let stats = unsafe { stats.assume_init() };
    let bytes = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
    u64::try_from(bytes).map_err(|_| io::Error::other("available space exceeds u64"))
}

#[cfg(not(any(windows, unix)))]
fn platform_space(_: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "filesystem space queries are unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorKind, test_support::Fixture};

    #[test]
    fn existing_unicode_directory_is_queried_without_writes() {
        let fixture = Fixture::new();
        let before = crate::test_support::snapshot(&fixture.library);
        free_space(&fixture.book).unwrap(); // Zero available bytes is valid.
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);
    }

    #[test]
    fn missing_file_empty_and_nul_paths_are_rejected() {
        let fixture = Fixture::new();
        for path in [
            fixture.root.join("missing"),
            fixture.library.join("metadata.db"),
            Path::new("").to_owned(),
            Path::new("bad\0path").to_owned(),
        ] {
            let error = free_space(&path).unwrap_err();
            assert_eq!(error.path.as_deref(), Some(path.as_path()));
            assert!(matches!(error.kind, ErrorKind::Io(_)));
        }
    }

    #[test]
    fn mock_os_failure_keeps_kind_and_path_and_zero_is_valid() {
        let fixture = Fixture::new();
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::NotFound] {
            let error = query_with(&fixture.kobo, |_| Err(io::Error::from(kind))).unwrap_err();
            assert_eq!(error.path.as_deref(), Some(fixture.kobo.as_path()));
            assert!(matches!(error.kind, ErrorKind::Io(e) if e.kind() == kind));
        }
        assert_eq!(query_with(&fixture.kobo, |_| Ok(0)).unwrap(), 0);
        assert_eq!(
            query_with(&fixture.kobo, |_| Ok(u64::MAX)).unwrap(),
            u64::MAX
        );
    }
}
