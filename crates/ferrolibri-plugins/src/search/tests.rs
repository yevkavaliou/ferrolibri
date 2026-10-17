use serde_json::json;

use super::*;

fn allow_all(_: &str) -> bool {
    true
}

fn only_covers(host: &str) -> bool {
    host == "covers.example.org"
}

fn parse(value: Value) -> ParsedPage {
    parse_search_page(&value.to_string(), &allow_all).unwrap_or_else(|e| panic!("{e}"))
}

/// One hit built from `base` with `overrides` applied; returns the page's single outcome.
fn parse_hit(overrides: Value) -> Result<SearchHit, String> {
    let mut hit = json!({ "id": "1", "title": "Title" });
    for (k, v) in overrides.as_object().unwrap() {
        hit[k] = v.clone();
    }
    let mut page = parse(json!({ "hits": [hit] }));
    match page.page.hits.pop() {
        Some(hit) => Ok(hit),
        None => Err(page.warnings.pop().expect("a warning").message),
    }
}

fn rejected(overrides: Value) -> String {
    parse_hit(overrides.clone()).expect_err(&format!("{overrides} must be rejected"))
}

#[test]
fn minimal_and_full_hits_are_accepted() {
    let page = parse(json!({ "hits": [{ "id": "a", "title": "T" }] }));
    assert_eq!(page.page.hits.len(), 1);
    assert!(page.warnings.is_empty());
    assert_eq!(page.page.next_cursor, None);

    let hit = parse_hit(json!({
        "authors": ["A. Author"],
        "description": "Line one\nLine two",
        "language": "pt-BR",
        "year": 1999,
        "isbn": "978-3-16-148410-0",
        "publisher": "Pub",
        "series": "Saga",
        "series_index": 2.5,
        "tags": ["fiction"],
        "cover_url": "https://covers.example.org/1.jpg",
        "page_url": "https://example.org/book/1",
        "offers": [
            { "id": "o1", "format": "EPUB", "size": 1000, "label": "EPUB" },
            { "id": "o2", "format": "FB2", "archive": "zip" }
        ]
    }))
    .unwrap();
    assert_eq!(hit.authors, ["A. Author"]);
    assert_eq!(hit.series_index, Some(2.5));
    assert_eq!(hit.offers[0].format, Format::Epub);
    assert_eq!(hit.offers[1].archive, Some(Archive::Zip));
}

#[test]
fn nulls_and_blank_optional_strings_mean_absent() {
    let hit = parse_hit(json!({
        "authors": null, "tags": null, "offers": null,
        "description": null, "year": null,
        "language": "", "isbn": "  ", "series": "", "publisher": "",
        "cover_url": "", "page_url": ""
    }))
    .unwrap();
    assert!(hit.authors.is_empty() && hit.tags.is_empty() && hit.offers.is_empty());
    assert_eq!(hit.description, None);
    assert_eq!(hit.year, None);
    assert_eq!(hit.language, None);
    assert_eq!(hit.isbn, None);
    assert_eq!(hit.series, None);
    assert_eq!(hit.publisher, None);
    assert_eq!(hit.cover_url, None);
    assert_eq!(hit.page_url, None);
}

#[test]
fn unknown_hit_fields_are_ignored() {
    assert!(parse_hit(json!({ "rating": 5, "extra": { "x": 1 } })).is_ok());
}

#[test]
fn required_fields_are_enforced() {
    let page = parse(json!({ "hits": [
        { "title": "no id" },
        { "id": "no title" },
        { "id": 7, "title": "numeric id" },
        { "id": "ok", "title": "fine" },
        "not an object"
    ]}));
    assert_eq!(page.page.hits.len(), 1);
    assert_eq!(page.warnings.len(), 4);
    assert_eq!(page.warnings[0].index, 0);
    assert_eq!(page.warnings[1].id.as_deref(), Some("no title"));
    assert_eq!(page.warnings[3].index, 4);
}

fn text_limit(field: &str, max: usize) {
    let ok = parse_hit(json!({ field: "я".repeat(max) }));
    assert!(ok.is_ok(), "{field} at the limit: {ok:?}");
    let message = rejected(json!({ field: "я".repeat(max + 1) }));
    assert!(message.starts_with(field), "{field}: {message}");
    assert!(message.contains(&max.to_string()), "{message}");
    let message = rejected(json!({ field: "bad\u{7}char" }));
    assert!(message.contains("control"), "{field}: {message}");
}

