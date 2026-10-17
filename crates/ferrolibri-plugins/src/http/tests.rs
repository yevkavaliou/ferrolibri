use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ferrolibri_core::ErrorKind;
use ferrolibri_core::converter::CancellationToken;

use super::*;
use crate::limits;
use crate::testing::{CATALOG_HOST, Reply, TestServer, opds};
use crate::{HostErrorKind, app_version};

fn ok_server() -> TestServer {
    TestServer::start(|_, _| Reply::ok("text/plain", "hello"))
}

fn session_with(server: &TestServer, config: HttpConfig) -> HttpSession {
    Http::with_config(config).session(
        Identity::Plugin("demo".into()),
        server.allow_list(),
        CancellationToken::default(),
    )
}

fn failure<T: std::fmt::Debug>(result: Result<T, HostError>) -> HostError {
    result.expect_err("the request must fail")
}

#[test]
fn assorted_types_can_cross_threads() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<ResponseStream>();
    send_sync::<HttpSession>();
    send_sync::<Http>();
}

// --- Success and what the client sends -------------------------------------------------------

#[test]
fn fetch_returns_status_headers_and_body() {
    let server = TestServer::start(|_, _| {
        Reply::ok("text/plain; charset=utf-8", "hello").header("X-Test", "1")
    });
    let response = server
        .session()
        .fetch(&server.url("/a?b=1#frag"), &[])
        .unwrap();
    assert_eq!(response.head.status, 200);
    assert!(response.head.is_success());
    assert_eq!(
        response.head.content_type(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(response.head.header("X-TEST"), Some("1"));
    assert_eq!(response.head.content_length(), Some(5));
    assert_eq!(response.head.url.as_str(), server.url("/a?b=1"));
    assert_eq!(response.body, b"hello");
    assert!(response.head.dropped_request_headers.is_empty());
}

#[test]
fn the_user_agent_names_the_app_and_the_plugin() {
    let server = ok_server();
    let http = server.http();
    let token = CancellationToken::default();
    http.session(
        Identity::Plugin("my plugin/1".into()),
        server.allow_list(),
        token.clone(),
    )
    .fetch(&server.url("/"), &[])
    .unwrap();
    http.session(Identity::App, server.allow_list(), token)
        .fetch(&server.url("/"), &[])
        .unwrap();
    let requests = server.requests();
    let version = app_version();
    assert_eq!(
        requests[0].header("user-agent"),
        Some(format!("FerroLibri/{version} plugin/my_plugin_1").as_str())
    );
    assert_eq!(
        requests[1].header("user-agent"),
        Some(format!("FerroLibri/{version}").as_str())
    );
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].header("accept-encoding"), None);
}

#[test]
fn only_white_listed_request_headers_reach_the_server() {
    let server = ok_server();
    let response = server
        .session()
        .fetch(
            &server.url("/"),
            &[
                ("Accept", "application/atom+xml"),
                ("accept-language", "ru"),
                ("Referer", "https://example.org/"),
                ("Range", "bytes=0-9"),
                ("Authorization", "Bearer secret"),
                ("cookie", "a=b"),
                ("X-Forwarded-For", "10.0.0.1"),
            ],
        )
        .unwrap();
    let seen = &server.requests()[0];
    assert_eq!(seen.header("accept"), Some("application/atom+xml"));
    assert_eq!(seen.header("accept-language"), Some("ru"));
    assert_eq!(seen.header("referer"), Some("https://example.org/"));
    assert_eq!(seen.header("range"), Some("bytes=0-9"));
    for name in ["authorization", "cookie", "x-forwarded-for"] {
        assert_eq!(seen.header(name), None, "{name}");
    }
    assert_eq!(
        response.head.dropped_request_headers,
        ["Authorization", "cookie", "X-Forwarded-For"]
    );
}

