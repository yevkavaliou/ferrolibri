//! The HTTP layer: the only way the app and its plugins reach the network (D3, D4).
//!
//! * [`AllowList`] says which hosts a caller may talk to; every URL, including each redirect
//!   target, is checked against it, and so is the address the socket is opened to.
//! * [`Http`] is the client: one request per host at a time, with a pause between requests,
//!   typed failures ([`HostError`]), size and time limits, cancellation, no automatic retries.
//! * [`HttpSession`] is a handle for one plugin call with its own allow-list, request budget
//!   and deadline; [`ResponseStream`] reads a body piece by piece for downloads.
//!
//! HTTP statuses are never errors here. The client sends GET only, drops every request header
//! outside a short white list, ignores proxy settings and keeps no cookies.

mod addr;
mod allow;
mod client;
mod control;
mod error;
mod gate;
mod headers;
mod net;

pub use addr::is_forbidden_ip;
pub use allow::{Access, AllowList, AllowListError};
pub use client::{
    FetchResponse, Http, HttpConfig, HttpSession, Identity, ResponseHead, ResponseStream,
};
pub use error::HostError;
pub use net::{NameResolver, SystemResolver};

#[cfg(test)]
mod tests;