#[test]
fn id_and_title_limits() {
    let ok = parse_search_page(
        &json!({ "hits": [{ "id": "i".repeat(MAX_HIT_ID_CHARS), "title": "t".repeat(MAX_TITLE_CHARS) }] })
            .to_string(),
        &allow_all,
    )
    .unwrap();
    assert_eq!(ok.page.hits.len(), 1);

    let page = parse(json!({ "hits": [
        { "id": "i".repeat(MAX_HIT_ID_CHARS + 1), "title": "t" },
        { "id": "i", "title": "t".repeat(MAX_TITLE_CHARS + 1) },
        { "id": " ", "title": "t" },
        { "id": "i", "title": "" },
        { "id": "i", "title": "two\nlines" },
    ]}));
    assert!(page.page.hits.is_empty());
    assert_eq!(page.warnings.len(), 5, "{:?}", page.warnings);
}

#[test]
fn single_line_text_limits() {
    text_limit("publisher", MAX_PUBLISHER_CHARS);
    text_limit("series", MAX_SERIES_CHARS);
}

#[test]
fn description_limit_allows_line_breaks_only() {
    assert!(parse_hit(json!({ "description": "a".repeat(MAX_HIT_DESCRIPTION_CHARS) })).is_ok());
    let message = rejected(json!({ "description": "a".repeat(MAX_HIT_DESCRIPTION_CHARS + 1) }));
    assert!(message.starts_with("description"), "{message}");
    assert!(parse_hit(json!({ "description": "a\r\n\tb" })).is_ok());
    assert!(rejected(json!({ "description": "a\u{0}b" })).contains("control"));
}

#[test]
fn authors_limits() {
    let many = |n: usize| vec!["A"; n];
    assert!(parse_hit(json!({ "authors": many(MAX_AUTHORS) })).is_ok());
    assert!(rejected(json!({ "authors": many(MAX_AUTHORS + 1) })).starts_with("authors"));
    assert!(parse_hit(json!({ "authors": ["я".repeat(MAX_AUTHOR_NAME_CHARS)] })).is_ok());
    assert!(
        rejected(json!({ "authors": ["ok", "я".repeat(MAX_AUTHOR_NAME_CHARS + 1)] }))
            .starts_with("authors[1]")
    );
    assert!(rejected(json!({ "authors": [""] })).starts_with("authors[0]"));
    assert!(rejected(json!({ "authors": "A. Author" })).contains("invalid type"));
}

#[test]
fn language_must_look_like_bcp47() {
    for good in ["en", "eng", "pt-BR", "zh-Hant-TW", "en-US-x1"] {
        assert!(parse_hit(json!({ "language": good })).is_ok(), "{good}");
    }
    for bad in [
        "e",
        "english",
        "en_US",
        "en-",
        "-en",
        "en US",
        "12",
        "en-toolongsubtag",
    ] {
        assert!(
            rejected(json!({ "language": bad })).starts_with("language"),
            "{bad}"
        );
    }
}

#[test]
fn year_and_series_index_ranges() {
    for good in [MIN_YEAR, 1999, MAX_YEAR] {
        assert!(parse_hit(json!({ "year": good })).is_ok(), "{good}");
    }
    for bad in [0, -5, MAX_YEAR + 1] {
        assert!(
            rejected(json!({ "year": bad })).starts_with("year"),
            "{bad}"
        );
    }
    assert!(rejected(json!({ "year": "1999" })).contains("invalid type"));

    for good in [0.0, 1.0, 2.5, MAX_SERIES_INDEX] {
        assert!(parse_hit(json!({ "series_index": good })).is_ok(), "{good}");
    }
    for bad in [-1.0, MAX_SERIES_INDEX + 1.0] {
        assert!(
            rejected(json!({ "series_index": bad })).starts_with("series_index"),
            "{bad}"
        );
    }
}

#[test]
fn isbn_must_have_10_or_13_digits() {
    for good in [
        "0306406152",
        "978-3-16-148410-0",
        "9783161484100",
        "080442957X",
        "0 8044 2957 x",
    ] {
        assert!(parse_hit(json!({ "isbn": good })).is_ok(), "{good}");
    }
    for bad in [
        "12345",
        "97831614841001",
        "isbn:0306406152",
        "030640615a",
        &"1".repeat(MAX_ISBN_CHARS + 1),
    ] {
        assert!(
            rejected(json!({ "isbn": bad })).starts_with("isbn"),
            "{bad}"
        );
    }
}

