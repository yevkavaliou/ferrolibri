use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Waiting,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Scanning,
    Converting,
    Preparing,
    Copying,
    /// Fetching a book from the network (`ExternalJob`s).
    Downloading,
    /// `calibredb add` is running.
    Adding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobEvent {
    State {
        id: JobId,
        state: JobState,
    },
    Stage {
        id: JobId,
        stage: Stage,
    },
    CopyProgress {
        id: JobId,
        completed: u64,
        total: u64,
    },
    /// Bytes received so far; `total` is `None` when the server did not announce a size.
    DownloadProgress {
        id: JobId,
        completed: u64,
        total: Option<u64>,
    },
}
