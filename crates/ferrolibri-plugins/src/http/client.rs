use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ferrolibri_core::converter::CancellationToken;
use url::{Host, Url};

use super::addr::vet_addresses;
use super::allow::{Access, AllowList};
use super::control::Control;
use super::error::HostError;
use super::gate::{HostGate, Permit};
use super::headers::sanitize;
use super::net::{self, Active, NameResolver, Registry, SystemResolver};
use crate::{HostErrorKind, app_version, limits};

/// Settings of the HTTP client. The defaults are the product's limits (`crate::limits`).
#[derive(Clone)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    /// One `fetch`: connecting, redirects, headers and body together.
    pub request_timeout: Duration,
    /// A read that gets no byte for this long fails with `Timeout`.
    pub stall_timeout: Duration,
    /// Pause between two requests to the same host.
    pub host_gap: Duration,
    pub max_redirects: u32,
    /// Largest body `fetch` returns.
    pub max_fetch_body: u64,
    pub resolver: Arc<dyn NameResolver>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout: limits::CONNECT_TIMEOUT,
            request_timeout: limits::REQUEST_TIMEOUT,
            stall_timeout: limits::STALL_TIMEOUT,
            host_gap: limits::HOST_REQUEST_GAP,
            max_redirects: limits::MAX_REDIRECTS,
            max_fetch_body: limits::MAX_FETCH_BODY_BYTES,
            resolver: Arc::new(SystemResolver),
        }
    }
}

impl std::fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpConfig")
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("stall_timeout", &self.stall_timeout)
            .field("host_gap", &self.host_gap)
            .field("max_redirects", &self.max_redirects)
            .field("max_fetch_body", &self.max_fetch_body)
            .finish_non_exhaustive()
    }
}

/// Who is asking; it only decides the `User-Agent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    /// The app itself (installing a plugin, downloading a book).
    App,
    /// A plugin, by manifest id.
    Plugin(String),
}

impl Identity {
    fn user_agent(&self) -> String {
        match self {
            Self::App => format!("FerroLibri/{}", app_version()),
            Self::Plugin(id) => {
                let id: String = id
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                format!("FerroLibri/{} plugin/{id}", app_version())
            }
        }
    }
}

/// The one HTTP client of the app. Cheap to clone; clones share the per-host queues.
#[derive(Clone)]
pub struct Http {
    shared: Arc<Shared>,
}

struct Shared {
    agent: ureq::Agent,
    registry: Registry,
    gate: Arc<HostGate>,
    config: HttpConfig,
    /// Requests (`fetch` or `open`) started on this client; redirects of one request are not counted.
    started: AtomicU64,
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("config", &self.shared.config)
            .finish_non_exhaustive()
    }
}

impl Http {
    pub fn new() -> Self {
        Self::with_config(HttpConfig::default())
    }

    /// How many requests (`fetch` or `open`, one per call, whatever the redirects) have started
    /// on this client and its clones. A request refused by the budget or the cancellation does
    /// not count. For diagnostics, such as the plugin checker.
    pub fn requests_started(&self) -> u64 {
        self.shared.started.load(Ordering::Relaxed)
    }

    pub fn with_config(config: HttpConfig) -> Self {
        let registry = Registry::default();
        Self {
            shared: Arc::new(Shared {
                agent: net::build_agent(&registry),
                gate: Arc::new(HostGate::new(config.host_gap)),
                registry,
                config,
                started: AtomicU64::new(0),
            }),
        }
    }

    /// A handle for one plugin call (or one app operation): its allow-list, cancellation and,
    /// optionally, a request budget and a deadline.
    pub fn session(
        &self,
        identity: Identity,
        allow: AllowList,
        cancel: CancellationToken,
    ) -> HttpSession {
        HttpSession {
            http: self.clone(),
            user_agent: identity.user_agent(),
            allow,
            cancel,
            deadline: None,
            budget: None,
        }
    }
}

/// What the server answered, without the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    /// The URL that produced this response, after redirects.
    pub url: Url,
    /// Lower-case names; `set-cookie` is not exposed.
    pub headers: Vec<(String, String)>,
    /// Names of request headers that were not sent (not on the white list, repeated or invalid).
    pub dropped_request_headers: Vec<String>,
}

impl ResponseHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.trim().parse().ok()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchResponse {
    pub head: ResponseHead,
    pub body: Vec<u8>,
}

