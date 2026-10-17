//! Validation helpers for strings that come from a plugin. Errors are short phrases that
//! complete a sentence starting with the field name ("title must not be empty").

use std::fmt::Display;

use url::Url;

use crate::limits::MAX_URL_CHARS;

pub(crate) fn at<T>(field: impl Display, result: Result<T, String>) -> Result<T, String> {
    result.map_err(|message| format!("{field} {message}"))
}

pub(crate) fn max_chars(value: &str, max: usize) -> Result<(), String> {
    let count = value.chars().count();
    if count > max {
        return Err(format!("must be at most {max} characters (got {count})"));
    }
    Ok(())
}

/// A single-line, non-blank string without control characters.
pub(crate) fn line(value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("must not be empty".into());
    }
    max_chars(value, max)?;
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters".into());
    }
    Ok(())
}

/// Free text: like [`line`] but line breaks and tabs are allowed.
pub(crate) fn multiline(value: &str, max: usize) -> Result<(), String> {
    max_chars(value, max)?;
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("must not contain control characters".into());
    }
    Ok(())
}

/// Optional text: a blank string means "absent" and becomes `None`.
pub(crate) fn normalize(value: &mut Option<String>) {
    if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
        *value = None;
    }
}

/// `http` or `https` URL with a host and without credentials or whitespace.
pub(crate) fn http_url(value: &str, max: usize) -> Result<Url, String> {
    max_chars(value, max.min(MAX_URL_CHARS))?;
    if value.is_empty() || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("must be a URL without whitespace".into());
    }
    let url = Url::parse(value).map_err(|e| format!("is not a valid URL ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("must use http or https".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("must contain a host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("must not contain credentials".into());
    }
    Ok(url)
}

/// [`http_url`] whose host must also pass the caller's allow-list.
pub(crate) fn allowed_url(value: &str, host_allowed: &dyn Fn(&str) -> bool) -> Result<Url, String> {
    let url = http_url(value, MAX_URL_CHARS)?;
    let host = url.host_str().unwrap_or_default();
    if !host_allowed(host) {
        return Err(format!("points to a domain that is not allowed ({host})"));
    }
    Ok(url)
}
