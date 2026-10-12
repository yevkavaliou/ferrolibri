//! MTP copy policy; native COM objects stay on the calling worker thread.
use super::check_cancel;
use crate::{Error, converter::CancellationToken, device::Device};
use std::path::{Path, PathBuf};
#[cfg(any(windows, test))]
use {
    super::normalize_component,
    crate::ErrorKind,
    std::{fs::File, io::Read},
};

#[cfg(windows)]
#[path = "mtp_windows.rs"]
mod native;

pub(super) fn validate(device: &Device) -> Result<(), Error> {
    #[cfg(windows)]
    {
        native::validate(device)
    }
    #[cfg(not(windows))]
    {
        let _ = device;
        Err(Error::unavailable("MTP requires Windows WPD", None))
    }
}

pub(super) fn transfer(
    file: &Path,
    device: &Device,
    cancel: &CancellationToken,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, Error> {
    check_cancel(cancel)?;
    #[cfg(windows)]
    {
        native::transfer(file, device, cancel, progress)
    }
    #[cfg(not(windows))]
    {
        let _ = (file, device, progress);
        Err(Error::unavailable("MTP requires Windows WPD", None))
    }
}

#[cfg(any(windows, test))]
trait Upload {
    fn begin(&mut self, name: &str, size: u64) -> Result<(), Error>;
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Error>;
    fn finish(&mut self, size: u64) -> Result<(), Error>;
    fn abort(&mut self) -> Result<(), Error>;
}

#[cfg(any(windows, test))]
fn copy(
    file: &Path,
    backend: &mut impl Upload,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, Error> {
    check_cancel(cancel)?;
    let mut input = File::open(file).map_err(|e| Error::io("open MTP source", file, e))?;
    let metadata = input
        .metadata()
        .map_err(|e| Error::io("inspect MTP source", file, e))?;
    if !metadata.is_file() {
        return Err(Error::invalid_data(
            "inspect MTP source",
            file,
            "expected a regular file",
        ));
    }
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::invalid_data("name MTP book", file, "expected a Unicode filename"))?;
    let name = normalize_component(name);
    // A display location only. It is never opened or interpreted as a local path.
    let destination = PathBuf::from("Kindle (MTP)").join("documents").join(&name);
    let total = metadata.len();
    let result = (|| {
        check_cancel(cancel)?;
        backend.begin(&name, total)?;
        let mut bytes = vec![0u8; 64 * 1024];
        let mut copied = 0u64;
        progress(0, total);
        loop {
            check_cancel(cancel)?;
            let count = input
                .read(&mut bytes)
                .map_err(|e| Error::io("read MTP source", file, e))?;
            if count == 0 {
                break;
            }
            if count as u64 > total - copied {
                return Err(Error::invalid_data(
                    "copy MTP source",
                    file,
                    "source size changed",
                ));
            }
            let mut offset = 0;
            while offset < count {
                check_cancel(cancel)?;
                let written = backend.write(&bytes[offset..count])?;
                if written == 0 || written > count - offset {
                    return Err(Error::invalid_data(
                        "write MTP book",
                        &destination,
                        "invalid stream write count",
                    ));
                }
                offset += written;
                copied += written as u64;
                progress(copied, total);
            }
        }
        if copied != total
            || input
                .metadata()
                .map_err(|e| Error::io("inspect MTP source", file, e))?
                .len()
                != total
        {
            return Err(Error::invalid_data(
                "copy MTP source",
                file,
                "source size changed",
            ));
        }
        check_cancel(cancel)?;
        backend.finish(total)?;
        // A completed commit wins over cancellation arriving afterwards.
        Ok(destination.clone())
    })();
    match result {
        Ok(path) => Ok(path),
        Err(cause) => match backend.abort() {
            Ok(()) => Err(cause),
            Err(cleanup_error) => Err(Error {
                operation: "clean up MTP transfer",
                path: Some(destination.clone()),
                kind: ErrorKind::PartialTransfer {
                    cause: Box::new(cause),
                    partial_path: destination,
                    cleanup_error: Box::new(cleanup_error),
                },
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        bytes: Vec<u8>,
        committed: bool,
        aborted: bool,
        fail_write: bool,
        fail_abort: bool,
        fail_finish: bool,
    }
    impl Upload for Fake {
        fn begin(&mut self, _: &str, _: u64) -> Result<(), Error> {
            Ok(())
        }
        fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
            if self.fail_write {
                return Err(Error::unavailable("disconnected MTP device", None));
            }
            let count = bytes.len().min(13);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn finish(&mut self, size: u64) -> Result<(), Error> {
            assert_eq!(self.bytes.len() as u64, size);
            if self.fail_finish {
                return Err(Error::unavailable("MTP commit failed", None));
            }
            self.committed = true;
            Ok(())
        }
        fn abort(&mut self) -> Result<(), Error> {
            self.aborted = true;
            if self.fail_abort {
                return Err(Error::unavailable("MTP revert failed", None));
            }
            self.bytes.clear();
            Ok(())
        }
    }
    fn fake() -> Fake {
        Fake {
            bytes: vec![],
            committed: false,
            aborted: false,
            fail_write: false,
            fail_abort: false,
            fail_finish: false,
        }
    }
    #[test]
    fn short_writes_report_actual_bytes_and_commit_full_contents() {
        let f = crate::test_support::Fixture::new();
        let file = f.book.join("Книга.azw3");
        let original = vec![42u8; 70_000];
        std::fs::write(&file, &original).unwrap();
        let mut output = fake();
        let mut events = vec![];
        let path = copy(&file, &mut output, &CancellationToken::default(), |a, b| {
            events.push((a, b))
        })
        .unwrap();
        assert_eq!(output.bytes, original);
        assert!(output.committed);
        assert!(!output.aborted);
        assert_eq!(events.last(), Some(&(70_000, 70_000)));
        assert!(events.windows(2).all(|p| p[1].0 > p[0].0));
        assert!(path.ends_with("documents/Книга.azw3"));
        assert_eq!(std::fs::read(file).unwrap(), original);
    }
    #[test]
    fn cancellation_and_disconnect_revert_and_cleanup_failure_is_explicit() {
        let f = crate::test_support::Fixture::new();
        let file = f.book.join("book.azw3");
        std::fs::write(&file, [1u8; 100]).unwrap();
        let cancel = CancellationToken::default();
        let mut output = fake();
        let error = copy(&file, &mut output, &cancel, |n, _| {
            if n > 0 {
                cancel.cancel();
            }
        })
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Cancelled));
        assert!(output.aborted);
        assert!(output.bytes.is_empty());
        let mut output = fake();
        output.fail_write = true;
        output.fail_abort = true;
        let error = copy(&file, &mut output, &CancellationToken::default(), |_, _| {}).unwrap_err();
        assert!(matches!(error.kind, ErrorKind::PartialTransfer { .. }));
        assert!(!output.committed);
    }

    #[test]
    fn changed_source_and_failed_commit_never_report_success() {
        let f = crate::test_support::Fixture::new();
        let file = f.book.join("book.azw3");
        for changed_size in [50, 150] {
            std::fs::write(&file, [1u8; 100]).unwrap();
            let mut output = fake();
            let error = copy(&file, &mut output, &CancellationToken::default(), |n, _| {
                if n == 0 {
                    std::fs::write(&file, vec![1u8; changed_size]).unwrap();
                }
            })
            .unwrap_err();
            assert!(matches!(error.kind, ErrorKind::InvalidData(_)));
            assert!(output.aborted);
            assert!(!output.committed);
        }
        let mut output = fake();
        output.fail_finish = true;
        let error = copy(&file, &mut output, &CancellationToken::default(), |_, _| {}).unwrap_err();
        assert_eq!(error.operation, "MTP commit failed");
        assert!(output.aborted);
        assert!(output.bytes.is_empty());
        assert!(!output.committed);
    }
}
