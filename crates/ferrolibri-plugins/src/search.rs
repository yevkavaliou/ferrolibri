use std::{collections::HashSet, fmt};

use ferrolibri_core::book::Format;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{
    limits::*,
    text::{self, at},
};

/// What the app passes to the plugin's `search(query, ctx)`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    /// 1-based page number; plugins that paginate by cursor may ignore it.
    pub page: u32,
    /// What the plugin returned as `next_cursor` for the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl SearchQuery {
    pub fn first_page(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            page: 1,
            cursor: None,
        }
    }

    /// The query for the page after `page`, continuing from its `next_cursor`.
    pub fn next_page(&self, page: &SearchPage) -> Option<Self> {
        let cursor = page.next_cursor.clone()?;
        Some(Self {
            text: self.text.clone(),
            page: self.page + 1,
            cursor: Some(cursor),
        })
    }
}

/// An archive wrapped around the book file. Only `zip` around `FB2` is supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Archive {
    Zip,
}

/// A downloadable file the source offers for a result. The app turns it into a URL by
/// calling `resolveDownload` only when the user clicks the download icon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub format: Format,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<Archive>,
    /// Size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Offer {
    fn validate(&mut self) -> Result<(), String> {
        at("id", text::line(&self.id, MAX_OFFER_ID_CHARS))?;
        if self.format == Format::Kepub {
            return Err("format must be one of EPUB, FB2, MOBI, AZW3, PDF, TXT, DOCX".into());
        }
        if self.archive.is_some() && self.format != Format::Fb2 {
            return Err("archive \"zip\" is allowed only with format FB2".into());
        }
        if let Some(size) = self.size
            && !(1..=MAX_OFFER_SIZE).contains(&size)
        {
            return Err(format!("size must be between 1 and {MAX_OFFER_SIZE}"));
        }
        text::normalize(&mut self.label);
        if let Some(label) = &self.label {
            at("label", text::line(label, MAX_OFFER_LABEL_CHARS))?;
        }
        Ok(())
    }
}

/// One search result, in the app's schema. The app draws results itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Unique within the page; passed back to `details` and `resolveDownload`.
    pub id: String,
    pub title: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// BCP 47 language tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_index: Option<f64>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    /// "Open page" button; only `http`/`https`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub offers: Vec<Offer>,
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

impl SearchHit {
    /// Checks every limit of the contract. Blank optional strings become `None`.
    /// `host_allowed` decides whether the host of `cover_url` is permitted.
    fn validate(&mut self, host_allowed: &dyn Fn(&str) -> bool) -> Result<(), String> {
        at("id", text::line(&self.id, MAX_HIT_ID_CHARS))?;
        at("title", text::line(&self.title, MAX_TITLE_CHARS))?;

        if self.authors.len() > MAX_AUTHORS {
            return Err(format!(
                "authors may have at most {MAX_AUTHORS} items (got {})",
                self.authors.len()
            ));
        }
        for (i, author) in self.authors.iter().enumerate() {
            at(
                format!("authors[{i}]"),
                text::line(author, MAX_AUTHOR_NAME_CHARS),
            )?;
        }

        for value in [
            &mut self.description,
            &mut self.language,
            &mut self.isbn,
            &mut self.publisher,
            &mut self.series,
            &mut self.cover_url,
            &mut self.page_url,
        ] {
            text::normalize(value);
        }
        if let Some(v) = &self.description {
            at("description", text::multiline(v, MAX_HIT_DESCRIPTION_CHARS))?;
        }
        if let Some(v) = &self.language {
            at("language", check_language(v))?;
        }
        if let Some(year) = self.year
            && !(MIN_YEAR..=MAX_YEAR).contains(&year)
        {
            return Err(format!("year must be between {MIN_YEAR} and {MAX_YEAR}"));
        }
        if let Some(v) = &self.isbn {
            at("isbn", check_isbn(v))?;
        }
        if let Some(v) = &self.publisher {
            at("publisher", text::line(v, MAX_PUBLISHER_CHARS))?;
        }
        if let Some(v) = &self.series {
            at("series", text::line(v, MAX_SERIES_CHARS))?;
        }
        if let Some(index) = self.series_index
            && !(index.is_finite() && (0.0..=MAX_SERIES_INDEX).contains(&index))
        {
            return Err(format!(
                "series_index must be between 0 and {MAX_SERIES_INDEX}"
            ));
        }

        if self.tags.len() > MAX_TAGS {
            return Err(format!(
                "tags may have at most {MAX_TAGS} items (got {})",
                self.tags.len()
            ));
        }
        for (i, tag) in self.tags.iter().enumerate() {
            at(format!("tags[{i}]"), text::line(tag, MAX_TAG_CHARS))?;
        }

        if let Some(v) = &self.cover_url {
            at("cover_url", text::allowed_url(v, host_allowed).map(drop))?;
        }
        if let Some(v) = &self.page_url {
            at("page_url", text::http_url(v, MAX_URL_CHARS).map(drop))?;
        }

        if self.offers.len() > MAX_OFFERS {
            return Err(format!(
                "offers may have at most {MAX_OFFERS} items (got {})",
                self.offers.len()
            ));
        }
        let mut offer_ids = HashSet::new();
        for (i, offer) in self.offers.iter_mut().enumerate() {
            offer.validate().map_err(|m| format!("offers[{i}].{m}"))?;
            if !offer_ids.insert(offer.id.clone()) {
                return Err(format!("offers[{i}].id is used twice"));
            }
        }
        Ok(())
    }