/// A response whose body is read piece by piece. Dropping it abandons the rest of the body.
///
/// The host stays occupied (no other request to it starts) until the body has been read to the
/// end, a read fails, or the stream is dropped.
pub struct ResponseStream {
    head: ResponseHead,
    reader: ureq::BodyReader<'static>,
    control: Control,
    limit: u64,
    read: u64,
    /// Held while the body may still be read; released at its end or at the first error.
    active: Option<ActiveRequest>,
}

impl ResponseStream {
    pub fn head(&self) -> &ResponseHead {
        &self.head
    }

    /// Bytes delivered so far.
    pub fn bytes_read(&self) -> u64 {
        self.read
    }

    /// Reads up to `buf.len()` bytes; `Ok(0)` is the end of the body. Fails with `TooLarge` as
    /// soon as more than the limit has been delivered, `Cancelled` when the session's token is
    /// cancelled (within about 100 ms even if the server is silent) and `Timeout` when no byte
    /// arrives for `stall_timeout`.
    pub fn read_chunk(&mut self, buf: &mut [u8]) -> Result<usize, HostError> {
        let result = self.read_inner(buf);
        if !matches!(result, Ok(n) if n > 0) {
            // The end of the body, or a connection that is no good any more: the host is free.
            self.active = None;
        }
        result
    }

    fn read_inner(&mut self, buf: &mut [u8]) -> Result<usize, HostError> {
        self.control.check()?;
        let n = match self.reader.read(buf) {
            Ok(n) => n,
            Err(e) => return Err(net::classify_io(&e, &self.control)),
        };
        self.read += n as u64;
        if self.read > self.limit {
            return Err(too_large(self.limit));
        }
        Ok(n)
    }

    /// Reads the rest of the body into memory.
    pub fn read_to_end(&mut self) -> Result<Vec<u8>, HostError> {
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let n = self.read_chunk(&mut chunk)?;
            if n == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
}

impl std::fmt::Debug for ResponseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseStream")
            .field("head", &self.head)
            .field("read", &self.read)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

fn too_large(limit: u64) -> HostError {
    HostError::new(
        HostErrorKind::TooLarge,
        format!("the response is larger than {limit} bytes"),
    )
}

/// Registration of the vetted addresses of a request plus its place in the host's queue.
/// Releases both when dropped.
struct ActiveRequest {
    registry: Registry,
    key: String,
    _permit: Permit,
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.registry.remove(&self.key);
    }
}

/// HTTP for one plugin call or app operation. Every request checks the allow-list (also after
/// each redirect), the address it connects to, the per-call request budget and the cancellation.
pub struct HttpSession {
    http: Http,
    user_agent: String,
    allow: AllowList,
    cancel: CancellationToken,
    deadline: Option<Instant>,
    budget: Option<AtomicU32>,
}

impl HttpSession {
    /// At most `max` requests (`fetch` or `open`) in this session; the next one fails with
    /// `TooManyRequests` without touching the network (D6: 20 per plugin call).
    pub fn with_request_budget(mut self, max: u32) -> Self {
        self.budget = Some(AtomicU32::new(max));
        self
    }

    /// Everything started in this session must end before `deadline` (a plugin call's time).
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// GET `url` and returns the response whatever its status; the body is limited to
    /// `max_fetch_body` (8 MiB by default). `headers` is filtered through the white list.
    pub fn fetch(&self, url: &str, headers: &[(&str, &str)]) -> Result<FetchResponse, HostError> {
        let config = &self.http.shared.config;
        let control = self.control(Some(Instant::now() + config.request_timeout));
        let mut stream = self.start(url, headers, control, config.max_fetch_body)?;
        let body = stream.read_to_end()?;
        Ok(FetchResponse {
            head: stream.head.clone(),
            body,
        })
    }

    /// GET `url` and returns as soon as the headers are in. There is no total time limit, only
    /// the stall limit; `max_bytes` caps the body (see `limits::MAX_DOWNLOAD_BYTES`). Any status
    /// is returned: the caller looks at `head().status` before it writes the body anywhere.
    pub fn open(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        max_bytes: u64,
    ) -> Result<ResponseStream, HostError> {
        self.start(url, headers, self.control(None), max_bytes)
    }

    fn control(&self, deadline: Option<Instant>) -> Control {
        Control {
            cancel: self.cancel.clone(),
            deadline: self.deadline,
        }
        .bounded_by(deadline)
    }

