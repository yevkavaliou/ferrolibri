use serde_json::json;

use super::*;
use crate::limits::MAX_RESPONSE_BYTES;

fn allow_all(_: &str) -> bool {
    true
}

fn only_files(host: &str) -> bool {
    host == "files.example.org"
}

fn offer(format: Format, archive: Option<Archive>) -> Offer {
    Offer {
        id: "o".into(),
        format,
        archive,
        size: None,
        label: None,
    }
}

fn spec(json: serde_json::Value, offer: &Offer) -> Result<DownloadSpec, ResponseError> {
    parse_download_spec(&json.to_string(), offer, &allow_all)
}

#[test]
fn url_is_returned_normalised() {
    let spec = spec(
        json!({ "url": "https://files.example.org/a/book.epub?id=1" }),
        &offer(Format::Epub, None),
    )
    .unwrap();
    assert_eq!(spec.url, "https://files.example.org/a/book.epub?id=1");
    assert_eq!(spec.archive, None);
}

#[test]
fn url_must_be_http_and_allowed() {
    let epub = offer(Format::Epub, None);
    for bad in [
        json!({}),
        json!({ "url": 5 }),
        json!({ "url": "" }),
        json!({ "url": "ftp://files.example.org/a" }),
        json!({ "url": "file:///C:/book.epub" }),
        json!({ "url": "/relative/book.epub" }),
        json!({ "url": "https://u:p@files.example.org/a" }),
    ] {
        assert!(
            matches!(spec(bad.clone(), &epub), Err(ResponseError::Malformed(_))),
            "{bad}"
        );
    }
    let denied = parse_download_spec(
        r#"{"url":"https://evil.example.net/a.epub"}"#,
        &epub,
        &only_files,
    );
    assert!(matches!(denied, Err(ResponseError::Malformed(m)) if m.contains("not allowed")));
    assert!(
        parse_download_spec(
            r#"{"url":"https://files.example.org/a"}"#,
            &epub,
            &only_files
        )
        .is_ok()
    );
}

#[test]
fn response_must_be_an_object() {
    let epub = offer(Format::Epub, None);
    for json in ["", "null", "[]", "\"https://files.example.org/a\"", "nope"] {
        assert!(
            matches!(
                parse_download_spec(json, &epub, &allow_all),
                Err(ResponseError::Malformed(_))
            ),
            "{json:?}"
        );
    }
    let huge = format!(
        r#"{{"url":"https://files.example.org/{}"}}"#,
        "a".repeat(MAX_RESPONSE_BYTES)
    );
    assert!(matches!(
        parse_download_spec(&huge, &epub, &allow_all),
        Err(ResponseError::TooLarge { .. })
    ));
}

#[test]
fn archive_comes_from_the_offer_unless_the_response_overrides_it() {
    let url = "https://files.example.org/a";
    let zipped = offer(Format::Fb2, Some(Archive::Zip));
    assert_eq!(
        spec(json!({ "url": url }), &zipped).unwrap().archive,
        Some(Archive::Zip)
    );
    assert_eq!(
        spec(json!({ "url": url, "archive": null }), &zipped)
            .unwrap()
            .archive,
        Some(Archive::Zip)
    );
    let plain = offer(Format::Fb2, None);
    assert_eq!(
        spec(json!({ "url": url, "archive": "zip" }), &plain)
            .unwrap()
            .archive,
        Some(Archive::Zip)
    );
    assert_eq!(spec(json!({ "url": url }), &plain).unwrap().archive, None);
}

#[test]
fn zip_is_refused_for_non_fb2_offers_even_if_the_response_asks_for_it() {
    for format in [
        Format::Epub,
        Format::Mobi,
        Format::Pdf,
        Format::Txt,
        Format::Docx,
        Format::Azw3,
    ] {
        let result = spec(
            json!({ "url": "https://files.example.org/a", "archive": "zip" }),
            &offer(format, None),
        );
        assert!(
            matches!(&result, Err(ResponseError::Malformed(m)) if m.contains("only for FB2")),
            "{format:?}: {result:?}"
        );
    }
}

#[test]
fn other_archive_kinds_are_refused() {
    for archive in ["rar", "7z", "gzip", "ZIP", "", "none"] {
        let result = spec(
            json!({ "url": "https://files.example.org/a", "archive": archive }),
            &offer(Format::Fb2, None),
        );
        assert!(
            matches!(result, Err(ResponseError::Malformed(_))),
            "{archive:?}"
        );
    }
}