#[test]
fn tags_limits() {
    let many = |n: usize| (0..n).map(|i| format!("t{i}")).collect::<Vec<_>>();
    assert!(parse_hit(json!({ "tags": many(MAX_TAGS) })).is_ok());
    assert!(rejected(json!({ "tags": many(MAX_TAGS + 1) })).starts_with("tags"));
    assert!(parse_hit(json!({ "tags": ["я".repeat(MAX_TAG_CHARS)] })).is_ok());
    assert!(rejected(json!({ "tags": ["я".repeat(MAX_TAG_CHARS + 1)] })).starts_with("tags[0]"));
}

#[test]
fn urls_must_be_http_without_credentials() {
    for field in ["cover_url", "page_url"] {
        for good in [
            "http://covers.example.org/a.jpg",
            "https://covers.example.org:8443/a?b=c#d",
        ] {
            assert!(parse_hit(json!({ field: good })).is_ok(), "{field} {good}");
        }
        for bad in [
            "ftp://covers.example.org/a.jpg",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "//covers.example.org/a.jpg",
            "/relative/a.jpg",
            "https://user:pw@covers.example.org/",
            "https://covers.example.org/a b.jpg",
            "https://",
            "not a url",
        ] {
            assert!(
                rejected(json!({ field: bad })).starts_with(field),
                "{field} {bad}"
            );
        }
        let long = format!("https://covers.example.org/{}", "a".repeat(MAX_URL_CHARS));
        assert!(rejected(json!({ field: long })).starts_with(field));
    }
}

#[test]
fn cover_url_host_must_be_allowed_but_page_url_may_be_anywhere() {
    let json = json!({ "hits": [
        { "id": "1", "title": "T", "cover_url": "https://covers.example.org/1.jpg" },
        { "id": "2", "title": "T", "cover_url": "https://evil.example.net/1.jpg" },
        { "id": "3", "title": "T", "cover_url": "http://127.0.0.1/1.jpg" },
        { "id": "4", "title": "T", "page_url": "https://anywhere.example.net/book" },
    ]})
    .to_string();
    let page = parse_search_page(&json, &only_covers).unwrap();
    let ids: Vec<_> = page.page.hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["1", "4"]);
    assert_eq!(page.warnings.len(), 2);
    assert!(page.warnings[0].message.contains("not allowed"));
}

#[test]
fn offers_limits() {
    let offer = |i: usize| json!({ "id": format!("o{i}"), "format": "EPUB" });
    let many = |n: usize| (0..n).map(offer).collect::<Vec<_>>();
    assert!(parse_hit(json!({ "offers": many(MAX_OFFERS) })).is_ok());
    assert!(rejected(json!({ "offers": many(MAX_OFFERS + 1) })).starts_with("offers"));

    let duplicate = json!([offer(1), offer(1)]);
    assert!(rejected(json!({ "offers": duplicate })).contains("used twice"));
}

#[test]
fn offer_fields_are_validated() {
    let bad = |offer: Value| rejected(json!({ "offers": [offer] }));
    assert!(bad(json!({ "format": "EPUB" })).contains("missing field"));
    assert!(bad(json!({ "id": "o", "format": "RTF" })).contains("unknown variant"));
    assert!(bad(json!({ "id": "o", "format": "epub" })).contains("unknown variant"));
    assert!(bad(json!({ "id": "o", "format": "KEPUB" })).contains("format must be one of"));
    assert!(bad(json!({ "id": "", "format": "EPUB" })).contains("offers[0].id"));
    assert!(bad(json!({ "id": "o", "format": "EPUB", "size": 0 })).contains("size"));
    assert!(
        bad(json!({ "id": "o", "format": "EPUB", "size": MAX_OFFER_SIZE + 1 })).contains("size")
    );
    assert!(bad(json!({ "id": "o", "format": "EPUB", "size": -5 })).contains("invalid value"));
    assert!(
        bad(json!({ "id": "o", "format": "EPUB", "label": "x".repeat(MAX_OFFER_LABEL_CHARS + 1) }))
            .contains("label")
    );
    assert!(
        bad(json!({ "id": "o".repeat(MAX_OFFER_ID_CHARS + 1), "format": "EPUB" }))
            .contains("offers[0].id")
    );
}

#[test]
fn all_seven_offer_formats_are_accepted() {
    for format in ["EPUB", "FB2", "MOBI", "AZW3", "PDF", "TXT", "DOCX"] {
        let hit = parse_hit(json!({ "offers": [{ "id": "o", "format": format }] })).unwrap();
        assert_eq!(hit.offers[0].format.label(), format);
    }
}

