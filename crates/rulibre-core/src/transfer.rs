//! Cancellable device transfer with exclusive publication and explicit cleanup failures.
use crate::{
    Error, ErrorKind,
    converter::CancellationToken,
    device::{DetectionEvidence, Device, DeviceKind},
};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn invalid(path: &Path) -> Error {
    Error {
        operation: "validate transfer path",
        path: Some(path.into()),
        kind: ErrorKind::InvalidInput,
    }
}

/// Portable single component: keep Unicode, replace separators/control/Windows
/// reserved characters, trim trailing dots/spaces and escape DOS device names.
pub fn normalize_component(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:<>\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut value = value.trim().trim_end_matches(['.', ' ']).to_owned();
    if value.is_empty() {
        value = "Unknown".into();
    }
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    {
        value.insert(0, '_');
    }
    value
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error {
            operation: "transfer book",
            path: None,
            kind: ErrorKind::Cancelled,
        })
    } else {
        Ok(())
    }
}

/// Revalidate OS volume identity and reader evidence, including manual selection.
pub fn validate_device(device: &Device) -> Result<(), Error> {
    let report = crate::volumes::mounted_volumes()?;
    validate_device_from_volumes(device, &report.volumes)
}

fn validate_device_from_volumes(
    device: &Device,
    volumes: &[crate::volumes::MountedVolume],
) -> Result<(), Error> {
    let current = if device
        .evidence
        .contains(&DetectionEvidence::ManualSelection)
    {
        crate::device::manual_device_from_volumes(&device.mount_point, device.kind, volumes)?
    } else {
        crate::device::detect_devices_from_volumes(volumes)
            .devices
            .into_iter()
            .find(|d| d.id == device.id && d.mount_point == device.mount_point)
            .ok_or_else(|| {
                Error::unavailable("revalidate selected reader", Some(&device.mount_point))
            })?
    };
    if current.id != device.id
        || current.kind != device.kind
        || current.mount_point != device.mount_point
        || current.filesystem != device.filesystem
    {
        return Err(Error::unavailable(
            "selected reader changed",
            Some(&device.mount_point),
        ));
    }
    if current.read_only || device.read_only {
        return Err(Error::io(
            "write reader",
            &device.mount_point,
            io::Error::from(io::ErrorKind::PermissionDenied),
        ));
    }
    Ok(())
}

// Directory descriptors anchor Unix mutations even after rename/unmount. On
// Windows, handles without FILE_SHARE_DELETE prevent directory/link replacement.
struct Directory {
    file: File,
    path: PathBuf,
}

