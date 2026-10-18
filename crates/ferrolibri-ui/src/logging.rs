//! The application log: a file next to `ui.toml`, and stderr when there is a console.
//!
//! The release build on Windows has no console, so without a file its warnings (a plugin
//! failing, a cover that cannot be decoded) would vanish. `RUST_LOG` still sets the level.
use ferrolibri_core::config::Config;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "ferrolibri.log";
/// A log above this size is moved to `ferrolibri.log.old` when the app starts.
const MAX_BYTES: u64 = 1024 * 1024;

pub fn log_path() -> Option<PathBuf> {
    Config::path()
        .ok()
        .map(|path| path.with_file_name(FILE_NAME))
}

/// Starts logging: warnings by default, to the file and to stderr.
pub fn init() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));
    if let Some(file) = log_path().and_then(|path| open(&path, MAX_BYTES).ok()) {
        builder.target(env_logger::Target::Pipe(Box::new(Tee {
            file,
            stderr: io::stderr(),
        })));
    }
    builder.init();
}

/// Opens the log for appending, moving an oversized one aside first.
fn open(path: &Path, max_bytes: u64) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path).is_ok_and(|meta| meta.len() > max_bytes) {
        let old = path.with_extension("log.old");
        let _ = fs::remove_file(&old);
        fs::rename(path, &old)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Every line goes to the file; stderr is a convenience that may not exist.
struct Tee {
    file: File,
    stderr: io::Stderr,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.file.write(buf)?;
        let _ = self.stderr.write_all(&buf[..written]);
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        let _ = self.stderr.flush();
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_is_appended_to_and_an_oversized_one_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE_NAME);
        let mut file = open(&path, 16).unwrap();
        file.write_all(b"first line\n").unwrap();
        drop(file);
        let mut file = open(&path, 16).unwrap();
        file.write_all(b"second line\n").unwrap();
        drop(file);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "first line\nsecond line\n"
        );
        // 23 bytes is above the limit: the next start begins a new file.
        let mut file = open(&path, 16).unwrap();
        file.write_all(b"third line\n").unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(&path).unwrap(), "third line\n");
        assert_eq!(
            fs::read_to_string(dir.path().join("nested").join("ferrolibri.log.old")).unwrap(),
            "first line\nsecond line\n"
        );
        // Only one old log is kept.
        let mut file = open(&path, 5).unwrap();
        file.write_all(b"fourth\n").unwrap();
        drop(file);
        assert_eq!(
            fs::read_to_string(dir.path().join("nested").join("ferrolibri.log.old")).unwrap(),
            "third line\n"
        );
    }

    #[test]
    fn the_log_sits_next_to_the_ui_settings() {
        if let (Some(log), Ok(ui)) = (log_path(), crate::settings::UiSettings::path()) {
            assert_eq!(log.parent(), ui.parent());
            assert_eq!(log.file_name().unwrap(), FILE_NAME);
        }
    }
}
