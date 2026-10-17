//! Limits of the plugin contract. Lengths are counted in Unicode scalar values (`char`s).

/// `plugin.toml` larger than this is rejected before parsing.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

pub const MIN_ID_CHARS: usize = 2;
pub const MAX_ID_CHARS: usize = 40;
pub const MAX_NAME_CHARS: usize = 60;
pub const MAX_AUTHOR_CHARS: usize = 100;
pub const MAX_LICENSE_CHARS: usize = 60;
pub const MAX_DESCRIPTION_CHARS: usize = 500;
pub const MAX_ENTRY_CHARS: usize = 64;
pub const MAX_PROVIDES: usize = 10;
pub const MAX_PROVIDES_CHARS: usize = 40;
pub const MAX_CONTENT_TAGS: usize = 10;
pub const MAX_CONTENT_TAG_CHARS: usize = 40;
pub const MAX_SEARCH_HINT_CHARS: usize = 120;
pub const MAX_DOMAINS: usize = 20;
pub const MAX_DOMAIN_CHARS: usize = 253;
pub const MAX_SETTINGS: usize = 20;
pub const MAX_SETTING_KEY_CHARS: usize = 40;
pub const MAX_SETTING_LABEL_CHARS: usize = 80;
pub const MAX_SETTING_TEXT_CHARS: usize = 500;
pub const MAX_SETTING_OPTIONS: usize = 20;
pub const MAX_SETTING_OPTION_CHARS: usize = 80;
pub const MIN_CALL_TIMEOUT_S: u32 = 5;
pub const MAX_CALL_TIMEOUT_S: u32 = 60;
pub const DEFAULT_CALL_TIMEOUT_S: u32 = 30;

/// JSON returned by `search`, `details` or `resolveDownload` (D6).
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_HITS_PER_PAGE: usize = 100;
pub const MAX_CURSOR_CHARS: usize = 500;
pub const MAX_URL_CHARS: usize = 2000;

pub const MAX_HIT_ID_CHARS: usize = 200;
pub const MAX_TITLE_CHARS: usize = 300;
pub const MAX_AUTHORS: usize = 20;
pub const MAX_AUTHOR_NAME_CHARS: usize = 120;
pub const MAX_HIT_DESCRIPTION_CHARS: usize = 8000;
pub const MAX_LANGUAGE_CHARS: usize = 35;
pub const MIN_YEAR: i32 = 1;
pub const MAX_YEAR: i32 = 2100;
pub const MAX_ISBN_CHARS: usize = 20;
pub const MAX_PUBLISHER_CHARS: usize = 200;
pub const MAX_SERIES_CHARS: usize = 200;
pub const MAX_SERIES_INDEX: f64 = 100_000.0;
pub const MAX_TAGS: usize = 30;
pub const MAX_TAG_CHARS: usize = 60;
pub const MAX_OFFERS: usize = 10;
pub const MAX_OFFER_ID_CHARS: usize = 200;
pub const MAX_OFFER_LABEL_CHARS: usize = 100;
/// Declared size of an offered file: a plausibility check, the real limit applies to the download.
pub const MAX_OFFER_SIZE: u64 = 2 * 1024 * 1024 * 1024;
