use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub title: String,
    pub authors: Vec<String>,
    pub publisher: String,
    pub date: String,
    pub language: String,
    pub subjects: Vec<String>,
    pub description: String,
    pub identifiers: Vec<(String, String)>,
    pub rating: String,
    pub series: String,
    pub series_index: String,
    pub unrecognized: Vec<String>,
}
