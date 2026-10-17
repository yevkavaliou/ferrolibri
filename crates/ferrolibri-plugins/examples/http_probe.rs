//! Manual check of the HTTP layer against a real server, mainly of TLS on a new platform
//! (plan item 5, risk R3).
//!
//! ```text
//! cargo run -p ferrolibri-plugins --example http_probe -- https://example.org/
//! cargo run -p ferrolibri-plugins --example http_probe -- --expect tls https://expired.badssl.com/
//! ```
//!
//! The host of the URL is allowed as a manifest domain, so the rules are the strictest ones:
//! `https` on port 443 only, public addresses only. Prints what came back, or the error kind a
//! plugin would see. With `--expect <kind>` the exit code is 0 only when that error occurs.

use std::process::ExitCode;

use ferrolibri_core::converter::CancellationToken;
use ferrolibri_plugins::{AllowList, Http, Identity};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut expect = None;
    let mut url = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--expect" => expect = args.next(),
            _ => url = Some(arg),
        }
    }
    let Some(url) = url else {
        eprintln!("usage: http_probe [--expect <error kind>] <https url>");
        return ExitCode::from(2);
    };
    let Some(host) = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
    else {
        eprintln!("not a URL with a host: {url}");
        return ExitCode::from(2);
    };
    let mut allow = AllowList::new();
    if let Err(e) = allow.allow_domain(&host) {
        eprintln!("{host} cannot be a manifest domain: {e}");
        return ExitCode::from(2);
    }
    let session = Http::new().session(Identity::App, allow, CancellationToken::default());
    let result = session.fetch(&url, &[("Accept", "text/html")]);
    match (result, expect) {
        (Ok(response), None) => {
            println!(
                "{} {} bytes {} ({})",
                response.head.status,
                response.body.len(),
                response.head.content_type().unwrap_or("-"),
                response.head.url
            );
            ExitCode::SUCCESS
        }
        (Ok(response), Some(kind)) => {
            eprintln!("expected {kind}, got status {}", response.head.status);
            ExitCode::FAILURE
        }
        (Err(error), Some(kind)) if error.kind.as_str() == kind => {
            println!("expected failure: {error}");
            ExitCode::SUCCESS
        }
        (Err(error), _) => {
            eprintln!("failed: {error}");
            ExitCode::FAILURE
        }
    }
}