#[test]
fn zip_archive_is_only_for_fb2() {
    assert!(
        parse_hit(json!({ "offers": [{ "id": "o", "format": "FB2", "archive": "zip" }] })).is_ok()
    );
    for format in ["EPUB", "MOBI", "PDF", "TXT", "DOCX", "AZW3"] {
        let message =
            rejected(json!({ "offers": [{ "id": "o", "format": format, "archive": "zip" }] }));
        assert!(
            message.contains("only with format FB2"),
            "{format}: {message}"
        );
    }
    for archive in ["rar", "7z", "ZIP", "gzip"] {
        let message =
            rejected(json!({ "offers": [{ "id": "o", "format": "FB2", "archive": archive }] }));
        assert!(message.contains("unknown variant"), "{archive}: {message}");
    }
}

#[test]
fn duplicate_hit_ids_drop_the_later_ones() {
    let page = parse(json!({ "hits": [
        { "id": "same", "title": "First" },
        { "id": "same", "title": "Second" },
        { "id": "other", "title": "Third" },
    ]}));
    let titles: Vec<_> = page.page.hits.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(titles, ["First", "Third"]);
    assert_eq!(page.warnings.len(), 1);
    assert_eq!(page.warnings[0].index, 1);
    assert!(page.warnings[0].to_string().contains("not unique"));
}

#[test]
fn good_hits_survive_bad_neighbours_in_order() {
    let page = parse(json!({ "hits": [
        { "id": "1", "title": "A" },
        { "id": "2" },
        { "id": "3", "title": "C" },
    ]}));
    let ids: Vec<_> = page.page.hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["1", "3"]);
    assert_eq!(page.warnings.len(), 1);
}

#[test]
fn page_limits_reject_the_whole_page() {
    let hits = |n: usize| {
        (0..n)
            .map(|i| json!({ "id": i.to_string(), "title": "T" }))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        parse(json!({ "hits": hits(MAX_HITS_PER_PAGE) }))
            .page
            .hits
            .len(),
        MAX_HITS_PER_PAGE
    );
    assert_eq!(
        parse_search_page(
            &json!({ "hits": hits(MAX_HITS_PER_PAGE + 1) }).to_string(),
            &allow_all
        ),
        Err(ResponseError::TooManyHits {
            count: MAX_HITS_PER_PAGE + 1
        })
    );

    let huge = format!(
        r#"{{"hits":[{{"id":"1","title":"T","description":"{}"}}]}}"#,
        "a".repeat(MAX_RESPONSE_BYTES)
    );
    assert!(matches!(
        parse_search_page(&huge, &allow_all),
        Err(ResponseError::TooLarge { .. })
    ));
}

#[test]
fn malformed_pages_are_rejected() {
    for json in [
        "",
        "not json",
        "[]",
        "null",
        "42",
        "\"hits\"",
        "{}",
        r#"{"hits":null}"#,
        r#"{"hits":{}}"#,
        r#"{"hits":"x"}"#,
    ] {
        assert!(
            matches!(
                parse_search_page(json, &allow_all),
                Err(ResponseError::Malformed(_))
            ),
            "{json:?}"
        );
    }
}

#[test]
fn next_cursor_rules() {
    assert_eq!(
        parse(json!({ "hits": [], "next_cursor": "abc" }))
            .page
            .next_cursor
            .as_deref(),
        Some("abc")
    );
    for absent in [
        json!({ "hits": [] }),
        json!({ "hits": [], "next_cursor": null }),
        json!({ "hits": [], "next_cursor": "" }),
    ] {
        assert_eq!(parse(absent).page.next_cursor, None);
    }
    let long = "я".repeat(MAX_CURSOR_CHARS);
    assert_eq!(
        parse(json!({ "hits": [], "next_cursor": long.clone() }))
            .page
            .next_cursor,
        Some(long)
    );
    for bad in [
        json!("я".repeat(MAX_CURSOR_CHARS + 1)),
        json!(5),
        json!({}),
        json!(true),
    ] {
        assert!(
            matches!(
                parse_search_page(
                    &json!({ "hits": [], "next_cursor": bad }).to_string(),
                    &allow_all
                ),
                Err(ResponseError::InvalidCursor(_))
            ),
            "{bad}"
        );
    }
}