#[test]
fn cookies_are_neither_exposed_nor_replayed() {
    let server = TestServer::start(|_, _| {
        Reply::ok("text/plain", "x").header("Set-Cookie", "session=1; Path=/")
    });
    let session = server.session();
    let first = session.fetch(&server.url("/"), &[]).unwrap();
    assert_eq!(first.head.header("set-cookie"), None);
    session.fetch(&server.url("/"), &[]).unwrap();
    assert_eq!(server.requests()[1].header("cookie"), None);
}

#[test]
fn error_statuses_with_html_bodies_are_responses_not_errors() {
    let server = TestServer::start(opds::handle);
    let session = server.session();
    for (path, status) in [("/error/502", 502), ("/error/400", 400), ("/nothing", 404)] {
        let response = session.fetch(&server.url(path), &[]).unwrap();
        assert_eq!(response.head.status, status, "{path}");
        assert!(!response.head.is_success());
        assert!(!response.body.is_empty());
    }
    let server = TestServer::start(|_, _| Reply::status(500).body("oops"));
    assert_eq!(
        server
            .session()
            .fetch(&server.url("/"), &[])
            .unwrap()
            .head
            .status,
        500
    );
}

#[test]
fn a_3xx_without_location_is_a_response() {
    let server = TestServer::start(|_, _| Reply::status(302).body("moved?"));
    let response = server.session().fetch(&server.url("/"), &[]).unwrap();
    assert_eq!(response.head.status, 302);
    assert_eq!(response.body, b"moved?");
}

// --- Redirects -------------------------------------------------------------------------------

#[test]
fn a_chain_of_two_302s_to_a_subdomain_of_the_typed_address_is_followed() {
    let server = TestServer::start(opds::handle);
    let response = server
        .session()
        .fetch(&server.url("/b/11/fb2"), &[])
        .unwrap();
    assert_eq!(response.head.status, 200);
    assert_eq!(response.head.url.host_str(), Some("static.catalog.test"));
    assert_eq!(response.head.content_type(), Some("application/zip"));
    assert_eq!(response.body, opds::book_zip(&opds::BOOKS[0]));
}

#[test]
fn a_redirect_to_a_sibling_second_level_domain_is_blocked() {
    let server = TestServer::start(|request, context| match request.path.as_str() {
        "/start" => Reply::redirect(302, &context.url("other.test", "/secret")),
        _ => Reply::ok("text/plain", "secret"),
    });
    let error = failure(server.session().fetch(&server.url("/start"), &[]));
    assert_eq!(error.kind, HostErrorKind::BlockedDomain);
    assert_eq!(
        server.paths(),
        ["/start"],
        "the target must not be contacted"
    );
}

#[test]
fn a_redirect_to_a_loopback_address_is_blocked() {
    for target in [
        "127.0.0.1",
        "localhost",
        "[::1]",
        "[::ffff:127.0.0.1]",
        "0.0.0.0",
    ] {
        let server = TestServer::start({
            let target = target.to_owned();
            move |request, context| match request.path.as_str() {
                "/start" => Reply::redirect(302, &context.url(&target, "/secret")),
                _ => Reply::ok("text/plain", "secret"),
            }
        });
        let error = failure(server.session().fetch(&server.url("/start"), &[]));
        assert_eq!(error.kind, HostErrorKind::BlockedDomain, "{target}");
        assert_eq!(server.paths(), ["/start"], "{target} must not be contacted");
    }
}

#[test]
fn a_redirect_to_a_private_address_is_blocked_even_from_a_typed_host() {
    let server = TestServer::start(|request, _| match request.path.as_str() {
        "/start" => Reply::redirect(302, "http://192.168.1.1/admin"),
        _ => Reply::ok("text/plain", "x"),
    });
    let error = failure(server.session().fetch(&server.url("/start"), &[]));
    assert_eq!(error.kind, HostErrorKind::BlockedDomain);
}