    fn take_budget(&self) -> Result<(), HostError> {
        let Some(budget) = &self.budget else {
            return Ok(());
        };
        budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                left.checked_sub(1)
            })
            .map(|_| ())
            .map_err(|_| {
                HostError::new(
                    HostErrorKind::TooManyRequests,
                    "the limit of requests for this call is used up",
                )
            })
    }

    fn start(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        control: Control,
        limit: u64,
    ) -> Result<ResponseStream, HostError> {
        control.check()?;
        self.take_budget()?;
        let shared = &self.http.shared;
        shared.started.fetch_add(1, Ordering::Relaxed);
        let config = &shared.config;
        let sanitized = sanitize(headers);
        let mut current = parse_url(url)?;
        let mut redirects = 0;
        loop {
            control.check()?;
            let access = self.allow.check(&current)?;
            let active = self.activate(&current, access, &control)?;

            let mut request = shared.agent.get(current.as_str());
            for (name, value) in &sanitized.kept {
                request = request.header(*name, value.as_str());
            }
            request = request.header("User-Agent", self.user_agent.as_str());
            let mut request_config =
                request
                    .config()
                    .timeout_connect(Some(match control.remaining() {
                        Some(left) => config.connect_timeout.min(left),
                        None => config.connect_timeout,
                    }));
            if let Some(left) = control.remaining() {
                request_config = request_config.timeout_global(Some(left));
            }
            let response = request_config
                .build()
                .call()
                .map_err(|e| net::classify(e, &control))?;

            let (parts, body) = response.into_parts();
            let status = parts.status.as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(location) = parts.headers.get("location")
            {
                drop(body);
                drop(active);
                redirects += 1;
                if redirects > config.max_redirects {
                    return Err(HostError::new(
                        HostErrorKind::Connect,
                        format!("more than {} redirects", config.max_redirects),
                    ));
                }
                let location = location.to_str().map_err(|_| {
                    HostError::new(
                        HostErrorKind::Connect,
                        "a redirect with an unreadable location",
                    )
                })?;
                current = current.join(location).map_err(|_| {
                    HostError::new(
                        HostErrorKind::Connect,
                        format!("a redirect to an invalid address: {location}"),
                    )
                })?;
                continue;
            }

            let mut head = ResponseHead {
                status,
                url: current,
                headers: parts
                    .headers
                    .iter()
                    .filter(|(name, _)| name.as_str() != "set-cookie")
                    .map(|(name, value)| {
                        (
                            name.as_str().to_owned(),
                            String::from_utf8_lossy(value.as_bytes()).into_owned(),
                        )
                    })
                    .collect(),
                dropped_request_headers: sanitized.dropped.clone(),
            };
            head.url.set_fragment(None);
            if head.content_length().is_some_and(|length| length > limit) {
                return Err(too_large(limit));
            }
            return Ok(ResponseStream {
                head,
                reader: body.into_reader(),
                control,
                limit,
                read: 0,
                active: Some(active),
            });
        }
    }

    /// Takes the host's turn, resolves and vets its addresses and registers them for the
    /// connection that is about to be made.
    fn activate(
        &self,
        url: &Url,
        access: Access,
        control: &Control,
    ) -> Result<ActiveRequest, HostError> {
        let shared = &self.http.shared;
        let config = &shared.config;
        let host = url
            .host()
            .ok_or_else(|| HostError::blocked("the URL has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| HostError::blocked("the URL has no port"))?;
        let host_text = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let permit = shared
            .gate
            .acquire(host_text.trim_end_matches('.'), control)?;

        let addresses: Vec<IpAddr> = match host {
            Host::Ipv4(ip) => vec![ip.into()],
            Host::Ipv6(ip) => vec![ip.into()],
            Host::Domain(name) => net::resolve_name(
                &config.resolver,
                name,
                port,
                control,
                config.connect_timeout,
            )?,
        };
        let addresses = vet_addresses(addresses, access == Access::Entered).ok_or_else(|| {
            HostError::blocked(format!("{host_text} does not resolve to a public address"))
        })?;

        let key = net::key_for(&host_text, port);
        shared.registry.insert(
            key.clone(),
            Active {
                addresses: addresses
                    .into_iter()
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect(),
                control: control.clone(),
                idle: config.stall_timeout,
            },
        );
        Ok(ActiveRequest {
            registry: shared.registry.clone(),
            key,
            _permit: permit,
        })
    }
}

fn parse_url(text: &str) -> Result<Url, HostError> {
    if text.chars().count() > limits::MAX_URL_CHARS {
        return Err(HostError::new(
            HostErrorKind::Connect,
            "the URL is too long",
        ));
    }
    let mut url = Url::parse(text).map_err(|e| {
        HostError::new(
            HostErrorKind::Connect,
            format!("not a valid URL ({e}): {text}"),
        )
    })?;
    url.set_fragment(None);
    Ok(url)
}
