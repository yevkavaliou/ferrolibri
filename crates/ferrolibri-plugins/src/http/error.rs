use std::fmt;

use ferrolibri_core::{Error, ErrorKind};

use crate::HostErrorKind;

/// A transport-level failure of an HTTP request. HTTP statuses (4xx/5xx included) are not
/// failures: they come back as responses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    pub kind: HostErrorKind,
    pub message: String,
}

impl HostError {
    pub fn new(kind: HostErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn blocked(message: impl Into<String>) -> Self {
        Self::new(HostErrorKind::BlockedDomain, message)
    }

    /// The failure as a core error. `ErrorKind::Network` carries no detail: log the
    /// `HostError` where it occurs if the detail matters.
    pub fn into_core(self, operation: &'static str) -> Error {
        Error {
            operation,
            path: None,
            kind: match self.kind {
                HostErrorKind::Cancelled => ErrorKind::Cancelled,
                _ => ErrorKind::Network,
            },
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for HostError {}