#[test]
fn redirects_are_resolved_against_the_current_url() {
    let server = TestServer::start(|request, context| match request.path.as_str() {
        "/a/b" => Reply::redirect(302, "c?x=1"),
        "/a/c" => Reply::redirect(307, "/d/e"),
        "/d/e" => Reply::redirect(308, &format!("//static.{CATALOG_HOST}:{}/f", context.port)),
        "/f" => Reply::redirect(303, "g"),
        _ => Reply::ok("text/plain", request.target.as_str()),
    });
    let response = server.session().fetch(&server.url("/a/b"), &[]).unwrap();
    assert_eq!(response.body, b"/g");
    assert_eq!(server.paths(), ["/a/b", "/a/c", "/d/e", "/f", "/g"]);
    assert_eq!(server.requests()[1].target, "/a/c?x=1");
}

#[test]
fn up_to_five_redirects_are_followed_and_the_sixth_is_an_error() {
    let server = TestServer::start(|request, _| {
        let step: u32 = request
            .path
            .trim_start_matches("/hop/")
            .parse()
            .unwrap_or(0);
        if request.path == "/endless" {
            Reply::redirect(302, "/endless")
        } else if step < 5 {
            Reply::redirect(302, &format!("/hop/{}", step + 1))
        } else {
            Reply::ok("text/plain", "arrived")
        }
    });
    let session = server.session();
    assert_eq!(
        session.fetch(&server.url("/hop/0"), &[]).unwrap().body,
        b"arrived"
    );
    assert_eq!(server.requests().len(), 6);

    let error = failure(session.fetch(&server.url("/endless"), &[]));
    assert_eq!(error.kind, HostErrorKind::Connect);
    assert!(error.message.contains("redirects"), "{error}");
    // The first request plus five followed redirects; the sixth redirect is not followed.
    assert_eq!(server.requests().len(), 6 + 6);
}