impl Directory {
    fn ensure_current(&self) -> Result<(), Error> {
        let current = fs::symlink_metadata(&self.path)
            .map_err(|e| Error::io("revalidate transfer directory", &self.path, e))?;
        if !current.is_dir() || current.file_type().is_symlink() {
            return Err(invalid(&self.path));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let original = self
                .file
                .metadata()
                .map_err(|e| Error::io("revalidate directory handle", &self.path, e))?;
            if (current.dev(), current.ino()) != (original.dev(), original.ino()) {
                return Err(invalid(&self.path));
            }
        }
        #[cfg(windows)]
        {
            let _ = &self.file;
        } // Holding this handle prevents deletion/replacement.
        Ok(())
    }
    fn open(path: &Path) -> Result<Self, Error> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|e| Error::io("inspect transfer directory", path, e))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid(path));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Err(invalid(path));
            }
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(3).custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
                    | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let file = options
            .open(path)
            .map_err(|e| Error::io("open transfer directory", path, e))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let opened = file
                .metadata()
                .map_err(|e| Error::io("inspect directory handle", path, e))?;
            if !opened.is_dir()
                || opened.file_attributes()
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                    != 0
            {
                return Err(invalid(path));
            }
        }
        Ok(Self {
            file,
            path: path.into(),
        })
    }

    fn child(&self, name: &str) -> Result<Self, Error> {
        let path = self.path.join(name);
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = c_name(OsStr::new(name))?;
            // SAFETY: live directory descriptor and NUL-terminated component.
            let result = unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o755) };
            if result != 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(Error::io(
                    "create reader directory",
                    &path,
                    io::Error::last_os_error(),
                ));
            }
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(Error::io(
                    "open reader directory",
                    &path,
                    io::Error::last_os_error(),
                ));
            }
            let file = unsafe { File::from_raw_fd(fd) };
            use std::os::unix::fs::MetadataExt;
            if file
                .metadata()
                .map_err(|e| Error::io("inspect reader directory", &path, e))?
                .dev()
                != self
                    .file
                    .metadata()
                    .map_err(|e| Error::io("inspect reader root", &self.path, e))?
                    .dev()
            {
                return Err(invalid(&path));
            }
            Ok(Self { file, path })
        }
        #[cfg(not(unix))]
        {
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Error::io("create reader directory", &path, e)),
            }
            let child = Self::open(&path)?;
            if !fs::canonicalize(&path)
                .map_err(|e| Error::io("resolve reader directory", &path, e))?
                .starts_with(&self.path)
            {
                return Err(invalid(&path));
            }
            Ok(child)
        }
    }

    fn create(&self, name: &OsStr) -> io::Result<File> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = c_name_io(name)?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { File::from_raw_fd(fd) })
            }
        }
        #[cfg(not(unix))]
        {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.path.join(name))
        }
    }

    fn remove(&self, name: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = c_name_io(name)?;
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(unix))]
        {
            fs::remove_file(self.path.join(name))
        }
    }

    fn publish(&self, temp: &OsStr, dest: &OsStr) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let from = c_name_io(temp)?;
            let to = c_name_io(dest)?;
            let fd = self.file.as_raw_fd();
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_NOREPLACE)
            };
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_EXCL)
            };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            return Err(io::Error::from(io::ErrorKind::Unsupported));
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let from: Vec<_> = self
                .path
                .join(temp)
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            let to: Vec<_> = self
                .path
                .join(dest)
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            // No REPLACE_EXISTING or COPY_ALLOWED: same-directory exclusive move,
            // also works on FAT/exFAT without requiring hard links.
            if unsafe {
                windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
            } != 0
            {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }
}

#[cfg(unix)]
fn c_name_io(name: &OsStr) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}
#[cfg(unix)]
fn c_name(name: &OsStr) -> Result<std::ffi::CString, Error> {
    c_name_io(name).map_err(|_| invalid(Path::new(name)))
}