    /// Applies the result of the plugin's `details(hit, ctx)` to this result.
    ///
    /// `details_json` is a JSON object (or `null` for "nothing to add"). Every non-null field
    /// replaces the field of the result as a whole (`offers` is replaced, not appended to),
    /// except `id`, which is never overwritten. The merged result is validated again; on
    /// error the original stays as it was and the caller keeps showing it.
    pub fn with_details(
        &self,
        details_json: &str,
        host_allowed: &dyn Fn(&str) -> bool,
    ) -> Result<SearchHit, ResponseError> {
        let details = parse_json(details_json)?;
        let details = match details {
            Value::Null => return Ok(self.clone()),
            Value::Object(map) => map,
            other => {
                return Err(ResponseError::Malformed(format!(
                    "details must be an object, got {}",
                    json_type(&other)
                )));
            }
        };
        let Value::Object(mut merged) =
            serde_json::to_value(self).expect("a hit always serializes")
        else {
            unreachable!("a hit serializes to an object")
        };
        for (key, value) in details {
            if key != "id" && !value.is_null() {
                merged.insert(key, value);
            }
        }
        let mut hit: SearchHit = serde_json::from_value(Value::Object(merged))
            .map_err(|e| ResponseError::Malformed(format!("details: {e}")))?;
        hit.validate(host_allowed)
            .map_err(|e| ResponseError::Malformed(format!("details: {e}")))?;
        Ok(hit)
    }
}

/// `search` returned this page. Hits that broke a limit were dropped individually
/// ([`ParsedPage::warnings`]); a page that broke a page-wide limit is a [`ResponseError`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Opaque string (at most 500 characters) to pass back as `query.cursor`; `None` on the
    /// last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A result dropped from a page, with the reason, for the plugin log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// Position in the plugin's `hits` array.
    pub index: usize,
    /// The result's `id`, if it had a readable one.
    pub id: Option<String>,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dropped hits[{}]", self.index)?;
        if let Some(id) = &self.id {
            write!(f, " (id {id:?})")?;
        }
        write!(f, ": {}", self.message)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedPage {
    pub page: SearchPage,
    pub warnings: Vec<Warning>,
}

/// The plugin's response is unusable as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResponseError {
    TooLarge {
        bytes: usize,
    },
    TooManyHits {
        count: usize,
    },
    InvalidCursor(String),
    /// Not JSON, wrong shape, or a value that breaks a limit.
    Malformed(String),
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                f,
                "response is too large ({bytes} bytes, at most {MAX_RESPONSE_BYTES})"
            ),
            Self::TooManyHits { count } => write!(
                f,
                "page has too many results ({count}, at most {MAX_HITS_PER_PAGE})"
            ),
            Self::InvalidCursor(message) => write!(f, "next_cursor {message}"),
            Self::Malformed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ResponseError {}