#[test]
fn cursor_flows_from_page_to_next_query() {
    let query = SearchQuery::first_page("dune");
    assert_eq!(query.page, 1);
    assert_eq!(query.cursor, None);
    let page = parse(json!({ "hits": [], "next_cursor": "https://example.org/opds?page=2" })).page;
    let next = query.next_page(&page).unwrap();
    assert_eq!((next.text.as_str(), next.page), ("dune", 2));
    assert_eq!(
        next.cursor.as_deref(),
        Some("https://example.org/opds?page=2")
    );
    assert_eq!(next.next_page(&SearchPage::default()), None);

    let sent = serde_json::to_value(&query).unwrap();
    assert_eq!(sent, json!({ "text": "dune", "page": 1 }));
    assert_eq!(
        serde_json::to_value(&next).unwrap()["cursor"],
        "https://example.org/opds?page=2"
    );
}

fn base_hit() -> SearchHit {
    parse_hit(json!({
        "authors": ["A"],
        "description": "short",
        "offers": [{ "id": "o", "format": "EPUB" }]
    }))
    .unwrap()
}

#[test]
fn details_fill_in_missing_fields() {
    let merged = merge_details(
        &base_hit(),
        r#"{"description":"long annotation","cover_url":"https://covers.example.org/c.jpg","series":"Saga","series_index":3}"#,
        &allow_all,
    )
    .unwrap();
    assert_eq!(merged.description.as_deref(), Some("long annotation"));
    assert_eq!(
        merged.cover_url.as_deref(),
        Some("https://covers.example.org/c.jpg")
    );
    assert_eq!(merged.series.as_deref(), Some("Saga"));
    assert_eq!(merged.series_index, Some(3.0));
    assert_eq!(merged.authors, ["A"]);
    assert_eq!(merged.title, "Title");
    assert_eq!(merged.offers.len(), 1);
}

#[test]
fn details_never_overwrite_the_id() {
    let merged = merge_details(
        &base_hit(),
        r#"{"id":"hijacked","title":"New title"}"#,
        &allow_all,
    )
    .unwrap();
    assert_eq!(merged.id, "1");
    assert_eq!(merged.title, "New title");
}

#[test]
fn details_replace_offers_and_ignore_nulls() {
    let merged = merge_details(
        &base_hit(),
        r#"{"offers":[{"id":"x","format":"PDF"},{"id":"y","format":"MOBI"}],"description":null,"authors":null}"#,
        &allow_all,
    )
    .unwrap();
    assert_eq!(merged.offers.len(), 2);
    assert_eq!(merged.offers[0].format, Format::Pdf);
    assert_eq!(merged.description.as_deref(), Some("short"));
    assert_eq!(merged.authors, ["A"]);
}

#[test]
fn empty_details_change_nothing() {
    for json in ["null", "{}"] {
        assert_eq!(
            merge_details(&base_hit(), json, &allow_all).unwrap(),
            base_hit(),
            "{json}"
        );
    }
}

#[test]
fn invalid_details_are_rejected_and_the_original_is_untouched() {
    let hit = base_hit();
    for json in [
        "[]",
        "\"text\"",
        "not json",
        r#"{"title":""}"#,
        r#"{"cover_url":"ftp://x/y"}"#,
        r#"{"offers":[{"id":"x","format":"EPUB","archive":"zip"}]}"#,
        r#"{"year":"1999"}"#,
    ] {
        assert!(
            matches!(
                merge_details(&hit, json, &allow_all),
                Err(ResponseError::Malformed(_))
            ),
            "{json}"
        );
    }
    assert_eq!(hit, base_hit());
    let denied = merge_details(
        &hit,
        r#"{"cover_url":"https://evil.example.net/c.jpg"}"#,
        &only_covers,
    );
    assert!(matches!(denied, Err(ResponseError::Malformed(m)) if m.contains("not allowed")));
    let huge = format!(r#"{{"description":"{}"}}"#, "a".repeat(MAX_RESPONSE_BYTES));
    assert!(matches!(
        merge_details(&hit, &huge, &allow_all),
        Err(ResponseError::TooLarge { .. })
    ));
}

#[test]
fn response_errors_become_plugin_errors_of_the_core() {
    let error = ResponseError::TooManyHits { count: 500 }.into_core("search", "demo");
    assert!(matches!(
        &error.kind,
        ferrolibri_core::ErrorKind::Plugin { id, message } if id == "demo" && message.contains("500")
    ));
    assert!(error.to_string().contains("plugin demo"), "{error}");
}
