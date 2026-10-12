use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Recognized book formats. Recognition does not imply converter/device support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Format {
    Azw3,
    Docx,
    Epub,
    Kepub,
    Mobi,
    Pdf,
    Txt,
}

impl Format {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Azw3 => "AZW3",
            Self::Docx => "DOCX",
            Self::Epub => "EPUB",
            Self::Kepub => "KEPUB",
            Self::Mobi => "MOBI",
            Self::Pdf => "PDF",
            Self::Txt => "TXT",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.to_ascii_uppercase().as_str() {
            "AZW3" => Some(Self::Azw3),
            "DOCX" => Some(Self::Docx),
            "EPUB" => Some(Self::Epub),
            "KEPUB" => Some(Self::Kepub),
            "MOBI" => Some(Self::Mobi),
            "PDF" => Some(Self::Pdf),
            "TXT" => Some(Self::Txt),
            _ => None,
        }
    }

    pub const fn ebook_convert_input(self) -> bool {
        !matches!(self, Self::Kepub)
    }

    pub const fn kepubify_input(self) -> bool {
        matches!(self, Self::Epub)
    }
}

/// IDs are scoped to a library. Relative paths preserve native path encoding.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum BookId {
    Calibre(u64),
    RelativePath(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookFile {
    pub format: Format,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub id: BookId,
    pub author: String,
    pub title: String,
    pub files: Vec<BookFile>,
    pub path: PathBuf,
}
