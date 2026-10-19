use ferrolibri_core::book::Format;
use serde::Deserialize;

use crate::{
    search::{Archive, Offer, ResponseError, json_type, parse_json},
    text::{self, at},
};

/// What the app downloads: the result of the plugin's `resolveDownload(offer, ctx)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadSpec {
    pub url: String,
    /// The effective archive: the plugin's answer if it gave one, otherwise the offer's.
    pub archive: Option<Archive>,
}

#[derive(Deserialize)]
struct RawSpec {
    url: String,
    #[serde(default)]
    archive: Option<Archive>,
}

/// Validates the JSON `resolveDownload` returned for `offer`.
///
/// `url` must be `http`/`https` and its host allowed. `archive` in the response overrides the
/// offer's; `zip` is accepted only for `FB2`, which the offer — not the response —
/// determines. The file name is never taken from the plugin.
pub fn parse_download_spec(
    json: &str,
    offer: &Offer,
    host_allowed: &dyn Fn(&str) -> bool,
) -> Result<DownloadSpec, ResponseError> {
    let value = parse_json(json)?;
    if !value.is_object() {
        return Err(ResponseError::Malformed(format!(
            "resolveDownload must return an object {{ url }}, got {}",
            json_type(&value)
        )));
    }
    let raw: RawSpec = serde_json::from_value(value)
        .map_err(|e| ResponseError::Malformed(format!("resolveDownload: {e}")))?;
    let url = at("url", text::allowed_url(&raw.url, host_allowed))
        .map_err(|e| ResponseError::Malformed(format!("resolveDownload: {e}")))?;
    let archive = raw.archive.or(offer.archive);
    if archive.is_some() && offer.format != Format::Fb2 {
        return Err(ResponseError::Malformed(
            "resolveDownload: archive \"zip\" is allowed only for FB2 offers".into(),
        ));
    }
    Ok(DownloadSpec {
        url: url.into(),
        archive,
    })
}

#[cfg(test)]
mod tests;
