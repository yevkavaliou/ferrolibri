use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub library_path: PathBuf,
}

impl Config {
    pub fn path() -> Result<PathBuf, crate::Error> {
        let root = dirs::config_dir()
            .ok_or_else(|| crate::Error::unavailable("locate config directory", None))?;
        Ok(root.join("dev.yevkavaliou.ferrolibri").join("config.toml"))
    }

    pub fn load() -> Result<Option<Self>, crate::Error> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Result<Option<Self>, crate::Error> {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(crate::Error::io("read config", path, e)),
        };
        let mut config: Self = toml::from_str(&content)
            .map_err(|e| crate::Error::invalid_data("parse config", path, e))?;
        if !config.library_path.as_os_str().is_empty() {
            config.library_path = resolve_path(&config.library_path)?;
        }
        Ok(Some(config))
    }

    pub fn save(&self) -> Result<(), crate::Error> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), crate::Error> {
        self.save_with(path, |file, content| file.write_all(content))
    }

    fn save_with(
        &self,
        path: &Path,
        write: impl FnOnce(&mut fs::File, &[u8]) -> io::Result<()>,
    ) -> Result<(), crate::Error> {
        let config = Self {
            library_path: if self.library_path.as_os_str().is_empty() {
                PathBuf::new()
            } else {
                resolve_path(&self.library_path)?
            },
        };
        let content = toml::to_string_pretty(&config)
            .map_err(|e| crate::Error::invalid_data("serialize config", path, e))?;
        write_atomic_with(path, content.as_bytes(), write)
    }
}

/// Atomically replace a file after syncing its complete contents. On failure the
/// previous file is preserved and the temporary file is removed.
pub fn write_atomic(path: &Path, content: &[u8]) -> Result<(), crate::Error> {
    write_atomic_with(path, content, |file, bytes| file.write_all(bytes))
}

fn write_atomic_with(
    path: &Path,
    content: &[u8],
    write: impl FnOnce(&mut fs::File, &[u8]) -> io::Result<()>,
) -> Result<(), crate::Error> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| crate::Error::io("create config directory", parent, e))?;
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(
            ".ferrolibri-config-{}-{}.tmp",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (TemporaryFile(candidate), file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(crate::Error::io("create temporary config", path, e)),
        }
    };
    write(&mut file, content).map_err(|e| crate::Error::io("write temporary config", path, e))?;
    file.sync_all()
        .map_err(|e| crate::Error::io("sync temporary config", path, e))?;
    // Close before replacement on Windows. Never delete the existing config first.
    drop(file);
    fs::rename(&temporary.0, path).map_err(|e| crate::Error::io("replace config", path, e))
}

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn sanitize_path(input: &str) -> Result<PathBuf, crate::Error> {
    let trimmed = input.trim();
    let stripped = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let stripped = stripped.trim();
    if stripped.is_empty() {
        return Err(crate::Error {
            operation: "parse library path",
            path: None,
            kind: crate::ErrorKind::InvalidInput,
        });
    }
    resolve_path(Path::new(stripped))
}

/// Resolve against the working directory without requiring the path to exist.
/// Saved native paths are not trimmed or converted through lossy Unicode.
fn resolve_path(path: &Path) -> Result<PathBuf, crate::Error> {
    let expanded = if path == Path::new("~") {
        dirs::home_dir()
            .ok_or_else(|| crate::Error::unavailable("locate home directory", Some(path)))?
    } else if let Some(text) = path
        .to_str()
        .and_then(|text| text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")))
    {
        dirs::home_dir()
            .ok_or_else(|| crate::Error::unavailable("locate home directory", Some(path)))?
            .join(text)
    } else {
        path.to_owned()
    };
    std::path::absolute(&expanded).map_err(|e| crate::Error::io("resolve library path", path, e))
}

/// What a folder chosen as the library location contains.
///
/// `Missing` is deliberately separate from the "not a library" states: an unplugged
/// disk or an offline network share looks the same as a mistyped path, and the caller
/// must never offer to create a library there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryPathState {
    /// A directory containing a `metadata.db` file.
    Ready,
    /// The path does not exist.
    Missing,
    /// An existing, completely empty directory: a new library can be created here.
    NotLibraryEmpty,
    /// A directory with entries but without `metadata.db`.
    NotLibraryNotEmpty,
    /// The path exists but is not a directory.
    NotDirectory,
}

/// Classify `path` without writing anything. Access errors other than "not found" are
/// returned as errors, not mapped to a state, so that they never lead to creation.
pub fn inspect_library_path(path: &Path) -> Result<LibraryPathState, crate::Error> {
    if path.as_os_str().is_empty() {
        return Err(crate::Error {
            operation: "inspect library path",
            path: None,
            kind: crate::ErrorKind::InvalidInput,
        });
    }
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(LibraryPathState::NotDirectory),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LibraryPathState::Missing),
        Err(e) => return Err(crate::Error::io("validate library directory", path, e)),
    }
    let database = path.join("metadata.db");
    match fs::metadata(&database) {
        Ok(meta) if meta.is_file() => return Ok(LibraryPathState::Ready),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(crate::Error::io("validate library database", &database, e)),
    }
    let mut entries =
        fs::read_dir(path).map_err(|e| crate::Error::io("read library directory", path, e))?;
    match entries.next() {
        None => Ok(LibraryPathState::NotLibraryEmpty),
        Some(Ok(_)) => Ok(LibraryPathState::NotLibraryNotEmpty),
        Some(Err(e)) => Err(crate::Error::io("read library directory", path, e)),
    }
}

/// Names of up to `limit` entries of a directory, sorted, for showing the user what
/// is already in a folder before a library is created next to it.
pub fn first_entries(path: &Path, limit: usize) -> Result<Vec<String>, crate::Error> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| crate::Error::io("read folder", path, e))? {
        if names.len() >= limit {
            break;
        }
        let entry = entry.map_err(|e| crate::Error::io("read folder", path, e))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// just a really base check of metadata.db
pub fn is_calibre_library(path: &Path) -> Result<bool, crate::Error> {
    if path.as_os_str().is_empty() {
        return Ok(false);
    }
    Ok(inspect_library_path(path)? == LibraryPathState::Ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;

    #[test]
    fn partial_write_preserves_previous_config_and_cleans_temporary_file() {
        let fixture = Fixture::new();
        let path = fixture.root.join("config.toml");
        let config = Config {
            library_path: fixture.library.clone(),
        };
        config.save_to(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let error = config
            .save_with(&path, |file, _| {
                file.write_all(b"partial")?;
                Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "injected write failure",
                ))
            })
            .unwrap_err();
        assert!(
            matches!(error.kind, crate::ErrorKind::Io(e) if e.kind() == io::ErrorKind::WriteZero)
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ferrolibri-config-")
        }));
    }
}
