pub mod book;
pub mod config;
pub mod conversion_model;
pub mod converter;
pub mod cover;
pub mod device;
pub mod device_model;
pub mod device_watch;
pub mod error;
pub mod job_model;
pub mod jobs;
pub mod metadata_model;
pub mod volumes;
pub use error::{Error, ErrorKind};
pub mod metadata;
pub mod scanner;
pub mod transfer;

#[cfg(test)]
mod test_support;
#[cfg(test)]
extern crate self as rulibre_core;
