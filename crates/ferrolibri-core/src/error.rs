use std::{
    fmt, io,
    path::{Path, PathBuf},
    process::ExitStatus,
};

/// Machine-readable failure category. I/O errors retain their original ErrorKind.
#[derive(Debug)]
pub enum ErrorKind {
    Io(io::Error),
    InvalidData(Box<dyn std::error::Error + Send + Sync>),
    Unavailable,
    ConversionUnavailable {
        diagnostic: String,
    },
    /// `calibredb` is not installed or cannot be located.
    CalibreUnavailable {
        diagnostic: String,
    },
    InvalidInput,
    Conflict,
    PartialTransfer {
        cause: Box<Error>,
        partial_path: PathBuf,
        cleanup_error: Box<Error>,
    },
    Cancelled,
    TimedOut,
    /// A downloaded book could not be added to the library. `kept` is where the file was saved
    /// for the user (an export folder), if one was set.
    BookNotAdded {
        cause: Box<Error>,
        kept: Option<PathBuf>,
    },
    ExternalProcess {
        program: &'static str,
        status: ExitStatus,
        stderr: Vec<u8>,
    },
    /// A network request made on behalf of the app failed. The detail (timeout, DNS, TLS, ...)
    /// is kept by the layer that made the request.
    Network,
    /// A plugin misbehaved or returned data that violates the plugin contract.
    Plugin {
        id: String,
        message: String,
    },
}

#[derive(Debug)]
pub struct Error {
    pub operation: &'static str,
    pub path: Option<PathBuf>,
    pub kind: ErrorKind,
}

impl Error {
    pub fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self {
            operation,
            path: Some(path.to_owned()),
            kind: ErrorKind::Io(source),
        }
    }

    pub fn invalid_data(
        operation: &'static str,
        path: &Path,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            operation,
            path: Some(path.to_owned()),
            kind: ErrorKind::InvalidData(source.into()),
        }
    }

    pub fn unavailable(operation: &'static str, path: Option<&Path>) -> Self {
        Self {
            operation,
            path: path.map(Path::to_owned),
            kind: ErrorKind::Unavailable,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.operation)?;
        if let Some(path) = &self.path {
            write!(f, " ({})", path.display())?;
        }
        match &self.kind {
            ErrorKind::Io(e) => write!(f, ": {e}"),
            ErrorKind::InvalidData(e) => write!(f, ": invalid data: {e}"),
            ErrorKind::Unavailable => write!(f, ": unavailable"),
            ErrorKind::ConversionUnavailable { diagnostic }
            | ErrorKind::CalibreUnavailable { diagnostic } => write!(f, ": {diagnostic}"),
            ErrorKind::InvalidInput => write!(f, ": invalid input"),
            ErrorKind::Conflict => write!(f, ": destination already exists"),
            ErrorKind::PartialTransfer {
                cause,
                partial_path,
                cleanup_error,
            } => write!(
                f,
                ": {cause}; partial file may remain at {}: {cleanup_error}",
                partial_path.display()
            ),
            ErrorKind::BookNotAdded { cause, kept } => {
                write!(f, ": {cause}")?;
                match kept {
                    Some(path) => write!(f, "; the downloaded file was kept at {}", path.display()),
                    None => Ok(()),
                }
            }
            ErrorKind::Cancelled => write!(f, ": cancelled"),
            ErrorKind::TimedOut => write!(f, ": timed out"),
            ErrorKind::ExternalProcess {
                program,
                status,
                stderr,
            } => write!(
                f,
                ": {program} failed ({status}): {}",
                String::from_utf8_lossy(stderr).trim()
            ),
            ErrorKind::Network => write!(f, ": network error"),
            ErrorKind::Plugin { id, message } => write!(f, ": plugin {id}: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(e) => Some(e),
            ErrorKind::InvalidData(e) => Some(e.as_ref()),
            ErrorKind::PartialTransfer { cause, .. } | ErrorKind::BookNotAdded { cause, .. } => {
                Some(cause.as_ref())
            }
            _ => None,
        }
    }
}

/// Read regular files without hiding directory-entry or metadata failures.
pub(crate) fn read_files(path: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| Error::io("read directory", path, e))? {
        let entry = entry.map_err(|e| Error::io("read directory entry", path, e))?;
        let metadata = std::fs::metadata(entry.path())
            .map_err(|e| Error::io("read file metadata", &entry.path(), e))?;
        if metadata.is_file() {
            files.push(entry.path());
        }
    }
    Ok(files)
}
