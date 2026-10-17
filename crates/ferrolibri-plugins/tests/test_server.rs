//! The test server as a dependant crate sees it (plan items 6 and 9): through the public API and
//! the `test-support` feature only.

use ferrolibri_core::converter::CancellationToken;
use ferrolibri_plugins::testing::{TestServer, opds};
use ferrolibri_plugins::{HostErrorKind, Identity};

#[test]
fn the_catalog_stand_in_serves_a_search_and_a_download_chain() {
    let server = TestServer::start(opds::handle);
    let session = server.session();

    // Search: the term alone gives a menu; the second link searches books.
    let menu = session
        .fetch(
            &server.url("/opds/search?searchTerm=%D0%B2%D0%BE%D0%B9"),
            &[],
        )
        .unwrap();
    assert_eq!(menu.head.status, 200);
    let menu = String::from_utf8(menu.body).unwrap();
    assert!(menu.contains("searchType=books"));

    // Download: two 302s, the second to the sibling subdomain, ending in an archive.
    let zip = session.fetch(&server.url("/b/12/fb2"), &[]).unwrap();
    assert_eq!(zip.head.content_type(), Some("application/zip"));
    assert_eq!(zip.head.url.host_str(), Some("static.catalog.test"));
    assert_eq!(zip.body, opds::book_zip(&opds::BOOKS[1]));
    assert_eq!(&zip.body[..4], b"PK\x03\x04");
}

#[test]
fn a_plugin_session_cannot_leave_the_typed_address() {
    let server = TestServer::start(opds::handle);
    let session = server.http().session(
        Identity::Plugin("demo".into()),
        server.allow_list(),
        CancellationToken::default(),
    );
    let error = session
        .fetch("http://elsewhere.test/opds", &[])
        .expect_err("a host outside the list");
    assert_eq!(error.kind, HostErrorKind::BlockedDomain);
}
