//! The part that touches `ureq`: connecting only to addresses this crate has vetted, and
//! giving up promptly when a request is cancelled.
//!
//! `ureq` is used through three of its "unversioned" extension points (a resolver, a transport
//! wrapper and the connector chain), which are documented as not following semver; the
//! dependency is therefore locked by `Cargo.lock` and upgraded deliberately.
//!
//! * **Pinning.** Before each request this crate resolves the host name itself, drops the
//!   forbidden addresses (`addr.rs`) and registers what is left under `host:port`. `ureq` is
//!   given a resolver that serves only those registered addresses, so the socket can only be
//!   opened to an address that passed the check: a DNS answer that changes between the check and
//!   the connection cannot matter, because there is no second lookup.
//! * **Cancellation.** A transport wrapper sits between TCP and TLS. It cuts every wait into
//!   slices of [`SLICE`] and looks at the cancellation flag between them, and it also enforces
//!   the "no byte for too long" limit. A blocking socket read therefore stops within about
//!   100 ms of the cancellation, in the TLS handshake and in the body alike. Only the TCP
//!   connect itself runs to its timeout.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ureq::Timeout;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::time::Duration as UreqDuration;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, NextTimeout, RustlsConnector, TcpConnector, Transport,
};

use super::control::Control;
use super::error::HostError;
use crate::HostErrorKind;

/// How often a blocked read looks at the cancellation flag.
const SLICE: Duration = Duration::from_millis(100);

/// Turns a host name into addresses. The default asks the operating system; tests plug their own.
pub trait NameResolver: Send + Sync + 'static {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>>;
}

impl<F> NameResolver for F
where
    F: Fn(&str, u16) -> io::Result<Vec<IpAddr>> + Send + Sync + 'static,
{
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        self(host, port)
    }
}

/// The operating system's resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemResolver;

impl NameResolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        Ok((host, port).to_socket_addrs()?.map(|a| a.ip()).collect())
    }
}

/// Resolves in a helper thread so that the wait can end on cancellation or timeout; an
/// abandoned lookup finishes on its own and its answer is dropped.
pub(crate) fn resolve_name(
    resolver: &Arc<dyn NameResolver>,
    host: &str,
    port: u16,
    control: &Control,
    timeout: Duration,
) -> Result<Vec<IpAddr>, HostError> {
    let (sender, receiver) = std::sync::mpsc::channel();
    {
        let resolver = Arc::clone(resolver);
        let host = host.to_owned();
        std::thread::Builder::new()
            .name("ferrolibri-dns".into())
            .spawn(move || {
                let _ = sender.send(resolver.resolve(&host, port));
            })
            .map_err(|e| {
                HostError::new(HostErrorKind::Dns, format!("cannot start a lookup: {e}"))
            })?;
    }
    let started = Instant::now();
    loop {
        control.check()?;
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(HostError::new(
                HostErrorKind::Timeout,
                format!("looking up {host} timed out"),
            ));
        }
        match receiver.recv_timeout(left.min(Duration::from_millis(50))) {
            Ok(Ok(addresses)) if addresses.is_empty() => {
                return Err(HostError::new(
                    HostErrorKind::Dns,
                    format!("no address for {host}"),
                ));
            }
            Ok(Ok(addresses)) => return Ok(addresses),
            Ok(Err(e)) => {
                return Err(HostError::new(
                    HostErrorKind::Dns,
                    format!("cannot resolve {host}: {e}"),
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(HostError::new(
                    HostErrorKind::Dns,
                    "the lookup thread failed",
                ));
            }
        }
    }
}

/// A request in flight to one `host:port`: the vetted addresses and how to stop it.
#[derive(Debug)]
pub(crate) struct Active {
    pub addresses: Vec<SocketAddr>,
    pub control: Control,
    /// A read that waits this long without receiving a byte fails with a timeout.
    pub idle: Duration,
}

/// `host:port` → the request currently talking to it. The per-host gate guarantees there is at
/// most one such request per host.
#[derive(Clone, Debug, Default)]
pub(crate) struct Registry(Arc<Mutex<HashMap<String, Arc<Active>>>>);

impl Registry {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Active>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn insert(&self, key: String, active: Active) {
        self.map().insert(key, Arc::new(active));
    }

    pub fn remove(&self, key: &str) {
        self.map().remove(key);
    }

    fn get(&self, key: &str) -> Option<Arc<Active>> {
        self.map().get(key).cloned()
    }
}

pub(crate) fn key_for(host: &str, port: u16) -> String {
    format!("{}:{port}", host.to_ascii_lowercase())
}

fn key_for_uri(uri: &Uri) -> Option<String> {
    let port = uri.port_u16().or(match uri.scheme_str() {
        Some("http") => Some(80),
        Some("https") => Some(443),
        _ => None,
    })?;
    Some(key_for(uri.host()?, port))
}

/// Hands `ureq` the registered addresses and nothing else. It never does a lookup.
#[derive(Debug)]
struct PinnedResolver {
    registry: Registry,
}

impl Resolver for PinnedResolver {
    fn resolve(
        &self,
        uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let active = key_for_uri(uri)
            .and_then(|key| self.registry.get(&key))
            .ok_or(ureq::Error::HostNotFound)?;
        let mut out = self.empty();
        for address in active.addresses.iter().take(16) {
            out.push(*address);
        }
        if out.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(out)
    }
}

/// Wraps the connected TCP stream (below TLS) of the request registered for its host.
#[derive(Debug)]
struct SlicedConnector {
    registry: Registry,
}

impl<In: Transport> Connector<In> for SlicedConnector {
    type Out = Sliced<In>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        let Some(inner) = chained else {
            return Ok(None);
        };
        let active = key_for_uri(details.uri)
            .and_then(|key| self.registry.get(&key))
            .ok_or(ureq::Error::ConnectionFailed)?;
        Ok(Some(Sliced { inner, active }))
    }
}

