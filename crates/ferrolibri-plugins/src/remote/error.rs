use std::fmt;

use ferrolibri_core::{Error, ErrorKind};

use crate::{HostError, HostErrorKind};

/// Why a plugin could not be fetched from an address. The variants are for the interface to
/// phrase in the user's language; `Display` is a plain English description for logs.
#[derive(Debug)]
#[non_exhaustive]
pub enum RemoteError {
    /// GitHub refused because the limit of requests is used up (HTTP 429, or 403 with
    /// `x-ratelimit-remaining: 0`). `reset_at` is when it ends, in seconds since 1970-01-01 UTC.
    RateLimited { reset_at: Option<u64> },
    /// The server answered 404 (or 410) for `what`.
    NotFound { what: String },
    /// Any other status that is not a success.
    Status { status: u16, what: String },
    /// The request itself failed: no connection, a blocked domain, a timeout, a body that is
    /// too large, cancellation.
    Network(HostError),
    /// The answer is not what was expected: not JSON, not a plugin manifest, and so on.
    Invalid { what: String, message: String },
    /// The downloaded file is not the one that was announced.
    HashMismatch {
        file: String,
        announced_by: &'static str,
    },
    /// Reading or writing files, or the plugin itself, failed.
    Local(Error),
}

impl RemoteError {
    pub(crate) fn invalid(what: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            what: what.into(),
            message: message.into(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Network(e) => e.kind == HostErrorKind::Cancelled,
            Self::Local(e) => matches!(e.kind, ErrorKind::Cancelled),
            _ => false,
        }
    }

    /// The failure as a core error. Details are lost for everything but local errors.
    pub fn into_core(self, operation: &'static str) -> Error {
        match self {
            Self::Network(e) => e.into_core(operation),
            Self::Local(e) => e,
            _ => Error {
                operation,
                path: None,
                kind: ErrorKind::Network,
            },
        }
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited { .. } => f.write_str(
                "the limit of requests is used up (GitHub allows 60 an hour), try again later",
            ),
            Self::NotFound { what } => write!(f, "{what} was not found"),
            Self::Status { status, what } => write!(f, "{what}: the server answered {status}"),
            Self::Network(e) => e.fmt(f),
            Self::Invalid { what, message } => write!(f, "{what}: {message}"),
            Self::HashMismatch { file, announced_by } => {
                write!(f, "{file} does not match the hash from {announced_by}")
            }
            Self::Local(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RemoteError {}

impl From<HostError> for RemoteError {
    fn from(e: HostError) -> Self {
        Self::Network(e)
    }
}

impl From<Error> for RemoteError {
    fn from(e: Error) -> Self {
        Self::Local(e)
    }
}
