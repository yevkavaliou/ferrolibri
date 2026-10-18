//! The example plugins in `examples/plugins/` run in the real sandbox against a local server
//! that answers with responses captured from the real services (`tests/fixtures/examples/`).
//! Nothing here reaches the internet; the check against the live services is done by hand with
//! `ferrolibri-plugin-check` (plan item 14).
//!
//! The Google Books response is the exception: it is written by hand from the API documentation
//! (see `_fixture` in the file), because the service refused anonymous requests when it was made.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use ferrolibri_core::converter::CancellationToken;
use ferrolibri_plugins::{
    Archive, LoadedPlugin, Manifest, ParsedPage, PluginFunction, PluginRuntime, SettingValue,
    app_version, parse_download_spec, parse_search_page,
    testing::{Reply, TestServer},
};
use serde_json::{Value, json};

/// Every domain the examples may name (D12): the three legal sources and nothing else.
const POLICY_DOMAINS: &[&str] = &[
    "openlibrary.org",
    "covers.openlibrary.org",
    "archive.org",
    "*.archive.org",
    "www.googleapis.com",
    "books.google.com",
    "www.gutenberg.org",
    // The mock names a domain that never resolves (RFC 2606): it reaches no site.
    "mock.invalid",
];

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/examples")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The example `name` with the given settings stored as the settings form would.
fn load(name: &str, stored: &[(&str, &str)]) -> LoadedPlugin {
    let dir = examples_dir().join(name);
    let manifest = Manifest::parse(&std::fs::read_to_string(dir.join("plugin.toml")).unwrap())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let source = std::fs::read_to_string(dir.join(&manifest.entry)).unwrap();
    LoadedPlugin {
        manifest,
        source,
        settings: stored
            .iter()
            .map(|(key, value)| ((*key).to_owned(), SettingValue::Text((*value).to_owned())))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn call(
    plugin: &LoadedPlugin,
    server: &TestServer,
    function: PluginFunction,
    argument: &Value,
) -> Result<Value, String> {
    PluginRuntime::new(server.http())
        .call(plugin, function, argument, &CancellationToken::default())
        .map_err(|e| e.to_string())
}

fn search(plugin: &LoadedPlugin, server: &TestServer, query: Value) -> Result<Value, String> {
    call(plugin, server, PluginFunction::Search, &query)
}

/// What the app does with the answer: the contract, with the plugin's real allow-list.
fn accept(plugin: &LoadedPlugin, answer: &Value) -> ParsedPage {
    let allow = plugin.allow_list().unwrap();
    parse_search_page(&answer.to_string(), &|host| allow.allows_host(host)).unwrap()
}

fn sole_query(server: &TestServer) -> ferrolibri_plugins::testing::Request {
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one request is expected: {requests:?}");
    requests.into_iter().next().unwrap()
}

// --- All examples --------------------------------------------------------------------------

#[test]
fn every_example_is_a_compatible_plugin_for_one_of_the_legal_sources_or_the_mock() {
    let mut seen = BTreeSet::new();
    for entry in std::fs::read_dir(examples_dir()).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let folder = dir.file_name().unwrap().to_string_lossy().into_owned();
        let manifest = Manifest::parse(&std::fs::read_to_string(dir.join("plugin.toml")).unwrap())
            .unwrap_or_else(|e| panic!("{folder}: {e}"));
        assert!(
            manifest.compatibility(&app_version()).is_compatible(),
            "{folder} is not compatible"
        );
        assert_eq!(manifest.id, folder, "the folder is named after the id");
        assert!(manifest.provides.iter().any(|p| p == "search"));
        assert!(dir.join(&manifest.entry).is_file(), "{folder}: no script");
        assert!(dir.join("README.md").is_file(), "{folder}: no README.md");
        for domain in &manifest.network.domains {
            assert!(
                POLICY_DOMAINS.contains(&domain.as_str()),
                "{folder}: {domain} is not a source the examples may use (D12)"
            );
        }
        seen.insert(folder);
    }
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        [
            "google-books",
            "mock-library",
            "opds-catalog",
            "open-library"
        ]
    );
}

// --- Open Library --------------------------------------------------------------------------

fn open_library_server() -> TestServer {
    TestServer::start(|request, _| match request.path.as_str() {
        "/search.json" => Reply::ok("application/json", fixture("open-library-search.json")),
        _ => Reply::status(404),
    })
}

