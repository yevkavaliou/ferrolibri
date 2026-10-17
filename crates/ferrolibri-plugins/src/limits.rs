//! Limits of the plugin contract. Lengths are counted in Unicode scalar values (`char`s).

use std::time::Duration;

/// `plugin.toml` larger than this is rejected before parsing.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// A plugin's script (`entry`) larger than this is not installed (D2).
pub const MAX_SCRIPT_BYTES: usize = 1024 * 1024;

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

// HTTP layer (D4, D6). All network access of the app and of plugins goes through `crate::Http`.

/// Body of one `host.fetch` response (D6).
pub const MAX_FETCH_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Body of a streamed download performed by the app (D7).
pub const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// Redirects followed for one request; every hop is checked against the allow-list.
pub const MAX_REDIRECTS: u32 = 5;
/// `host.fetch` calls during one plugin call (D6).
pub const MAX_FETCH_REQUESTS_PER_CALL: u32 = 20;
/// Pause between two requests to the same host. The catalog under observation answered 502 under
/// load and the cause is unknown, so requests are spaced out (research.md, OPDS reconnaissance).
pub const HOST_REQUEST_GAP: Duration = Duration::from_millis(250);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// One `host.fetch`: connection, redirects, headers and body together.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A streamed download is aborted when no byte arrives for this long.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_REQUEST_HEADER_VALUE_CHARS: usize = 1000;

// Plugin runtime (D6). One call = one QuickJS runtime on its own thread.

/// Memory of one plugin call's QuickJS runtime.
pub const PLUGIN_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// JavaScript stack of one plugin call.
pub const PLUGIN_STACK_BYTES: usize = 1024 * 1024;
/// Native stack of the thread that runs a plugin call: the JavaScript stack plus the host's own
/// recursion (XML trees are built recursively).
pub const PLUGIN_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;
/// How long past its deadline a call may take to unwind before the caller stops waiting for the
/// thread and reports a timeout (the thread is then abandoned).
pub const PLUGIN_UNWIND_GRACE: Duration = Duration::from_secs(3);
/// Text handed to `host.html.select` or `host.xml.parse` (the size of a `host.fetch` body).
pub const MAX_PARSE_INPUT_BYTES: usize = MAX_FETCH_BODY_BYTES as usize;
/// Deepest nesting of elements `host.html.select` accepts (browsers stop at 512 as well).
pub const MAX_HTML_DEPTH: usize = 512;
/// Tags open at once, by a count from the text, past which `host.html.select` refuses a page:
/// the HTML parser is quadratic in the depth of unclosed `<div>`s and cannot be interrupted.
pub const MAX_HTML_UNCLOSED_TAGS: usize = 20_000;
/// Elements `host.html.select` returns for one selector.
pub const MAX_SELECT_MATCHES: usize = 5_000;
/// Text and markup `host.html.select` returns for one call, all matches together.
pub const MAX_SELECT_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Elements `host.xml.parse` accepts.
pub const MAX_XML_NODES: u32 = 200_000;
/// `host.log` messages are cut to this many characters ...
pub const MAX_LOG_MESSAGE_CHARS: usize = 2000;
/// ... and a call writes at most this many of them.
pub const MAX_LOG_ENTRIES_PER_CALL: u32 = 200;
