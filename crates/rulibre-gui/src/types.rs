use serde::{Deserialize, Serialize};
#[path = "../../rulibre-core/src/book.rs"]
mod book_model;
pub use book_model::{BookFile, BookId};
#[path = "../../rulibre-core/src/conversion_model.rs"]
mod conversion_model;
pub use conversion_model::{Backend, BackendKind, BackendStatus};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub id: BookId,
    pub files: Vec<BookFile>,
    pub author: String,
    pub title: String,
    pub formats: String,
    pub path: String,
}

#[path = "../../rulibre-core/src/metadata_model.rs"]
mod metadata_model;
pub use metadata_model::Metadata;
#[path = "../../rulibre-core/src/job_model.rs"]
mod job_model;
pub use job_model::{JobEvent, JobId};

#[path = "../../rulibre-core/src/device_model.rs"]
mod device_model;
pub use device_model::{Device, DeviceEvent};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub library_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConvertOk {
    pub message: String,
    pub output_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanReport {
    pub books: Vec<Book>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobValue {
    Scan(ScanReport),
    Conversion(ConvertOk),
    Sent(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobReply {
    pub id: JobId,
    pub events: Vec<JobEvent>,
    pub result: Option<Result<JobValue, String>>,
}
