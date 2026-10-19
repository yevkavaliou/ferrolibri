//! A small scriptable HTTP/1.1 server on a loopback port.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ferrolibri_core::converter::CancellationToken;

use crate::{AllowList, Http, HttpConfig, HttpSession, Identity, NameResolver};

/// The name tests use for the server; [`TestServer::resolver`] maps it (and any other name) to
/// the loopback address.
pub const CATALOG_HOST: &str = "catalog.test";

/// A request as the server saw it.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// Path and query, as sent.
    pub target: String,
    pub path: String,
    pub query: String,
    /// The `Host` header without the port, lower-case.
    pub host: String,
    /// The `Host` header as sent (`name:port`).
    pub host_header: String,
    /// Lower-case names, in the order sent.
    pub headers: Vec<(String, String)>,
    /// When the head of the request had been read.
    pub at: Instant,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The first query parameter called `key`, percent-decoded.
    pub fn query_param(&self, key: &str) -> Option<String> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }
}

/// What a handler knows about the server it runs in.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub port: u16,
}

impl Context {
    /// `http://{host}:{port}{path}`.
    pub fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framing {
    ContentLength,
    Chunked,
    /// No length at all; the end of the body is the end of the connection.
    UntilClose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    /// Send the head and this many body bytes, then hold the connection open without a word.
    StallAfter(usize),
    /// Read the request and never answer.
    Silent,
}

#[derive(Clone, Debug)]
enum Body {
    Bytes(Vec<u8>),
    /// `len` copies of a byte, produced as it is written (so a 200 MiB body costs no memory).
    Repeat(u8, u64),
}

impl Body {
    fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::Repeat(_, len) => *len,
        }
    }
}

/// A scripted answer.
#[derive(Clone, Debug)]
pub struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
    framing: Framing,
    delay: Duration,
    fault: Fault,
}

impl Reply {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Body::Bytes(Vec::new()),
            framing: Framing::ContentLength,
            delay: Duration::ZERO,
            fault: Fault::None,
        }
    }

    pub fn ok(content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self::status(200)
            .header("Content-Type", content_type)
            .body(body)
    }

    pub fn html(status: u16, body: &str) -> Self {
        Self::status(status)
            .header("Content-Type", "text/html")
            .body(body.as_bytes().to_vec())
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self::status(status).header("Location", location)
    }

    /// `len` bytes of `byte` with a correct `Content-Length`.
    pub fn repeated(content_type: &str, byte: u8, len: u64) -> Self {
        let mut reply = Self::status(200).header("Content-Type", content_type);
        reply.body = Body::Repeat(byte, len);
        reply
    }

    /// Reads the request and never answers.
    pub fn silent() -> Self {
        let mut reply = Self::status(200);
        reply.fault = Fault::Silent;
        reply
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Body::Bytes(body.into());
        self
    }

    /// Waits before answering.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// `Transfer-Encoding: chunked` instead of `Content-Length`.
    pub fn chunked(mut self) -> Self {
        self.framing = Framing::Chunked;
        self
    }

    /// Neither length nor chunks: the body ends when the server closes the connection.
    pub fn until_close(mut self) -> Self {
        self.framing = Framing::UntilClose;
        self
    }

    /// Sends the head (with the full `Content-Length`) and the first `bytes` of the body, then
    /// goes silent while keeping the connection open.
    pub fn stall_after(mut self, bytes: usize) -> Self {
        self.fault = Fault::StallAfter(bytes);
        self
    }
}

type Handler = dyn Fn(&Request, &Context) -> Reply + Send + Sync + 'static;

struct Shared {
    handler: Box<Handler>,
    context: Context,
    requests: Mutex<Vec<Request>>,
    /// Host → (requests being served now, the most there ever were at once).
    in_flight: Mutex<HashMap<String, (usize, usize)>>,
    stop: AtomicBool,
}

