//! Test support: a scriptable local HTTP server and a stand-in for the OPDS catalog.
//!
//! Compiled for this crate's own tests and, for other crates, behind the
//! `test-support` feature:
//!
//! ```toml
//! [dev-dependencies]
//! ferrolibri-plugins = { path = "../ferrolibri-plugins", features = ["test-support"] }
//! ```
//!
//! ```ignore
//! use ferrolibri_plugins::testing::{TestServer, opds};
//!
//! let server = TestServer::start(opds::handle);      // the OPDS catalog stand-in
//! let session = server.session();                    // plugin "test", the user typed the catalog URL
//! let feed = session.fetch(&server.url("/opds/search?searchTerm=war"), &[])?;
//! let book = session.fetch(&server.url("/b/12/fb2"), &[])?; // two 302s, ends in application/zip
//! ```
//!
//! Names (`catalog.test`, `static.catalog.test`, anything else) resolve to the loopback address
//! through [`TestServer::resolver`], so redirects between "different hosts" work without DNS.
//! `server.http()` is a client wired that way, without the 250 ms pause between requests to one
//! host; `server.config()` is the same configuration for tests that adjust it. For a custom
//! route table use `TestServer::start(|request, context| Reply::...)`.

pub mod opds;
mod server;
mod zipfile;

pub use server::{CATALOG_HOST, Context, LoopbackResolver, Reply, Request, TestServer};
pub use zipfile::{single_file_zip, zip_declaring, zip_stored};