/// Copy a prepared regular file. Progress reports actual bytes after each chunk.
/// The caller must retain PreparedFile until this call finishes.
pub fn transfer_file(
    file: &Path,
    device: &Device,
    author: &str,
    cancel: &CancellationToken,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, Error> {
    transfer_with(
        file,
        device,
        author,
        cancel,
        progress,
        validate_device,
        |out, bytes| out.write_all(bytes),
    )
}

fn transfer_with(
    file: &Path,
    device: &Device,
    author: &str,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
    mut validate: impl FnMut(&Device) -> Result<(), Error>,
    mut write: impl FnMut(&mut File, &[u8]) -> io::Result<()>,
) -> Result<PathBuf, Error> {
    check_cancel(cancel)?;
    validate(device)?;
    let root_path = fs::canonicalize(&device.mount_point)
        .map_err(|e| Error::io("resolve reader root", &device.mount_point, e))?;
    // Never create the mount root, including if a missing POSIX mount directory remains.
    let root = Directory::open(&root_path)?;
    let filename = file
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid(file))?;
    let filename = normalize_component(filename);
    let mut source = File::open(file).map_err(|e| Error::io("open transfer source", file, e))?;
    let metadata = source
        .metadata()
        .map_err(|e| Error::io("inspect transfer source", file, e))?;
    if !metadata.is_file() {
        return Err(invalid(file));
    }
    let total = metadata.len();
    validate(device)?;
    check_cancel(cancel)?;
    root.ensure_current()?;
    let dir = root.child(&match device.kind {
        DeviceKind::Kobo => normalize_component(author),
        DeviceKind::Kindle => "documents".into(),
    })?;
    let dest = dir.path.join(&filename);
    // Early conflict check is only an optimization; publication is exclusive too.
    match fs::symlink_metadata(&dest) {
        Ok(_) => {
            return Err(Error {
                operation: "publish reader book",
                path: Some(dest),
                kind: ErrorKind::Conflict,
            });
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io("inspect reader destination", &dest, e)),
    }
    validate(device)?;
    root.ensure_current()?;
    dir.ensure_current()?;
    let (temp, mut output) = loop {
        let name = format!(
            ".rulibre-{}-{}.part",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        match dir.create(OsStr::new(&name)) {
            Ok(output) => break (name, output),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(Error::io(
                    "create partial reader book",
                    &dir.path.join(name),
                    e,
                ));
            }
        }
    };
    let temp_path = dir.path.join(&temp);
    let result = (|| {
        let mut copied = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            validate(device)?;
            root.ensure_current()?;
            dir.ensure_current()?;
            let count = source
                .read(&mut buffer)
                .map_err(|e| Error::io("read transfer source", file, e))?;
            if count == 0 {
                break;
            }
            write(&mut output, &buffer[..count])
                .map_err(|e| Error::io("write partial reader book", &temp_path, e))?;
            copied += count as u64;
            progress(copied, total);
        }
        if copied != total
            || output
                .metadata()
                .map_err(|e| Error::io("inspect partial reader book", &temp_path, e))?
                .len()
                != total
        {
            return Err(Error::invalid_data(
                "verify transfer size",
                &temp_path,
                "source changed or incomplete copy",
            ));
        }
        output
            .sync_all()
            .map_err(|e| Error::io("flush reader book", &temp_path, e))?;
        check_cancel(cancel)?;
        validate(device)?;
        root.ensure_current()?;
        dir.ensure_current()?;
        Ok(())
    })();
    drop(output);
    let result = result.and_then(|()| {
        check_cancel(cancel)?;
        validate(device)?;
        root.ensure_current()?;
        dir.ensure_current()?;
        dir.publish(OsStr::new(&temp), OsStr::new(&filename))
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    Error {
                        operation: "publish reader book",
                        path: Some(dest.clone()),
                        kind: ErrorKind::Conflict,
                    }
                } else {
                    Error::io("publish reader book", &dest, e)
                }
            })
    });
    match result {
        Ok(()) => Ok(dest),
        Err(cause) => {
            // Never clean a path on a replacement volume; retain the original error.
            let cleanup = validate(device)
                .and_then(|()| root.ensure_current())
                .and_then(|()| dir.ensure_current())
                .and_then(|()| {
                    dir.remove(OsStr::new(&temp))
                        .map_err(|e| Error::io("remove partial reader book", &temp_path, e))
                });
            match cleanup {
                Ok(()) => Err(cause),
                Err(cleanup_error) => Err(Error {
                    operation: "cleanup failed transfer",
                    path: Some(temp_path.clone()),
                    kind: ErrorKind::PartialTransfer {
                        cause: Box::new(cause),
                        partial_path: temp_path,
                        cleanup_error: Box::new(cleanup_error),
                    },
                }),
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn transfer_fixture(
    file: &Path,
    device: &Device,
    author: &str,
) -> Result<PathBuf, Error> {
    transfer_fixture_cancellable(
        file,
        device,
        author,
        &CancellationToken::default(),
        |_, _| {},
    )
}

#[cfg(test)]
pub(crate) fn transfer_fixture_cancellable(
    file: &Path,
    device: &Device,
    author: &str,
    cancel: &CancellationToken,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, Error> {
    transfer_with(
        file,
        device,
        author,
        cancel,
        progress,
        |_| Ok(()),
        |out, bytes| out.write_all(bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, device, snapshot};

    fn source(f: &Fixture) -> PathBuf {
        f.book.join("A small book.epub")
    }
    fn parts(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension() == Some(OsStr::new("part")))
            .collect()
    }
    fn io_writer(out: &mut File, bytes: &[u8]) -> io::Result<()> {
        out.write_all(bytes)
    }

    #[test]
    fn readiness_pipeline_preserves_library_across_success_and_expected_failures() {
        use crate::{
            config::Config,
            device,
            jobs::{JobExecutor, JobOutput, JobRequest},
            volumes::MountedVolume,
        };
        use std::cell::Cell;

        let f = Fixture::new();
        let before = snapshot(&f.library);
        let config_path = f.root.join("config.toml");
        Config {
            library_path: f.library.clone(),
        }
        .save_to(&config_path)
        .unwrap();
        let config = Config::load_from(&config_path).unwrap().unwrap();
        assert!(crate::config::is_calibre_library(&config.library_path).unwrap());
        let mut jobs = JobExecutor::new(2, 8).unwrap();
        let JobOutput::Scan(scan) = jobs
            .submit(JobRequest::Scan {
                library: config.library_path,
            })
            .unwrap()
            .wait()
            .unwrap()
        else {
            panic!("expected scan result");
        };
        assert!(scan.warnings.is_empty());
        let book = scan.books.iter().find(|b| b.path == f.book).unwrap();
        let JobOutput::Metadata(metadata) = jobs
            .submit(JobRequest::Metadata {
                book: book.path.clone(),
            })
            .unwrap()
            .wait()
            .unwrap()
        else {
            panic!("expected metadata result");
        };
        assert!(!metadata.title.is_empty());
        assert!(!metadata.authors.is_empty());
        let volumes = [MountedVolume {
            id: "readiness-kobo".into(),
            mount_point: f.kobo.clone(),
            label: None,
            filesystem: "vfat".into(),
            read_only: false,
        }];
        let report = device::detect_devices_from_volumes(&volumes);
        assert!(report.warnings.is_empty());
        let reader = report.single_device().unwrap();
        let prepared =
            device::prepare_file(&book.path, reader, device::PreparationOptions::default())
                .unwrap();
        assert!(!prepared.is_temporary());
        let validate = |d: &Device| {
            let current = device::manual_device_from_volumes(&d.mount_point, d.kind, &volumes)?;
            assert_eq!(current.id, d.id);
            Ok(())
        };
        let path = transfer_with(
            &prepared.path,
            reader,
            &book.author,
            &CancellationToken::default(),
            |_, _| {},
            validate,
            io_writer,
        )
        .unwrap();
        let sent = fs::read(&path).unwrap();
        assert_eq!(sent, fs::read(&prepared.path).unwrap());
        let error = transfer_with(
            &prepared.path,
            reader,
            &book.author,
            &CancellationToken::default(),
            |_, _| {},
            validate,
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Conflict));
        assert_eq!(fs::read(&path).unwrap(), sent);
        assert_eq!(snapshot(&f.library), before);

        // Capability injection avoids changing process-wide PATH during parallel tests.
        let txt = scan.books.iter().find(|b| b.path != f.book).unwrap();
        assert!(
            device::plan_preparation(
                &txt.files,
                reader,
                device::PreparationOptions::default(),
                &[]
            )
            .is_err()
        );
        let token = CancellationToken::default();
        let error = transfer_with(
            &prepared.path,
            reader,
            "Cancelled",
            &token,
            |_, _| token.cancel(),
            validate,
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Cancelled));
        assert!(
            fs::read_dir(reader.books_dir("Cancelled"))
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(snapshot(&f.library), before);

        // Simulate loss of volume visibility after the first copy progress event.
        let disconnected = Cell::new(false);
        let error = transfer_with(
            &prepared.path,
            reader,
            "Disconnected",
            &CancellationToken::default(),
            |_, _| disconnected.set(true),
            |d| {
                if disconnected.get() {
                    Err(Error::unavailable(
                        "reader disconnected",
                        Some(&d.mount_point),
                    ))
                } else {
                    validate(d)
                }
            },
            io_writer,
        )
        .unwrap_err();
        let ErrorKind::PartialTransfer {
            cause,
            partial_path,
            cleanup_error,
        } = error.kind
        else {
            panic!("expected retained partial-file diagnostics");
        };
        assert!(matches!(cause.kind, ErrorKind::Unavailable));
        assert!(matches!(cleanup_error.kind, ErrorKind::Unavailable));
        assert!(
            !reader
                .books_dir("Disconnected")
                .join(prepared.path.file_name().unwrap())
                .exists()
        );
        fs::remove_file(partial_path).unwrap(); // fixture reconnection/cleanup
        assert_eq!(snapshot(&f.library), before);
        assert_eq!(fs::read(path).unwrap(), sent);
        jobs.shutdown();
    }

    #[test]
    fn success_preserves_library_normalizes_author_and_cleans_temp() {
        let f = Fixture::new();
        let before = snapshot(&f.library);
        let d = device(DeviceKind::Kobo, &f.kobo);
        let path = transfer_fixture(&source(&f), &d, "../C:\\Автор").unwrap();
        assert_eq!(
            path.parent().unwrap(),
            fs::canonicalize(f.kobo.join(".._C__Автор")).unwrap()
        );
        assert_eq!(fs::read(&path).unwrap(), fs::read(source(&f)).unwrap());
        assert!(parts(path.parent().unwrap()).is_empty());
        assert_eq!(snapshot(&f.library), before);
        for (input, expected) in [
            ("..", "Unknown"),
            ("CON.epub", "_CON.epub"),
            (" LPT1. ", "_LPT1"),
            ("", "Unknown"),
            ("/abs", "_abs"),
            ("a\0b", "a_b"),
        ] {
            assert_eq!(normalize_component(input), expected);
        }
    }

    #[test]
    fn conflict_before_and_during_copy_preserves_existing_book() {
        let f = Fixture::new();
        let d = device(DeviceKind::Kindle, &f.kindle);
        let dest = d.books_dir("").join("A small book.epub");
        fs::write(&dest, b"old book").unwrap();
        assert!(matches!(
            transfer_fixture(&source(&f), &d, "").unwrap_err().kind,
            ErrorKind::Conflict
        ));
        assert_eq!(fs::read(&dest).unwrap(), b"old book");
        fs::remove_file(&dest).unwrap();
        let error = transfer_with(
            &source(&f),
            &d,
            "",
            &CancellationToken::default(),
            |_, _| {
                fs::write(&dest, b"concurrent book").unwrap();
            },
            |_| Ok(()),
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Conflict));
        assert_eq!(fs::read(&dest).unwrap(), b"concurrent book");
        assert!(parts(dest.parent().unwrap()).is_empty());
    }

    #[test]
    fn partial_write_and_permission_errors_clean_temp_without_final_file() {
        let f = Fixture::new();
        let d = device(DeviceKind::Kindle, &f.kindle);
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::WriteZero] {
            let error = transfer_with(
                &source(&f),
                &d,
                "",
                &CancellationToken::default(),
                |_, _| {},
                |_| Ok(()),
                |out, bytes| {
                    out.write_all(&bytes[..3])?;
                    Err(io::Error::from(kind))
                },
            )
            .unwrap_err();
            assert!(matches!(error.kind, ErrorKind::Io(ref e) if e.kind() == kind));
            assert!(!d.books_dir("").join("A small book.epub").exists());
            assert!(parts(&d.books_dir("")).is_empty());
        }
    }

    #[test]
    fn cancellation_before_and_during_copy_cleans_partial_file() {
        let f = Fixture::new();
        let d = device(DeviceKind::Kobo, &f.kobo);
        let token = CancellationToken::default();
        token.cancel();
        let error = transfer_with(
            &source(&f),
            &d,
            "Author",
            &token,
            |_, _| {},
            |_| panic!("cancelled before validation"),
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Cancelled));
        assert!(!d.books_dir("Author").exists());
        let token = CancellationToken::default();
        fs::write(source(&f), vec![7; 200_000]).unwrap();
        let mut bytes = 0;
        let error = transfer_with(
            &source(&f),
            &d,
            "Author",
            &token,
            |copied, total| {
                assert_eq!(total, 200_000);
                bytes = copied;
                token.cancel();
            },
            |_| Ok(()),
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Cancelled));
        assert_eq!(bytes, 64 * 1024);
        assert!(
            fs::read_dir(d.books_dir("Author"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn missing_mount_root_is_never_recreated() {
        let f = Fixture::new();
        let missing = f.root.join("missing-reader");
        let d = device(DeviceKind::Kobo, &missing);
        assert!(transfer_fixture(&source(&f), &d, "Author").is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn disconnect_reports_cause_and_partial_path_then_reconnect_allows_cleanup() {
        use std::cell::Cell;
        let f = Fixture::new();
        let d = device(DeviceKind::Kindle, &f.kindle);
        let disconnected = Cell::new(false);
        let error = transfer_with(
            &source(&f),
            &d,
            "",
            &CancellationToken::default(),
            |_, _| disconnected.set(true),
            |d| {
                if disconnected.get() {
                    Err(Error::unavailable(
                        "reader disconnected",
                        Some(&d.mount_point),
                    ))
                } else {
                    Ok(())
                }
            },
            io_writer,
        )
        .unwrap_err();
        let ErrorKind::PartialTransfer {
            cause,
            partial_path,
            cleanup_error,
        } = error.kind
        else {
            panic!("missing cleanup diagnostics")
        };
        assert!(matches!(cause.kind, ErrorKind::Unavailable));
        assert!(matches!(cleanup_error.kind, ErrorKind::Unavailable));
        assert!(partial_path.is_file());
        assert!(!d.books_dir("").join("A small book.epub").exists());
        fs::remove_file(partial_path).unwrap();
        assert!(parts(&d.books_dir("")).is_empty());
    }

    #[test]
    fn size_change_aborts_publication() {
        let f = Fixture::new();
        let d = device(DeviceKind::Kindle, &f.kindle);
        let path = source(&f);
        let error = transfer_with(
            &path,
            &d,
            "",
            &CancellationToken::default(),
            |_, _| {
                // Append just once, so the next chunk discovers a changed source size.
                if fs::metadata(&path).unwrap().len() < 100 {
                    OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .unwrap()
                        .write_all(&[0; 100])
                        .unwrap();
                }
            },
            |_| Ok(()),
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvalidData(_)));
        assert!(parts(&d.books_dir("")).is_empty());
    }

    #[test]
    fn volume_identity_read_only_and_unmounted_local_directory_are_rejected() {
        let f = Fixture::new();
        let mut volumes = vec![crate::volumes::MountedVolume {
            id: "usb-one".into(),
            mount_point: f.kobo.clone(),
            label: None,
            filesystem: "vfat".into(),
            read_only: false,
        }];
        let automatic = crate::device::detect_devices_from_volumes(&volumes)
            .devices
            .remove(0);
        let manual =
            crate::device::manual_device_from_volumes(&f.kobo, DeviceKind::Kobo, &volumes).unwrap();
        for d in [&automatic, &manual] {
            validate_device_from_volumes(d, &volumes).unwrap();
            assert!(validate_device_from_volumes(d, &[]).is_err());
            volumes[0].read_only = true;
            assert!(validate_device_from_volumes(d, &volumes).is_err());
            volumes[0].read_only = false;
            volumes[0].id = "usb-two".into();
            assert!(validate_device_from_volumes(d, &volumes).is_err());
            volumes[0].id = "usb-one".into();
        }
        // An unmounted directory still exists but must receive no author directory.
        assert!(
            transfer_with(
                &source(&f),
                &automatic,
                "Author",
                &CancellationToken::default(),
                |_, _| {},
                |d| validate_device_from_volumes(d, &[]),
                io_writer
            )
            .is_err()
        );
        assert!(!automatic.books_dir("Author").exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_lock_returns_real_access_error() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new();
        let d = device(DeviceKind::Kindle, &f.kindle);
        let _lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
            .open(d.books_dir(""))
            .unwrap();
        let error = transfer_fixture(&source(&f), &d, "").unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Io(ref e) if e.raw_os_error() == Some(32)));
        drop(_lock);
        assert!(parts(&d.books_dir("")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn linked_destination_is_rejected_and_renamed_directory_is_not_published() {
        let f = Fixture::new();
        let d = device(DeviceKind::Kobo, &f.kobo);
        std::os::unix::fs::symlink(&f.library, d.books_dir("Author")).unwrap();
        let before = snapshot(&f.library);
        assert!(transfer_fixture(&source(&f), &d, "Author").is_err());
        assert_eq!(snapshot(&f.library), before);
        fs::remove_file(d.books_dir("Author")).unwrap();
        let moved = f.kobo.join("moved");
        let error = transfer_with(
            &source(&f),
            &d,
            "Author",
            &CancellationToken::default(),
            |_, _| {
                fs::rename(d.books_dir("Author"), &moved).unwrap();
                fs::create_dir(d.books_dir("Author")).unwrap();
            },
            |_| Ok(()),
            io_writer,
        )
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::PartialTransfer { .. }));
        assert!(!moved.join("A small book.epub").exists());
        assert!(
            fs::read_dir(d.books_dir("Author"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_cannot_redirect_transfer_into_library() {
        let f = Fixture::new();
        let d = device(DeviceKind::Kobo, &f.kobo);
        let before = snapshot(&f.library);
        let junction = d.books_dir("Author");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(junction.to_str().unwrap().replace('/', "\\"))
            .arg(f.library.to_str().unwrap().replace('/', "\\"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let error = transfer_fixture(&source(&f), &d, "Author").unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvalidInput));
        assert_eq!(snapshot(&f.library), before);
        fs::remove_dir(junction).unwrap();
    }
}