/// Serves scripted replies on `127.0.0.1:<free port>` until dropped.
///
/// The connection is closed after every reply. Requests are recorded; the server also tracks
/// how many requests were being served at once per `Host`.
pub struct TestServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start(handler: impl Fn(&Request, &Context) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind a loopback port");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let addr = listener.local_addr().expect("local address");
        let shared = Arc::new(Shared {
            handler: Box::new(handler),
            context: Context { port: addr.port() },
            requests: Mutex::default(),
            in_flight: Mutex::default(),
            stop: AtomicBool::new(false),
        });
        let accept = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                while !shared.stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let shared = Arc::clone(&shared);
                            std::thread::spawn(move || {
                                let _ = serve(stream, &shared);
                            });
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Self {
            addr,
            shared,
            accept: Some(accept),
        }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://catalog.test:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        self.shared.context.url(CATALOG_HOST, path)
    }

    /// The same on a subdomain: `http://<sub>.catalog.test:<port><path>`.
    pub fn url_on(&self, subdomain: &str, path: &str) -> String {
        self.shared
            .context
            .url(&format!("{subdomain}.{CATALOG_HOST}"), path)
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn ip_url(&self, path: &str) -> String {
        self.shared.context.url("127.0.0.1", path)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Paths of the requests received so far, in arrival order.
    pub fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.path).collect()
    }

    /// The most requests ever served at the same time for this `Host` name.
    pub fn max_concurrent_for(&self, host: &str) -> usize {
        self.shared
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&host.to_ascii_lowercase())
            .map_or(0, |(_, max)| *max)
    }

    /// The most requests ever served at the same time, whatever the `Host`.
    pub fn max_concurrent(&self) -> usize {
        let map = self
            .shared
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Per-host maxima do not add up; recompute from the global counter instead.
        map.get("*").map_or(0, |(_, max)| *max)
    }

    /// Maps every name to this server (the loopback address); names starting with `nxdomain.`
    /// do not exist.
    pub fn resolver(&self) -> Arc<dyn NameResolver> {
        Arc::new(LoopbackResolver)
    }

    /// A client for tests: no pause between requests to one host (the pause has tests of its own)
    /// and names resolved to this server.
    pub fn config(&self) -> HttpConfig {
        HttpConfig {
            host_gap: Duration::ZERO,
            resolver: self.resolver(),
            ..HttpConfig::default()
        }
    }

    pub fn http(&self) -> Http {
        Http::with_config(self.config())
    }

    /// What a user who typed `http://catalog.test:<port>/` into a plugin setting allows
    ///: the host, its subdomains, any scheme and port, private addresses.
    pub fn allow_list(&self) -> AllowList {
        let mut list = AllowList::new();
        list.allow_entered_url(&self.url("/"))
            .expect("the test address is valid");
        list
    }

    /// A session of the plugin `test` with [`TestServer::allow_list`].
    pub fn session(&self) -> HttpSession {
        self.http().session(
            Identity::Plugin("test".into()),
            self.allow_list(),
            CancellationToken::default(),
        )
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// Every name resolves to `127.0.0.1`, except `nxdomain.*`.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoopbackResolver;

impl NameResolver for LoopbackResolver {
    fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        if host.starts_with("nxdomain.") {
            return Err(std::io::Error::new(ErrorKind::NotFound, "no such host"));
        }
        Ok(vec![Ipv4Addr::LOCALHOST.into()])
    }
}

fn pause(duration: Duration, shared: &Shared) {
    let end = Instant::now() + duration;
    while Instant::now() < end && !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10).min(end - Instant::now()));
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        head.push(byte[0]);
        if head.len() > 32 * 1024 {
            return Err(ErrorKind::InvalidData.into());
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let target = first.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let host_header = headers
        .iter()
        .find(|(n, _)| n == "host")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let host = host_header
        .rsplit_once(':')
        .map_or(host_header.as_str(), |(h, _)| h)
        .to_ascii_lowercase();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    Ok(Request {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        target,
        host,
        host_header,
        headers,
        at: Instant::now(),
    })
}

/// Counts the request as being served for its host and overall (key `*`) until dropped.
struct Counted<'a> {
    shared: &'a Shared,
    host: String,
}

impl<'a> Counted<'a> {
    fn new(shared: &'a Shared, host: &str) -> Self {
        let mut map = shared
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for key in [host, "*"] {
            let entry = map.entry(key.to_owned()).or_default();
            entry.0 += 1;
            entry.1 = entry.1.max(entry.0);
        }
        Self {
            shared,
            host: host.to_owned(),
        }
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        let mut map = self
            .shared
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for key in [self.host.as_str(), "*"] {
            if let Some(entry) = map.get_mut(key) {
                entry.0 -= 1;
            }
        }
    }
}

fn serve(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let request = read_request(&mut stream)?;
    shared
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request.clone());
    let _counted = Counted::new(shared, &request.host);
    let reply = (shared.handler)(&request, &shared.context);
    pause(reply.delay, shared);
    if reply.fault == Fault::Silent {
        return hold(stream, shared);
    }
    write_head(&mut stream, &reply)?;
    let limit = match reply.fault {
        Fault::StallAfter(n) => n as u64,
        _ => u64::MAX,
    };
    write_body(&mut stream, &reply, limit, shared)?;
    if matches!(reply.fault, Fault::StallAfter(_)) {
        return hold(stream, shared);
    }
    stream.flush()
}

/// Keeps the connection open and quiet until the client hangs up or the server stops.
fn hold(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut byte = [0u8; 1];
    while !shared.stop.load(Ordering::Acquire) {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Status",
    }
}

fn write_head(stream: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    match reply.framing {
        Framing::ContentLength => {
            head.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
        }
        Framing::Chunked => head.push_str("Transfer-Encoding: chunked\r\n"),
        Framing::UntilClose => {}
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())
}

fn write_body(
    stream: &mut TcpStream,
    reply: &Reply,
    limit: u64,
    shared: &Shared,
) -> std::io::Result<()> {
    const PIECE: u64 = 8 * 1024;
    let total = reply.body.len().min(limit);
    let mut sent = 0u64;
    let mut buffer = Vec::new();
    while sent < total && !shared.stop.load(Ordering::Acquire) {
        let size = PIECE.min(total - sent);
        buffer.clear();
        match &reply.body {
            Body::Bytes(bytes) => {
                buffer.extend_from_slice(&bytes[sent as usize..(sent + size) as usize]);
            }
            Body::Repeat(byte, _) => buffer.resize(size as usize, *byte),
        }
        if reply.framing == Framing::Chunked {
            stream.write_all(format!("{:x}\r\n", buffer.len()).as_bytes())?;
            stream.write_all(&buffer)?;
            stream.write_all(b"\r\n")?;
        } else {
            stream.write_all(&buffer)?;
        }
        sent += size;
    }
    if reply.framing == Framing::Chunked && sent == reply.body.len() {
        stream.write_all(b"0\r\n\r\n")?;
    }
    stream.flush()
}
