use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Why `host.fetch` failed. The plugin sees it as the `kind` field of the thrown exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostErrorKind {
    Timeout,
    Connect,
    Dns,
    Tls,
    /// The domain is neither in the manifest nor entered by the user.
    BlockedDomain,
    TooLarge,
    /// The per-call limit on `host.fetch` requests is exhausted.
    TooManyRequests,
    Cancelled,
}

impl HostErrorKind {
    pub const ALL: [HostErrorKind; 8] = [
        Self::Timeout,
        Self::Connect,
        Self::Dns,
        Self::Tls,
        Self::BlockedDomain,
        Self::TooLarge,
        Self::TooManyRequests,
        Self::Cancelled,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Dns => "dns",
            Self::Tls => "tls",
            Self::BlockedDomain => "blocked_domain",
            Self::TooLarge => "too_large",
            Self::TooManyRequests => "too_many_requests",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for HostErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HostErrorKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, ()> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or(())
    }
}
