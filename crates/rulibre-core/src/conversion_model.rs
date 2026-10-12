//! Serializable converter discovery shared with the WASM client.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Kepubify,
    EbookConvert,
}

impl BackendKind {
    pub fn program(self) -> &'static str {
        match self {
            Self::Kepubify => "kepubify",
            Self::EbookConvert => "ebook-convert",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendStatus {
    Ready,
    Missing,
    Broken,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backend {
    pub kind: BackendKind,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: BackendStatus,
    pub diagnostic: String,
    /// True only after the installed tool's EPUB -> KEPUB help probe succeeds.
    pub kepub_output: bool,
}