/// Validates the JSON a plugin's `search` returned.
///
/// `host_allowed` tells whether a host may be contacted (manifest domains plus hosts the
/// user entered); it is applied to `cover_url`.
pub fn parse_search_page(
    json: &str,
    host_allowed: &dyn Fn(&str) -> bool,
) -> Result<ParsedPage, ResponseError> {
    let Value::Object(mut page) = parse_json(json)? else {
        return Err(ResponseError::Malformed(
            "search must return an object { hits, next_cursor }".into(),
        ));
    };
    let hits = match page.remove("hits") {
        Some(Value::Array(hits)) => hits,
        Some(other) => {
            return Err(ResponseError::Malformed(format!(
                "hits must be an array, got {}",
                json_type(&other)
            )));
        }
        None => return Err(ResponseError::Malformed("hits is missing".into())),
    };
    if hits.len() > MAX_HITS_PER_PAGE {
        return Err(ResponseError::TooManyHits { count: hits.len() });
    }
    let next_cursor = match page.remove("next_cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) if cursor.trim().is_empty() => None,
        Some(Value::String(cursor)) => {
            text::max_chars(&cursor, MAX_CURSOR_CHARS).map_err(ResponseError::InvalidCursor)?;
            Some(cursor)
        }
        Some(other) => {
            return Err(ResponseError::InvalidCursor(format!(
                "must be a string, got {}",
                json_type(&other)
            )));
        }
    };

    let mut result = ParsedPage {
        page: SearchPage {
            hits: Vec::with_capacity(hits.len()),
            next_cursor,
        },
        warnings: Vec::new(),
    };
    let mut seen = HashSet::new();
    for (index, raw) in hits.into_iter().enumerate() {
        let id = raw.get("id").and_then(Value::as_str).map(str::to_owned);
        let warn = |message: String| Warning {
            index,
            id: id.clone(),
            message,
        };
        let mut hit: SearchHit = match serde_json::from_value(raw) {
            Ok(hit) => hit,
            Err(e) => {
                result.warnings.push(warn(e.to_string()));
                continue;
            }
        };
        if let Err(message) = hit.validate(host_allowed) {
            result.warnings.push(warn(message));
            continue;
        }
        if !seen.insert(hit.id.clone()) {
            result
                .warnings
                .push(warn("id is not unique within the page".into()));
            continue;
        }
        result.page.hits.push(hit);
    }
    Ok(result)
}

/// See [`SearchHit::with_details`].
pub fn merge_details(
    hit: &SearchHit,
    details_json: &str,
    host_allowed: &dyn Fn(&str) -> bool,
) -> Result<SearchHit, ResponseError> {
    hit.with_details(details_json, host_allowed)
}

pub(crate) fn parse_json(json: &str) -> Result<Value, ResponseError> {
    if json.len() > MAX_RESPONSE_BYTES {
        return Err(ResponseError::TooLarge { bytes: json.len() });
    }
    serde_json::from_str(json).map_err(|e| ResponseError::Malformed(format!("not valid JSON: {e}")))
}

pub(crate) fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Loose BCP 47: a 2–3 letter language and optional 1–8 character subtags (`en`, `pt-BR`).
fn check_language(tag: &str) -> Result<(), String> {
    text::max_chars(tag, MAX_LANGUAGE_CHARS)?;
    let mut parts = tag.split('-');
    let language_ok = parts
        .next()
        .is_some_and(|l| (2..=3).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_alphabetic()));
    let rest_ok =
        parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if language_ok && rest_ok {
        Ok(())
    } else {
        Err("must be a BCP 47 language tag such as \"en\" or \"pt-BR\"".into())
    }
}

/// Digits, `X`, spaces and hyphens only; the checksum is not verified.
fn check_isbn(value: &str) -> Result<(), String> {
    text::max_chars(value, MAX_ISBN_CHARS)?;
    let digits = value.bytes().filter(u8::is_ascii_alphanumeric).count();
    let chars_ok = value
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'X' | b'x' | b'-' | b' '));
    if chars_ok && matches!(digits, 10 | 13) {
        Ok(())
    } else {
        Err("must be an ISBN-10 or ISBN-13".into())
    }
}

#[cfg(test)]
mod tests;