#[test]
fn a_redirect_to_an_invalid_address_is_an_error() {
    let server = TestServer::start(|_, _| Reply::redirect(302, "http://[bad"));
    let error = failure(server.session().fetch(&server.url("/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Connect);
}

// --- Allow-list, schemes, addresses ----------------------------------------------------------

#[test]
fn schemes_other_than_http_and_https_are_blocked_before_any_request() {
    let server = ok_server();
    let session = server.session();
    for url in [
        "ftp://catalog.test/x",
        "file:///etc/passwd",
        "gopher://catalog.test/",
        "data:text/plain,hello",
    ] {
        let error = failure(session.fetch(url, &[]));
        assert_eq!(error.kind, HostErrorKind::BlockedDomain, "{url}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn credentials_in_a_url_are_blocked() {
    let server = ok_server();
    let url = format!("http://user:pw@catalog.test:{}/", server.port());
    assert_eq!(
        failure(server.session().fetch(&url, &[])).kind,
        HostErrorKind::BlockedDomain
    );
    assert!(server.requests().is_empty());
}

#[test]
fn something_that_is_not_a_url_is_a_connect_error() {
    let server = ok_server();
    let session = server.session();
    assert_eq!(
        failure(session.fetch("not a url", &[])).kind,
        HostErrorKind::Connect
    );
    let long = format!("{}/{}", server.url(""), "a".repeat(limits::MAX_URL_CHARS));
    assert_eq!(
        failure(session.fetch(&long, &[])).kind,
        HostErrorKind::Connect
    );
}

#[test]
fn a_host_that_is_not_listed_is_blocked() {
    let server = ok_server();
    let error = failure(server.session().fetch("https://example.org/", &[]));
    assert_eq!(error.kind, HostErrorKind::BlockedDomain);
    assert!(server.requests().is_empty());
}

#[test]
fn an_address_the_user_typed_may_be_a_loopback_or_private_one() {
    let server = ok_server();
    let mut allow = AllowList::new();
    allow.allow_entered_url(&server.ip_url("/opds")).unwrap();
    let session = server
        .http()
        .session(Identity::App, allow, CancellationToken::default());
    let response = session.fetch(&server.ip_url("/x"), &[]).unwrap();
    assert_eq!(response.body, b"hello");
    // Only that address, not its neighbours.
    let error = failure(session.fetch(&format!("http://127.0.0.2:{}/", server.port()), &[]));
    assert_eq!(error.kind, HostErrorKind::BlockedDomain);
}

#[test]
fn a_manifest_domain_must_not_resolve_to_a_private_address() {
    // DNS rebinding: the name is allowed but its address is the loopback one. The server must
    // never be contacted.
    let server = ok_server();
    let mut allow = AllowList::new();
    allow.allow_domain("*.example.org").unwrap();
    let session = server.http().session(
        Identity::Plugin("demo".into()),
        allow,
        CancellationToken::default(),
    );
    let error = failure(session.fetch("https://books.example.org/", &[]));
    assert_eq!(error.kind, HostErrorKind::BlockedDomain);
    assert!(error.message.contains("public address"), "{error}");
    assert!(server.requests().is_empty());
}

#[test]
fn a_manifest_domain_is_not_reached_over_plain_http() {
    let server = ok_server();
    let mut allow = AllowList::new();
    allow.allow_domain("example.org").unwrap();
    let session = server
        .http()
        .session(Identity::App, allow, CancellationToken::default());
    assert_eq!(
        failure(session.fetch("http://example.org/", &[])).kind,
        HostErrorKind::BlockedDomain
    );
}

#[test]
fn a_www_in_the_typed_address_opens_the_subdomains_without_www() {
    let server = ok_server();
    let mut allow = AllowList::new();
    allow
        .allow_entered_url(&format!("http://www.{CATALOG_HOST}:{}/opds", server.port()))
        .unwrap();
    let session = server
        .http()
        .session(Identity::App, allow, CancellationToken::default());
    for url in [
        server.url("/a"),
        server.url_on("www", "/b"),
        server.url_on("static", "/c"),
        server.url_on("cdn.static", "/d"),
    ] {
        assert_eq!(session.fetch(&url, &[]).unwrap().head.status, 200, "{url}");
    }
    let other = format!("http://other.test:{}/", server.port());
    assert_eq!(
        failure(session.fetch(&other, &[])).kind,
        HostErrorKind::BlockedDomain
    );
}

#[test]
fn every_hop_resolves_the_name_once_and_connects_only_to_that_answer() {
    let server = TestServer::start(opds::handle);
    let lookups = Arc::new(AtomicUsize::new(0));
    let mut config = server.config();
    config.resolver = Arc::new({
        let lookups = Arc::clone(&lookups);
        move |_: &str, _: u16| -> io::Result<Vec<IpAddr>> {
            lookups.fetch_add(1, Ordering::SeqCst);
            Ok(vec!["127.0.0.1".parse().unwrap()])
        }
    });
    let response = session_with(&server, config)
        .fetch(&server.url("/b/12/fb2"), &[])
        .unwrap();
    assert_eq!(response.head.status, 200);
    // Three hops: /b/12/fb2, /b/12/fb2/get and the file on the sibling subdomain.
    assert_eq!(lookups.load(Ordering::SeqCst), 3);
}

// --- Failures of the transport ---------------------------------------------------------------

#[test]
fn an_unknown_host_is_a_dns_error() {
    let server = ok_server();
    let error = failure(server.session().fetch(&server.url_on("nxdomain", "/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Dns);

    let mut config = server.config();
    config.resolver = Arc::new(|_: &str, _: u16| -> io::Result<Vec<IpAddr>> { Ok(Vec::new()) });
    let error = failure(session_with(&server, config).fetch(&server.url("/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Dns);
}

#[test]
fn a_lookup_that_never_ends_times_out_and_can_be_cancelled() {
    let server = ok_server();
    let slow = |_: &str, _: u16| -> io::Result<Vec<IpAddr>> {
        thread::sleep(Duration::from_secs(5));
        Ok(vec!["127.0.0.1".parse().unwrap()])
    };

    let mut config = server.config();
    config.connect_timeout = Duration::from_millis(300);
    config.resolver = Arc::new(slow);
    let started = Instant::now();
    let error = failure(session_with(&server, config).fetch(&server.url("/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));

    let mut config = server.config();
    config.resolver = Arc::new(slow);
    let token = CancellationToken::default();
    let session =
        Http::with_config(config).session(Identity::App, server.allow_list(), token.clone());
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        token.cancel();
    });
    let started = Instant::now();
    let error = failure(session.fetch(&server.url("/"), &[]));
    canceller.join().unwrap();
    assert_eq!(error.kind, HostErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn a_refused_connection_is_a_connect_error() {
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let server = ok_server();
    let mut allow = AllowList::new();
    allow
        .allow_entered_url(&format!("http://127.0.0.1:{port}/"))
        .unwrap();
    let session = server
        .http()
        .session(Identity::App, allow, CancellationToken::default());
    let error = failure(session.fetch(&format!("http://127.0.0.1:{port}/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Connect);
}

#[test]
fn an_ipv6_address_the_user_typed_is_reached() {
    use std::io::{Read, Write};
    // IPv6 loopback may be missing (some containers): nothing to check then.
    let Ok(listener) = std::net::TcpListener::bind(("::1", 0)) else {
        return;
    };
    let port = listener.local_addr().unwrap().port();
    let serve = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = [0u8; 1024];
        let _ = stream.read(&mut head).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nv6")
            .unwrap();
    });
    let url = format!("http://[::1]:{port}/");
    let mut allow = AllowList::new();
    allow.allow_entered_url(&url).unwrap();
    let response = Http::new()
        .session(Identity::App, allow, CancellationToken::default())
        .fetch(&url, &[])
        .unwrap();
    serve.join().unwrap();
    assert_eq!(response.body, b"v6");
}

#[test]
fn a_server_that_never_answers_is_a_timeout() {
    let server = TestServer::start(|_, _| Reply::silent());
    let mut config = server.config();
    config.request_timeout = Duration::from_millis(400);
    let started = Instant::now();
    let error = failure(session_with(&server, config).fetch(&server.url("/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_silent_server_hits_the_stall_limit_of_a_download() {
    let server = TestServer::start(|_, _| Reply::silent());
    let mut config = server.config();
    config.stall_timeout = Duration::from_millis(300);
    let started = Instant::now();
    let error = failure(session_with(&server, config).open(&server.url("/"), &[], 1000));
    assert_eq!(error.kind, HostErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn a_download_that_stalls_midway_times_out() {
    let server = TestServer::start(|_, _| {
        Reply::repeated("application/octet-stream", b'x', 100_000).stall_after(1000)
    });
    let mut config = server.config();
    config.stall_timeout = Duration::from_millis(300);
    let mut stream = session_with(&server, config)
        .open(&server.url("/"), &[], limits::MAX_DOWNLOAD_BYTES)
        .unwrap();
    let started = Instant::now();
    let mut buffer = [0u8; 4096];
    let error = loop {
        match stream.read_chunk(&mut buffer) {
            Ok(0) => panic!("the body ended"),
            Ok(_) => {}
            Err(e) => break e,
        }
    };
    assert_eq!(error.kind, HostErrorKind::Timeout);
    assert_eq!(stream.bytes_read(), 1000);
    assert!(started.elapsed() < Duration::from_secs(3));
}

// --- Cancellation ----------------------------------------------------------------------------

#[test]
fn cancelling_stops_a_download_stalled_midway() {
    let server = TestServer::start(|_, _| {
        Reply::repeated("application/octet-stream", b'x', 1_000_000).stall_after(2000)
    });
    let token = CancellationToken::default();
    // Default stall limit (30 s): only the cancellation can end this in time.
    let session = server
        .http()
        .session(Identity::App, server.allow_list(), token.clone());
    let mut stream = session
        .open(&server.url("/"), &[], limits::MAX_DOWNLOAD_BYTES)
        .unwrap();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        token.cancel();
    });
    let started = Instant::now();
    let mut buffer = [0u8; 1024];
    let error = loop {
        match stream.read_chunk(&mut buffer) {
            Ok(0) => panic!("the body ended"),
            Ok(_) => {}
            Err(e) => break e,
        }
    };
    canceller.join().unwrap();
    assert_eq!(error.kind, HostErrorKind::Cancelled);
    assert_eq!(stream.bytes_read(), 2000);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn cancelling_stops_the_wait_for_a_response() {
    let server = TestServer::start(|_, _| Reply::silent());
    let token = CancellationToken::default();
    let session = server
        .http()
        .session(Identity::App, server.allow_list(), token.clone());
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        token.cancel();
    });
    let started = Instant::now();
    let error = failure(session.fetch(&server.url("/"), &[]));
    canceller.join().unwrap();
    assert_eq!(error.kind, HostErrorKind::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_cancelled_token_fails_before_any_request() {
    let server = ok_server();
    let token = CancellationToken::default();
    token.cancel();
    let session = server
        .http()
        .session(Identity::App, server.allow_list(), token);
    assert_eq!(
        failure(session.fetch(&server.url("/"), &[])).kind,
        HostErrorKind::Cancelled
    );
    assert!(server.requests().is_empty());
}

#[test]
fn cancelling_ends_the_wait_in_the_queue_of_a_busy_host() {
    let server = TestServer::start(|request, _| match request.path.as_str() {
        "/hold" => Reply::repeated("application/octet-stream", b'x', 1000).stall_after(10),
        _ => Reply::ok("text/plain", "x"),
    });
    let http = server.http();
    let holder = http.session(
        Identity::App,
        server.allow_list(),
        CancellationToken::default(),
    );
    // An unread stream keeps the host busy.
    let _held = holder.open(&server.url("/hold"), &[], 10_000).unwrap();

    let token = CancellationToken::default();
    let waiting = http.session(Identity::App, server.allow_list(), token.clone());
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        token.cancel();
    });
    let started = Instant::now();
    let error = failure(waiting.fetch(&server.url("/other"), &[]));
    canceller.join().unwrap();
    assert_eq!(error.kind, HostErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(server.paths(), ["/hold"]);
}

// --- Limits ----------------------------------------------------------------------------------

#[test]
fn a_body_over_the_limit_is_too_large_with_or_without_a_declared_length() {
    type Shape = fn(Reply) -> Reply;
    let cases: [(&str, Shape); 3] = [
        ("content-length", |r| r),
        ("chunked", Reply::chunked),
        ("until close", Reply::until_close),
    ];
    for (name, shape) in cases {
        let server = TestServer::start(move |_, _| {
            shape(Reply::repeated("application/octet-stream", b'x', 5000))
        });
        let mut config = server.config();
        config.max_fetch_body = 1000;
        let error = failure(session_with(&server, config).fetch(&server.url("/"), &[]));
        assert_eq!(error.kind, HostErrorKind::TooLarge, "{name}");
    }
}

#[test]
fn a_body_exactly_at_the_limit_passes() {
    let server =
        TestServer::start(|_, _| Reply::repeated("application/octet-stream", b'x', 1000).chunked());
    let mut config = server.config();
    config.max_fetch_body = 1000;
    let response = session_with(&server, config)
        .fetch(&server.url("/"), &[])
        .unwrap();
    assert_eq!(response.body.len(), 1000);
}

#[test]
fn the_default_fetch_limit_is_eight_mebibytes() {
    assert_eq!(HttpConfig::default().max_fetch_body, 8 * 1024 * 1024);
    let server = TestServer::start(|request, _| {
        let len = if request.path == "/exact" {
            limits::MAX_FETCH_BODY_BYTES
        } else {
            limits::MAX_FETCH_BODY_BYTES + 1
        };
        Reply::repeated("application/octet-stream", b'x', len).chunked()
    });
    let session = server.session();
    let exact = session.fetch(&server.url("/exact"), &[]).unwrap();
    assert_eq!(exact.body.len() as u64, limits::MAX_FETCH_BODY_BYTES);
    assert_eq!(
        failure(session.fetch(&server.url("/over"), &[])).kind,
        HostErrorKind::TooLarge
    );
}

#[test]
fn a_stream_enforces_its_own_limit_and_reports_progress() {
    let server = TestServer::start(|_, _| Reply::repeated("application/zip", b'z', 10_000));
    let session = server.session();

    let mut stream = session.open(&server.url("/"), &[], 10_000).unwrap();
    assert_eq!(stream.head().content_length(), Some(10_000));
    let body = stream.read_to_end().unwrap();
    assert_eq!(body.len(), 10_000);
    assert_eq!(stream.bytes_read(), 10_000);

    // A declared length over the limit is refused before any body is read.
    assert_eq!(
        failure(session.open(&server.url("/"), &[], 9_999)).kind,
        HostErrorKind::TooLarge
    );
}

#[test]
fn a_stream_returns_error_statuses_for_the_caller_to_judge() {
    let server = TestServer::start(opds::handle);
    let mut stream = server
        .session()
        .open(&server.url("/error/502"), &[], limits::MAX_DOWNLOAD_BYTES)
        .unwrap();
    assert_eq!(stream.head().status, 502);
    assert!(!stream.head().is_success());
    assert!(!stream.read_to_end().unwrap().is_empty());
}

#[test]
fn the_request_budget_counts_fetches_not_redirect_hops() {
    let server = TestServer::start(opds::handle);
    let session = server.session().with_request_budget(3);
    // One fetch that follows two redirects costs one request.
    session.fetch(&server.url("/b/12/fb2"), &[]).unwrap();
    session.fetch(&server.url("/error/400"), &[]).unwrap();
    session.fetch(&server.url("/error/400"), &[]).unwrap();
    let seen = server.requests().len();
    let error = failure(session.fetch(&server.url("/error/400"), &[]));
    assert_eq!(error.kind, HostErrorKind::TooManyRequests);
    assert_eq!(server.requests().len(), seen, "no request may be made");
    assert_eq!(limits::MAX_FETCH_REQUESTS_PER_CALL, 20);
}

#[test]
fn a_session_deadline_bounds_the_request() {
    let server = TestServer::start(|_, _| Reply::silent());
    let session = server
        .session()
        .with_deadline(Instant::now() + Duration::from_millis(300));
    let started = Instant::now();
    let error = failure(session.fetch(&server.url("/"), &[]));
    assert_eq!(error.kind, HostErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
}

// --- Queue per host --------------------------------------------------------------------------

#[test]
fn requests_to_one_host_never_overlap_and_none_is_rejected() {
    let server =
        TestServer::start(|_, _| Reply::ok("text/plain", "x").delay(Duration::from_millis(80)));
    let http = server.http();
    let workers: Vec<_> = (0..5)
        .map(|_| {
            let http = http.clone();
            let allow = server.allow_list();
            let url = server.url("/");
            thread::spawn(move || {
                http.session(Identity::App, allow, CancellationToken::default())
                    .fetch(&url, &[])
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap().unwrap().head.status, 200);
    }
    assert_eq!(server.requests().len(), 5);
    assert_eq!(server.max_concurrent_for(CATALOG_HOST), 1);
}

#[test]
fn different_hosts_are_served_in_parallel() {
    let server =
        TestServer::start(|_, _| Reply::ok("text/plain", "x").delay(Duration::from_millis(600)));
    let http = server.http();
    let workers: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|sub| {
            let http = http.clone();
            let allow = server.allow_list();
            let url = server.url_on(sub, "/");
            thread::spawn(move || {
                http.session(Identity::App, allow, CancellationToken::default())
                    .fetch(&url, &[])
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap().unwrap();
    }
    assert_eq!(server.max_concurrent(), 2);
    assert_eq!(server.max_concurrent_for(&format!("a.{CATALOG_HOST}")), 1);
}

#[test]
fn requests_to_one_host_are_spaced_by_the_pause() {
    assert_eq!(limits::HOST_REQUEST_GAP, Duration::from_millis(250));
    assert_eq!(HttpConfig::default().host_gap, limits::HOST_REQUEST_GAP);
    let server = ok_server();
    let mut config = server.config();
    config.host_gap = limits::HOST_REQUEST_GAP;
    let session = session_with(&server, config);
    for _ in 0..3 {
        session.fetch(&server.url("/"), &[]).unwrap();
    }
    let requests = server.requests();
    for pair in requests.windows(2) {
        let gap = pair[1].at.duration_since(pair[0].at);
        assert!(gap >= limits::HOST_REQUEST_GAP, "{gap:?}");
    }
}

#[test]
fn the_pause_is_counted_from_the_end_of_the_previous_response() {
    // The first response takes 300 ms to arrive; the second request must start 250 ms after
    // it ended, not after it started.
    let server = TestServer::start(|request, _| match request.path.as_str() {
        "/slow" => Reply::ok("text/plain", "x").delay(Duration::from_millis(300)),
        _ => Reply::ok("text/plain", "x"),
    });
    let mut config = server.config();
    config.host_gap = limits::HOST_REQUEST_GAP;
    let session = session_with(&server, config);
    let started = Instant::now();
    session.fetch(&server.url("/slow"), &[]).unwrap();
    session.fetch(&server.url("/fast"), &[]).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300 + 250));
}

#[test]
fn a_stream_keeps_the_host_busy_until_it_is_dropped() {
    let server = ok_server();
    let http = server.http();
    let first = http.session(
        Identity::App,
        server.allow_list(),
        CancellationToken::default(),
    );
    let stream = first.open(&server.url("/one"), &[], 100).unwrap();

    let second = {
        let http = http.clone();
        let allow = server.allow_list();
        let url = server.url("/two");
        thread::spawn(move || {
            http.session(Identity::App, allow, CancellationToken::default())
                .fetch(&url, &[])
                .map(|r| r.head.status)
        })
    };
    thread::sleep(Duration::from_millis(400));
    assert_eq!(server.paths(), ["/one"], "the second request has to wait");
    drop(stream);
    assert_eq!(second.join().unwrap().unwrap(), 200);
    assert_eq!(server.paths(), ["/one", "/two"]);
}

// --- Conversion to the core error -----------------------------------------------------------

#[test]
fn host_errors_convert_to_core_errors() {
    let network = HostError::new(HostErrorKind::Tls, "bad certificate").into_core("fetch");
    assert!(matches!(network.kind, ErrorKind::Network));
    assert_eq!(network.operation, "fetch");
    let cancelled = HostError::new(HostErrorKind::Cancelled, "stop").into_core("fetch");
    assert!(matches!(cancelled.kind, ErrorKind::Cancelled));
    assert_eq!(
        HostError::new(HostErrorKind::Dns, "no such host").to_string(),
        "dns: no such host"
    );
}

// --- Real TLS (needs the internet; run with `cargo test -p ferrolibri-plugins -- --ignored`) ---

fn real_session(domain: &str) -> HttpSession {
    let mut allow = AllowList::new();
    allow.allow_domain(domain).unwrap();
    Http::new().session(Identity::App, allow, CancellationToken::default())
}

#[test]
#[ignore = "needs the internet"]
fn https_to_a_public_host_verifies_the_certificate() {
    let response = real_session("example.org")
        .fetch("https://example.org/", &[("Accept", "text/html")])
        .unwrap();
    assert_eq!(response.head.status, 200);
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.contains("Example Domain"), "{body}");
}

#[test]
#[ignore = "needs the internet"]
fn a_certificate_that_does_not_verify_is_a_tls_error() {
    for host in [
        "expired.badssl.com",
        "self-signed.badssl.com",
        "wrong.host.badssl.com",
        "untrusted-root.badssl.com",
    ] {
        let error = failure(real_session(host).fetch(&format!("https://{host}/"), &[]));
        assert_eq!(error.kind, HostErrorKind::Tls, "{host}: {error}");
    }
}