#[derive(Debug)]
struct Sliced<T> {
    inner: T,
    active: Arc<Active>,
}

impl<T: Transport> Sliced<T> {
    fn check(&self) -> Result<(), ureq::Error> {
        if self.active.control.cancel.is_cancelled() {
            return Err(ureq::Error::Io(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            )));
        }
        if self
            .active
            .control
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ureq::Error::Timeout(Timeout::Global));
        }
        Ok(())
    }
}

impl<T: Transport> Transport for Sliced<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.check()?;
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let limit = match timeout.after {
            UreqDuration::Exact(after) => after.min(self.active.idle),
            UreqDuration::NotHappening => self.active.idle,
        };
        let started = Instant::now();
        loop {
            self.check()?;
            let left = limit.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(ureq::Error::Timeout(timeout.reason));
            }
            let slice = NextTimeout {
                after: UreqDuration::Exact(left.min(SLICE)),
                reason: timeout.reason,
            };
            match self.inner.await_input(slice) {
                // A slice ran out, not the real timeout: look at the flags and wait again.
                Err(ureq::Error::Timeout(_)) => {}
                other => return other,
            }
        }
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

pub(crate) fn build_agent(registry: &Registry) -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    use ureq::unversioned::transport::Connector as _;

    let config = ureq::Agent::config_builder()
        // Redirects are followed by the caller, one checked hop at a time.
        .max_redirects(0)
        .max_redirects_will_error(false)
        // 4xx and 5xx are responses.
        .http_status_as_error(false)
        // No proxy from the environment: the address checks above apply to the real peer.
        .proxy(None)
        // A fresh connection per request: the behaviour under which the catalog was observed,
        // and no hidden retry on a stale pooled connection.
        .max_idle_connections(0)
        .user_agent(ureq::config::AutoHeaderValue::None)
        // The operating system's certificate store, so corporate and antivirus roots work.
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build();
    let connector = ()
        .chain(TcpConnector::default())
        .chain(SlicedConnector {
            registry: registry.clone(),
        })
        .chain(RustlsConnector::default());
    ureq::Agent::with_parts(
        config,
        connector,
        PinnedResolver {
            registry: registry.clone(),
        },
    )
}

/// Maps a `ureq` failure to the error kinds plugins see. The cancellation flag wins: whatever
/// error the interrupted socket produced, the caller asked for it.
pub(crate) fn classify(error: ureq::Error, control: &Control) -> HostError {
    if control.cancel.is_cancelled() {
        return HostError::new(HostErrorKind::Cancelled, "request cancelled");
    }
    let message = error.to_string();
    let kind = match &error {
        ureq::Error::Timeout(_) => HostErrorKind::Timeout,
        ureq::Error::HostNotFound => HostErrorKind::Dns,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => HostErrorKind::Tls,
        ureq::Error::Io(io) => return classify_io(io, control),
        _ => HostErrorKind::Connect,
    };
    HostError::new(kind, message)
}

pub(crate) fn classify_io(error: &io::Error, control: &Control) -> HostError {
    if control.cancel.is_cancelled() {
        return HostError::new(HostErrorKind::Cancelled, "request cancelled");
    }
    // `ureq` carries its own errors through `io::Error` when it implements `Read`.
    if let Some(inner) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
    {
        let kind = match inner {
            ureq::Error::Timeout(_) => HostErrorKind::Timeout,
            ureq::Error::Tls(_) | ureq::Error::Rustls(_) => HostErrorKind::Tls,
            _ => HostErrorKind::Connect,
        };
        return HostError::new(kind, inner.to_string());
    }
    // A failed handshake or an unacceptable certificate surfaces as a bare `rustls::Error`
    // inside the `io::Error` (checked against badssl.com, see the ignored tests).
    if error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some())
    {
        return HostError::new(HostErrorKind::Tls, error.to_string());
    }
    let kind = match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HostErrorKind::Timeout,
        _ => HostErrorKind::Connect,
    };
    HostError::new(kind, error.to_string())
}