#[test]
fn open_library_maps_a_real_answer_and_asks_for_the_right_page() {
    let server = open_library_server();
    let plugin = load("open-library", &[("base_url", &server.url("/"))]);
    let answer = search(&plugin, &server, json!({ "text": "dracula", "page": 1 })).unwrap();

    let request = sole_query(&server);
    assert_eq!(request.query_param("q").as_deref(), Some("dracula"));
    assert_eq!(request.query_param("page").as_deref(), Some("1"));
    assert_eq!(request.query_param("limit").as_deref(), Some("20"));
    assert!(
        request
            .query_param("fields")
            .unwrap()
            .contains("author_name")
    );

    // Hits with 30, 57 and 29 author names are in the capture: none may be dropped.
    let parsed = accept(&plugin, &answer);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.page.hits.len(), 6);
    assert!(parsed.page.hits.iter().all(|hit| hit.authors.len() <= 5));
    // Six of 2575 arrived: there is a next page, and its cursor is its number.
    assert_eq!(parsed.page.next_cursor.as_deref(), Some("2"));

    let first = &parsed.page.hits[0];
    assert_eq!(first.title, "Dracula");
    assert_eq!(first.authors, ["Bram Stoker"]);
    assert_eq!(first.year, Some(1897));
    assert!(first.id.starts_with("/works/OL"));
    assert_eq!(
        first.cover_url.as_deref(),
        Some("https://covers.openlibrary.org/b/id/12216503-M.jpg")
    );
    assert_eq!(first.page_url, Some(server.url(&first.id)));
    assert!(first.offers.is_empty(), "Open Library offers no downloads");
    assert!(first.tags.len() <= 10);
}

#[test]
fn open_library_continues_from_the_cursor_and_stops_at_the_end() {
    let server = open_library_server();
    let plugin = load("open-library", &[("base_url", &server.url("/"))]);
    search(
        &plugin,
        &server,
        json!({ "text": "dracula", "page": 2, "cursor": "7" }),
    )
    .unwrap();
    assert_eq!(
        sole_query(&server).query_param("page").as_deref(),
        Some("7")
    );

    let last = TestServer::start(|_, _| {
        Reply::ok(
            "application/json",
            r#"{"numFound":1,"start":0,"docs":[{"key":"/works/OL1W","title":"Only"}]}"#,
        )
    });
    let plugin = load("open-library", &[("base_url", &last.url("/"))]);
    let answer = search(&plugin, &last, json!({ "text": "only", "page": 1 })).unwrap();
    let parsed = accept(&plugin, &answer);
    assert_eq!(parsed.page.hits.len(), 1);
    assert_eq!(parsed.page.next_cursor, None);
    assert_eq!(parsed.page.hits[0].cover_url, None);
}

