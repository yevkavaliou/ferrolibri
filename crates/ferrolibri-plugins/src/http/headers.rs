//! Request headers a plugin may set: a short white list. `Authorization` and `Cookie` are not
//! on it and neither is anything that could change how the request is framed or routed.

use crate::limits::MAX_REQUEST_HEADER_VALUE_CHARS;

const ALLOWED: [&str; 5] = [
    "Accept",
    "Accept-Language",
    "Content-Type",
    "Referer",
    "Range",
];

#[derive(Debug, Default)]
pub(crate) struct Sanitized {
    /// Canonical name and trimmed value, each name at most once (the first occurrence wins).
    pub kept: Vec<(&'static str, String)>,
    /// Names (as given) that were not sent: not on the list, repeated or with an invalid value.
    pub dropped: Vec<String>,
}

pub(crate) fn sanitize(headers: &[(&str, &str)]) -> Sanitized {
    let mut out = Sanitized::default();
    for (name, value) in headers {
        let canonical = ALLOWED
            .iter()
            .find(|allowed| allowed.eq_ignore_ascii_case(name));
        let value = value.trim();
        let valid = value.chars().count() <= MAX_REQUEST_HEADER_VALUE_CHARS
            && value
                .bytes()
                .all(|b| b == b'\t' || (b' '..0x7f).contains(&b));
        match canonical {
            Some(canonical) if valid && !out.kept.iter().any(|(kept, _)| kept == canonical) => {
                out.kept.push((canonical, value.to_owned()));
            }
            _ => out.dropped.push((*name).to_owned()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_headers_are_kept_with_canonical_names() {
        let result = sanitize(&[
            ("accept", "application/atom+xml"),
            ("AUTHORIZATION", "Bearer secret"),
            ("Cookie", "a=b"),
            ("accept-language", "ru"),
            ("Host", "evil.example"),
            ("X-Anything", "1"),
            ("range", "bytes=0-99"),
            ("Referer", "https://example.org/"),
            ("Content-Type", "text/plain"),
        ]);
        assert_eq!(
            result.kept,
            [
                ("Accept", "application/atom+xml".to_owned()),
                ("Accept-Language", "ru".to_owned()),
                ("Range", "bytes=0-99".to_owned()),
                ("Referer", "https://example.org/".to_owned()),
                ("Content-Type", "text/plain".to_owned()),
            ]
        );
        assert_eq!(
            result.dropped,
            ["AUTHORIZATION", "Cookie", "Host", "X-Anything"]
        );
    }

    #[test]
    fn invalid_and_repeated_values_are_dropped() {
        let long = "a".repeat(MAX_REQUEST_HEADER_VALUE_CHARS + 1);
        let result = sanitize(&[
            ("Accept", "text/html\r\nCookie: a=b"),
            ("Accept", "text/\u{0}html"),
            ("Accept", "caf\u{e9}"),
            ("Accept", &long),
            ("Accept", "  text/html  "),
            ("Accept", "text/plain"),
        ]);
        assert_eq!(result.kept, [("Accept", "text/html".to_owned())]);
        assert_eq!(result.dropped.len(), 5);
    }
}