#[test]
fn open_library_reports_a_refusal_and_a_failure_in_words() {
    for (status, expected) in [(429, "slow down"), (503, "status 503")] {
        let server = TestServer::start(move |_, _| Reply::status(status));
        let plugin = load("open-library", &[("base_url", &server.url("/"))]);
        let error = search(&plugin, &server, json!({ "text": "x", "page": 1 })).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

// --- Google Books --------------------------------------------------------------------------

fn google_server() -> TestServer {
    TestServer::start(|request, _| match request.path.as_str() {
        "/books/v1/volumes" => Reply::ok("application/json", fixture("google-books-search.json")),
        _ => Reply::status(404),
    })
}

#[test]
fn google_books_maps_the_volumes_and_leaves_out_what_it_cannot_show() {
    let server = google_server();
    let plugin = load("google-books", &[("base_url", &server.url("/"))]);
    let answer = search(
        &plugin,
        &server,
        json!({ "text": "dracula stoker", "page": 1 }),
    )
    .unwrap();

    let request = sole_query(&server);
    assert_eq!(request.query_param("q").as_deref(), Some("dracula stoker"));
    assert_eq!(request.query_param("printType").as_deref(), Some("books"));
    assert_eq!(request.query_param("maxResults").as_deref(), Some("20"));
    assert_eq!(request.query_param("startIndex").as_deref(), Some("0"));
    assert_eq!(request.query_param("key"), None, "no key was entered");

    let parsed = accept(&plugin, &answer);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    // The volume without a title is left out by the plugin.
    assert_eq!(parsed.page.hits.len(), 2);
    assert_eq!(
        parsed.page.next_cursor, None,
        "two of twenty is the last page"
    );

    let book = &parsed.page.hits[0];
    assert_eq!(book.id, "ix0LAAAAYAAJ");
    assert_eq!(book.title, "Dracula: A Mystery Story");
    assert_eq!(book.authors, ["Bram Stoker"]);
    assert_eq!(book.year, Some(1897));
    assert_eq!(book.isbn.as_deref(), Some("9780141439518"));
    assert_eq!(book.language.as_deref(), Some("en"));
    assert_eq!(book.tags, ["Fiction / Horror"]);
    assert_eq!(
        book.description.as_deref(),
        Some("The classic vampire novel & its \"Count\".\nTold in letters.\nAnd diaries.")
    );
    assert_eq!(
        book.cover_url.as_deref(),
        Some(
            "https://books.google.com/books/content?id=ix0LAAAAYAAJ&printsec=frontcover&img=1&zoom=1&source=gbs_api"
        )
    );
    assert_eq!(
        book.page_url.as_deref(),
        Some("https://books.google.com/books?id=ix0LAAAAYAAJ")
    );
    assert!(book.offers.is_empty());

    let bare = &parsed.page.hits[1];
    assert_eq!(
        (bare.year, bare.isbn.as_deref(), bare.cover_url.as_deref()),
        (None, None, None)
    );
    assert!(bare.authors.is_empty() && bare.description.is_none());
}

#[test]
fn google_books_pages_by_start_index_and_sends_the_key_only_when_entered() {
    let twenty: Vec<_> = (0..20)
        .map(|n| json!({ "id": format!("v{n}"), "volumeInfo": { "title": format!("Book {n}") } }))
        .collect();
    let body = json!({ "items": twenty }).to_string();
    let server = TestServer::start(move |_, _| Reply::ok("application/json", body.clone()));
    let plugin = load(
        "google-books",
        &[("base_url", &server.url("/")), ("api_key", "my key&1")],
    );

    let answer = search(&plugin, &server, json!({ "text": "x", "page": 3 })).unwrap();
    assert_eq!(
        answer["next_cursor"], "60",
        "a full page may be followed by another"
    );
    let answer = search(
        &plugin,
        &server,
        json!({ "text": "x", "page": 2, "cursor": "20" }),
    )
    .unwrap();
    assert_eq!(answer["next_cursor"], "40");

    let requests = server.requests();
    assert_eq!(requests[0].query_param("startIndex").as_deref(), Some("40"));
    assert_eq!(requests[1].query_param("startIndex").as_deref(), Some("20"));
    assert_eq!(requests[1].query_param("key").as_deref(), Some("my key&1"));
}

#[test]
fn google_books_tells_the_user_what_to_do_when_the_quota_is_used_up() {
    for status in [429, 403] {
        let server = TestServer::start(move |_, _| Reply::status(status));
        let plugin = load("google-books", &[("base_url", &server.url("/"))]);
        let error = search(&plugin, &server, json!({ "text": "x", "page": 1 })).unwrap_err();
        assert!(
            error.contains("API key") && error.contains(&status.to_string()),
            "{error}"
        );
    }
}

// --- OPDS catalog (Project Gutenberg) --------------------------------------------------------

fn gutenberg_server() -> TestServer {
    TestServer::start(|request, _| match request.path.as_str() {
        "/ebooks/search.opds/" => match request.query_param("query").as_deref() {
            Some("dracula") => Reply::ok(
                "application/atom+xml",
                fixture("gutenberg-search-dracula.xml"),
            ),
            Some("war") => Reply::ok("application/atom+xml", fixture("gutenberg-search-war.xml")),
            _ => Reply::status(404),
        },
        "/ebooks/345.opds" => Reply::ok("application/atom+xml", fixture("gutenberg-book-345.xml")),
        _ => Reply::status(404),
    })
}

fn gutenberg(server: &TestServer) -> LoadedPlugin {
    load("opds-catalog", &[("base_url", &server.url("/"))])
}

#[test]
fn the_opds_plugin_lists_the_books_of_a_real_search_and_skips_the_categories() {
    let server = gutenberg_server();
    let plugin = gutenberg(&server);
    let answer = search(&plugin, &server, json!({ "text": "dracula", "page": 1 })).unwrap();
    let parsed = accept(&plugin, &answer);

    // The capture has seven entries; one is the "Subjects" category.
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.page.hits.len(), 6);
    assert_eq!(parsed.page.next_cursor, None);
    let book = &parsed.page.hits[0];
    assert_eq!(
        (book.title.as_str(), book.authors.clone()),
        ("Dracula", vec!["Bram Stoker".to_owned()])
    );
    assert_eq!(book.id, server.url("/ebooks/345.opds"));
    assert_eq!(book.page_url, Some(server.url("/ebooks/345")));
    assert_eq!(
        book.cover_url,
        Some(server.url("/cache/epub/345/pg345.cover.small.jpg"))
    );
    // The feed names the files in the book's own feed, not here.
    assert!(book.offers.is_empty());
    assert_eq!(
        server.requests()[0].target,
        "/ebooks/search.opds/?query=dracula"
    );
}

#[test]
fn the_opds_plugin_follows_the_next_link_as_the_cursor() {
    let server = gutenberg_server();
    let plugin = gutenberg(&server);
    let answer = search(&plugin, &server, json!({ "text": "war", "page": 1 })).unwrap();
    let parsed = accept(&plugin, &answer);
    // Three categories (authors, subjects, bookshelves) and 25 books.
    assert_eq!(parsed.page.hits.len(), 25);
    // A book without an author carries "8109 downloads" where the author usually is.
    assert!(
        parsed
            .page
            .hits
            .iter()
            .all(|hit| !hit.authors.iter().any(|a| a.contains("downloads"))),
        "a download count is not an author"
    );
    assert!(parsed.page.hits.iter().any(|hit| hit.authors.is_empty()));
    let cursor = parsed.page.next_cursor.expect("the feed has a next page");
    assert_eq!(
        cursor,
        server.url("/ebooks/search.opds/?query=war&start_index=26")
    );

    search(
        &plugin,
        &server,
        json!({ "text": "war", "page": 2, "cursor": cursor }),
    )
    .unwrap();
    assert_eq!(
        server.requests()[1].target,
        "/ebooks/search.opds/?query=war&start_index=26"
    );
}

#[test]
fn the_opds_plugin_reads_the_files_of_a_book_from_its_own_feed() {
    let server = gutenberg_server();
    let plugin = gutenberg(&server);
    let found = search(&plugin, &server, json!({ "text": "dracula", "page": 1 })).unwrap();
    let allow = plugin.allow_list().unwrap();
    let hit = accept(&plugin, &found).page.hits.remove(0);

    let answer = call(
        &plugin,
        &server,
        PluginFunction::Details,
        &serde_json::to_value(&hit).unwrap(),
    )
    .unwrap();
    let merged = hit
        .with_details(&answer.to_string(), &|host| allow.allows_host(host))
        .unwrap();

    let formats: Vec<_> = merged.offers.iter().map(|o| o.format.label()).collect();
    assert_eq!(formats, ["EPUB", "MOBI", "EPUB", "EPUB", "MOBI", "MOBI"]);
    assert!(merged.offers[2].id.ends_with("/ebooks/345.epub3.images"));
    assert_eq!(merged.offers[2].size, Some(602_902));
    assert_eq!(
        merged.offers[2].label.as_deref(),
        Some("EPUB3 (E-readers incl. Send-to-Kindle)")
    );
    assert!(
        merged
            .description
            .as_deref()
            .unwrap()
            .starts_with("\"Dracula\" by Bram Stoker is a Gothic horror novel")
    );
    assert_eq!(merged.language.as_deref(), Some("en"));
    assert_eq!(merged.tags.len(), 7);
    assert_eq!(merged.tags[0], "Horror tales");
    assert_eq!(
        merged.cover_url.as_deref(),
        Some("https://www.gutenberg.org/cache/epub/345/pg345.cover.medium.jpg")
    );
    // What the search had is kept.
    assert_eq!(merged.id, hit.id);
    assert_eq!(merged.authors, hit.authors);

    // A click on the icon: the offer's address, nothing renamed.
    let offer = &merged.offers[2];
    let spec = call(
        &plugin,
        &server,
        PluginFunction::ResolveDownload,
        &serde_json::to_value(offer).unwrap(),
    )
    .unwrap();
    let spec =
        parse_download_spec(&spec.to_string(), offer, &|host| allow.allows_host(host)).unwrap();
    assert_eq!(spec.url, offer.id);
    assert_eq!(spec.archive, None);
}

/// A catalog that lists the files in the search feed itself (the other shape the plugin reads).
#[test]
fn the_opds_plugin_takes_files_listed_in_the_search_feed() {
    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Results</title>
  <link rel="next" href="/opds/search?q=war&amp;page=2"/>
  <entry>
    <id>urn:book:1</id><title>War and Peace</title>
    <author><name>Leo Tolstoy</name></author>
    <link rel="http://opds-spec.org/acquisition/open-access" type="application/fb2+zip" href="/files/1.fb2.zip" title="FB2" length="1234"/>
    <link rel="http://opds-spec.org/acquisition" type="application/epub+zip; profile=x" href="files/1.epub"/>
    <link rel="http://opds-spec.org/acquisition" type="application/x-unknown" href="/files/1.bin"/>
  </entry>
  <entry><id>urn:category:1</id><title>Authors</title><link rel="subsection" href="/opds/authors"/></entry>
</feed>"#;
    let server = TestServer::start(|_, _| Reply::ok("application/atom+xml", FEED));
    let plugin = load(
        "opds-catalog",
        &[
            ("base_url", &server.url("/")),
            ("search_path", "/opds/search?q={searchTerms}"),
        ],
    );
    let answer = search(
        &plugin,
        &server,
        json!({ "text": "war & peace", "page": 1 }),
    )
    .unwrap();
    let parsed = accept(&plugin, &answer);
    assert_eq!(
        server.requests()[0].target,
        "/opds/search?q=war%20%26%20peace"
    );

    assert_eq!(parsed.page.hits.len(), 1);
    let book = &parsed.page.hits[0];
    assert_eq!(
        (book.id.as_str(), book.authors.clone()),
        ("urn:book:1", vec!["Leo Tolstoy".to_owned()])
    );
    assert_eq!(book.offers.len(), 2, "the unknown type is not offered");
    assert_eq!(book.offers[0].format.label(), "FB2");
    assert_eq!(book.offers[0].archive, Some(Archive::Zip));
    assert_eq!(book.offers[0].size, Some(1234));
    assert_eq!(book.offers[0].id, server.url("/files/1.fb2.zip"));
    assert_eq!(
        book.offers[1].id,
        server.url("/opds/files/1.epub"),
        "relative to the feed"
    );
    assert_eq!(
        parsed.page.next_cursor,
        Some(server.url("/opds/search?q=war&page=2"))
    );

    // Everything is already there: no request for details.
    let none = call(
        &plugin,
        &server,
        PluginFunction::Details,
        &serde_json::to_value(book).unwrap(),
    )
    .unwrap();
    assert_eq!(none, Value::Null);
    assert_eq!(server.requests().len(), 1);

    // The zip comes through `resolveDownload` as an archive.
    let allow = plugin.allow_list().unwrap();
    let spec = call(
        &plugin,
        &server,
        PluginFunction::ResolveDownload,
        &serde_json::to_value(&book.offers[0]).unwrap(),
    )
    .unwrap();
    let spec = parse_download_spec(&spec.to_string(), &book.offers[0], &|h| {
        allow.allows_host(h)
    })
    .unwrap();
    assert_eq!(spec.archive, Some(Archive::Zip));
}

#[test]
fn the_opds_plugin_refuses_a_search_path_without_the_search_terms() {
    let server = gutenberg_server();
    let plugin = load(
        "opds-catalog",
        &[
            ("base_url", &server.url("/")),
            ("search_path", "/ebooks/search.opds/"),
        ],
    );
    let error = search(&plugin, &server, json!({ "text": "x", "page": 1 })).unwrap_err();
    assert!(error.contains("{searchTerms}"), "{error}");
    assert!(server.requests().is_empty());
}

#[test]
fn the_opds_plugin_reaches_the_default_catalog_only_through_its_manifest() {
    // Nothing was entered: the allow-list is the manifest's, and the default address is in it.
    let plugin = load("opds-catalog", &[]);
    let allow = plugin.allow_list().unwrap();
    assert!(allow.allows_host("www.gutenberg.org"));
    assert!(!allow.allows_host("example.org"));
}
