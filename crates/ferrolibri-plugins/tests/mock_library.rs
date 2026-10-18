//! The `mock-library` example (`examples/plugins/mock-library`): a source that answers every
//! search at once from made-up data, for checking the app. The tests run it in the real sandbox
//! and put every answer through the contract the app applies; nothing here uses the network.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ferrolibri_core::converter::CancellationToken;
use ferrolibri_plugins::{
    Archive, LoadedPlugin, LogLevel, LogRecord, Manifest, ParsedPage, PluginFunction,
    PluginRuntime, ResponseError, SearchHit, SettingValue, app_version, merge_details,
    parse_download_spec, parse_search_page,
    testing::{Reply, TestServer},
};
use serde_json::{Value, json};

const TOTAL: usize = 45;

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/mock-library")
}

fn plugin(stored: &[(&str, SettingValue)]) -> LoadedPlugin {
    let manifest = Manifest::parse(&std::fs::read_to_string(folder().join("plugin.toml")).unwrap())
        .expect("the manifest parses");
    let source = std::fs::read_to_string(folder().join(&manifest.entry)).unwrap();
    LoadedPlugin {
        manifest,
        source,
        settings: stored
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn text(value: &str) -> SettingValue {
    SettingValue::Text(value.to_owned())
}

/// A sandbox over a server that records every request (there must be none).
struct Bench {
    server: TestServer,
    logs: Arc<Mutex<Vec<LogRecord>>>,
    runtime: PluginRuntime,
}

impl Bench {
    fn new() -> Self {
        let server = TestServer::start(|_, _| Reply::status(404));
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let runtime = PluginRuntime::new(server.http())
            .with_log_sink(Arc::new(move |record| sink.lock().unwrap().push(record)));
        Self {
            server,
            logs,
            runtime,
        }
    }

    fn call(
        &self,
        plugin: &LoadedPlugin,
        function: PluginFunction,
        argument: &Value,
    ) -> Result<Value, String> {
        self.runtime
            .call(plugin, function, argument, &CancellationToken::default())
            .map_err(|e| e.to_string())
    }

    fn raw_search(&self, plugin: &LoadedPlugin, argument: &Value) -> Result<Value, String> {
        self.call(plugin, PluginFunction::Search, argument)
    }

    /// What the app does with a search answer.
    fn search(&self, plugin: &LoadedPlugin, argument: &Value) -> ParsedPage {
        accept(plugin, &self.raw_search(plugin, argument).unwrap()).unwrap()
    }

    fn first_page(&self, plugin: &LoadedPlugin, query: &str) -> ParsedPage {
        self.search(plugin, &json!({ "text": query, "page": 1 }))
    }
}

fn accept(plugin: &LoadedPlugin, answer: &Value) -> Result<ParsedPage, ResponseError> {
    let allow = plugin.allow_list().unwrap();
    parse_search_page(&answer.to_string(), &|host| allow.allows_host(host))
}

fn ids(page: &ParsedPage) -> Vec<&str> {
    page.page.hits.iter().map(|h| h.id.as_str()).collect()
}

fn hit<'a>(page: &'a ParsedPage, id: &str) -> &'a SearchHit {
    page.page
        .hits
        .iter()
        .find(|h| h.id == id)
        .unwrap_or_else(|| panic!("no {id} on the page"))
}

/// Every page of the regular set, following `next_cursor` as the app does.
fn walk(bench: &Bench, plugin: &LoadedPlugin, query: &str) -> Vec<ParsedPage> {
    let mut pages = vec![bench.first_page(plugin, query)];
    while let Some(cursor) = pages.last().unwrap().page.next_cursor.clone() {
        let next = json!({ "text": query, "page": pages.len() + 1, "cursor": cursor });
        pages.push(bench.search(plugin, &next));
        assert!(pages.len() <= 10, "the cursor never ends");
    }
    pages
}

// --- The plugin as a whole -----------------------------------------------------------------

#[test]
fn the_mock_is_a_compatible_plugin_that_names_only_the_unresolvable_domain() {
    let plugin = plugin(&[]);
    assert!(
        plugin
            .manifest
            .compatibility(&app_version())
            .is_compatible()
    );
    assert_eq!(plugin.manifest.id, "mock-library");
    assert_eq!(plugin.manifest.network.domains, ["mock.invalid"]);
    assert!(folder().join("README.md").is_file());
}

#[test]
fn any_text_gives_the_same_books_and_nothing_touches_the_network() {
    let bench = Bench::new();
    let plugin = plugin(&[]);
    let one = bench.first_page(&plugin, "war and peace");
    let two = bench.first_page(&plugin, "something else entirely");
    let empty = bench.first_page(&plugin, "");
    assert_eq!(ids(&one), ids(&two));
    assert_eq!(ids(&one), ids(&empty));
    // The text is echoed so a tester can see that the request arrived.
    let echo = |page: &ParsedPage| hit(page, "book-01").description.clone().unwrap();
    assert!(echo(&one).contains("\"war and peace\", page 1"));
    assert!(echo(&two).contains("\"something else entirely\""));
    // The same call twice gives the same answer.
    let argument = json!({ "text": "x", "page": 2 });
    assert_eq!(
        bench.raw_search(&plugin, &argument).unwrap(),
        bench.raw_search(&plugin, &argument).unwrap()
    );
    assert!(bench.server.requests().is_empty());
}

// --- The regular set -----------------------------------------------------------------------

#[test]
fn the_set_has_45_books_in_pages_that_follow_per_page() {
    let bench = Bench::new();
    for (per_page, pages) in [("10", 5), ("20", 3), ("50", 1)] {
        let plugin = plugin(&[("per_page", text(per_page))]);
        let walked = walk(&bench, &plugin, "x");
        assert_eq!(walked.len(), pages, "per_page {per_page}");
        let all: Vec<&str> = walked.iter().flat_map(ids).collect();
        assert_eq!(all.len(), TOTAL);
        assert_eq!(
            all.iter().collect::<BTreeSet<_>>().len(),
            TOTAL,
            "ids repeat"
        );
        assert!(
            walked.iter().all(|p| p.warnings.is_empty()),
            "{:?}",
            walked[0].warnings
        );
        assert!(walked.last().unwrap().page.next_cursor.is_none());
        assert!(
            walked[..walked.len() - 1]
                .iter()
                .all(|p| p.page.hits.len() == per_page.parse::<usize>().unwrap())
        );
    }
}

#[test]
fn the_page_number_works_without_a_cursor() {
    let bench = Bench::new();
    let plugin = plugin(&[("per_page", text("20"))]);
    let third = bench.search(&plugin, &json!({ "text": "x", "page": 3 }));
    assert_eq!(ids(&third).first().copied(), Some("book-41"));
    assert_eq!(third.page.hits.len(), 5);
    assert!(third.page.next_cursor.is_none());
    assert!(
        hit(&third, "book-41")
            .description
            .as_deref()
            .unwrap()
            .contains("page 3")
    );
}

#[test]
fn the_first_twelve_books_sit_on_the_limits_of_the_contract() {
    let bench = Bench::new();
    let plugin = plugin(&[("per_page", text("50"))]);
    let page = bench.first_page(&plugin, "x");

    let war = hit(&page, "book-01");
    assert_eq!(war.offers.len(), 4);
    assert!(war.offers.iter().any(|o| o.archive == Some(Archive::Zip)));
    assert_eq!(hit(&page, "book-02").title, "Война и мир");
    assert_eq!(hit(&page, "book-03").series_index, Some(2.0));
    let bare = hit(&page, "book-04");
    assert!(bare.authors.is_empty() && bare.offers.is_empty() && bare.year.is_none());
    assert!(bare.page_url.is_some());
    assert_eq!(hit(&page, "book-05").authors.len(), 20);
    assert_eq!(hit(&page, "book-06").title.chars().count(), 300);
    assert_eq!(hit(&page, "book-07").offers.len(), 10);
    assert_eq!(hit(&page, "book-10").tags.len(), 30);
    let long = hit(&page, "book-10").description.as_deref().unwrap();
    assert_eq!(long.chars().count(), 8000);
    assert!(long.contains('\n') && long.contains('\t'));
    let last = hit(&page, "book-11");
    assert_eq!(
        (last.year, last.language.as_deref()),
        (Some(2100), Some("pt-BR"))
    );
    let minimal = hit(&page, "book-12");
    assert!(minimal.description.is_none() && minimal.authors.is_empty());

    let formats: BTreeSet<String> = page
        .page
        .hits
        .iter()
        .flat_map(|h| &h.offers)
        .map(|o| serde_json::to_value(o.format).unwrap().to_string())
        .collect();
    assert_eq!(formats.len(), 7, "every format is offered: {formats:?}");
}

#[test]
fn the_settings_change_the_files_and_the_tags() {
    let bench = Bench::new();
    let bare = plugin(&[
        ("with_offers", SettingValue::Bool(false)),
        ("per_page", text("50")),
    ]);
    let page = bench.first_page(&bare, "x");
    assert!(page.page.hits.iter().all(|h| h.offers.is_empty()));
    // The text form of a switch counts too.
    let textual = plugin(&[("with_offers", text("false")), ("per_page", text("50"))]);
    let page = bench.first_page(&textual, "x");
    assert!(page.page.hits.iter().all(|h| h.offers.is_empty()));

    let tagged = plugin(&[("extra_tag", text("  from-settings "))]);
    let page = bench.first_page(&tagged, "x");
    assert!(
        page.page
            .hits
            .iter()
            .filter(|h| h.id != "book-10") // it has 30 tags already: the limit wins
            .all(|h| h.tags.last().map(String::as_str) == Some("from-settings"))
    );
    // The thirty tags of book-10 stay thirty.
    let wide = plugin(&[("extra_tag", text("one more")), ("per_page", text("50"))]);
    assert_eq!(hit(&bench.first_page(&wide, "x"), "book-10").tags.len(), 30);
}

// --- Service queries -----------------------------------------------------------------------

#[test]
fn service_queries_give_the_edge_cases() {
    let bench = Bench::new();
    let plugin = plugin(&[]);

    let empty = bench.first_page(&plugin, "!empty");
    assert!(empty.page.hits.is_empty() && empty.page.next_cursor.is_none());
    // Case and the spaces around do not matter.
    assert!(bench.first_page(&plugin, "  !EMPTY ").page.hits.is_empty());

    let single = bench.first_page(&plugin, "!single");
    assert_eq!(
        (single.page.hits.len(), single.page.next_cursor.clone()),
        (1, None)
    );

    let max = bench.first_page(&plugin, "!max");
    assert_eq!(max.page.hits.len(), 100);
    assert!(max.warnings.is_empty() && max.page.next_cursor.is_none());

    // An unknown word with "!" is an ordinary query.
    assert_eq!(bench.first_page(&plugin, "!no-such").page.hits.len(), 20);
    assert!(bench.server.requests().is_empty());
}

#[test]
fn invalid_hits_are_dropped_one_by_one_and_the_rest_stay() {
    let bench = Bench::new();
    let plugin = plugin(&[]);
    let page = bench.first_page(&plugin, "!invalid");
    assert_eq!(ids(&page), ["book-01", "book-02", "book-03"]);
    let dropped: BTreeSet<_> = page
        .warnings
        .iter()
        .filter_map(|w| w.id.as_deref())
        .collect();
    assert_eq!(
        dropped,
        BTreeSet::from([
            "bad-archive",
            "bad-authors",
            "bad-cover",
            "bad-format",
            "bad-isbn",
            "bad-no-title",
            "bad-year",
            "book-01",
        ]),
        "{:?}",
        page.warnings
    );
    // The result without a title has no readable cause besides the missing field.
    assert_eq!(page.warnings.len(), 8, "{:?}", page.warnings);
}

#[test]
fn pages_that_break_a_page_wide_rule_are_rejected_as_a_whole() {
    let bench = Bench::new();
    let plugin = plugin(&[]);
    let query = |text: &str| json!({ "text": text, "page": 1 });

    let too_many = bench.raw_search(&plugin, &query("!toomany")).unwrap();
    assert!(matches!(
        accept(&plugin, &too_many),
        Err(ResponseError::TooManyHits { count: 101 })
    ));
    let not_array = bench.raw_search(&plugin, &query("!reject")).unwrap();
    assert!(matches!(
        accept(&plugin, &not_array),
        Err(ResponseError::Malformed(_))
    ));
    // Over 2 MiB the sandbox itself refuses the answer.
    let huge = bench.raw_search(&plugin, &query("!huge")).unwrap_err();
    assert!(huge.contains("larger than"), "{huge}");
}

#[test]
fn the_error_query_fails_in_words_and_the_log_query_writes_every_level() {
    let bench = Bench::new();
    let plugin = plugin(&[]);
    let error = bench
        .raw_search(&plugin, &json!({ "text": "!error", "page": 1 }))
        .unwrap_err();
    assert!(error.contains("Mock Library failed on purpose"), "{error}");

    bench.logs.lock().unwrap().clear(); // the runtime logs the exception of `!error` too
    bench.first_page(&plugin, "!log");
    let levels: Vec<LogLevel> = bench.logs.lock().unwrap().iter().map(|r| r.level).collect();
    assert_eq!(
        levels,
        [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error
        ]
    );
}

// --- details and resolveDownload -----------------------------------------------------------

fn details_for(
    bench: &Bench,
    plugin: &LoadedPlugin,
    page: &ParsedPage,
    id: &str,
) -> Result<SearchHit, String> {
    let hit = hit(page, id);
    let answer = bench.call(
        plugin,
        PluginFunction::Details,
        &serde_json::to_value(hit).unwrap(),
    )?;
    let allow = plugin.allow_list().unwrap();
    merge_details(hit, &answer.to_string(), &|host| allow.allows_host(host))
        .map_err(|e| e.to_string())
}

#[test]
fn details_adds_what_the_table_says() {
    let bench = Bench::new();
    let plugin = plugin(&[("per_page", text("50"))]);
    let page = bench.first_page(&plugin, "x");

    let merged = details_for(&bench, &plugin, &page, "book-01").unwrap();
    assert_eq!(merged.publisher.as_deref(), Some("Mock Press"));
    assert!(merged.description.unwrap().contains("Extended description"));
    assert_eq!(merged.offers.len(), 4, "a book with files keeps its own");

    // A book without files gets two from details.
    let filler = ids(&page)
        .into_iter()
        .find(|id| hit(&page, id).offers.is_empty() && *id != "book-04" && *id != "book-12")
        .unwrap()
        .to_owned();
    let merged = details_for(&bench, &plugin, &page, &filler).unwrap();
    assert_eq!(merged.offers.len(), 2);
    assert_eq!(merged.id, filler);

    // Nothing to add: the result is unchanged.
    for id in ["book-04", "book-12"] {
        assert_eq!(
            &details_for(&bench, &plugin, &page, id).unwrap(),
            hit(&page, id)
        );
    }
    assert!(
        details_for(&bench, &plugin, &page, "book-05")
            .unwrap_err()
            .contains("failed on purpose")
    );
    // The year 0 breaks the contract after the merge.
    let broken = details_for(&bench, &plugin, &page, "book-06").unwrap_err();
    assert!(broken.contains("year"), "{broken}");

    let no_files = self::plugin(&[
        ("per_page", text("50")),
        ("with_offers", SettingValue::Bool(false)),
    ]);
    let page = bench.first_page(&no_files, "x");
    assert!(
        details_for(&bench, &no_files, &page, &filler)
            .unwrap()
            .offers
            .is_empty()
    );
    assert!(bench.server.requests().is_empty());
}

fn resolve(
    bench: &Bench,
    plugin: &LoadedPlugin,
    page: &ParsedPage,
    offer_id: &str,
) -> Result<(String, Option<Archive>), String> {
    let (owner, _) = offer_id.rsplit_once('-').unwrap();
    let offer = hit(page, owner)
        .offers
        .iter()
        .find(|o| o.id == offer_id)
        .unwrap_or_else(|| panic!("no offer {offer_id}"));
    let answer = bench.call(
        plugin,
        PluginFunction::ResolveDownload,
        &serde_json::to_value(offer).unwrap(),
    )?;
    let allow = plugin.allow_list().unwrap();
    parse_download_spec(&answer.to_string(), offer, &|host| allow.allows_host(host))
        .map(|spec| (spec.url, spec.archive))
        .map_err(|e| e.to_string())
}

#[test]
fn resolve_download_names_an_unresolvable_address_and_fails_where_asked() {
    let bench = Bench::new();
    let plugin = plugin(&[("per_page", text("50"))]);
    let page = bench.first_page(&plugin, "x");

    assert_eq!(
        resolve(&bench, &plugin, &page, "book-01-epub").unwrap(),
        (
            "https://mock.invalid/files/book-01-epub.epub".to_owned(),
            None
        )
    );
    assert_eq!(
        resolve(&bench, &plugin, &page, "book-01-fb2zip").unwrap(),
        (
            "https://mock.invalid/files/book-01-fb2zip.fb2.zip".to_owned(),
            Some(Archive::Zip)
        )
    );
    // A plain FB2 has no archive.
    assert_eq!(
        resolve(&bench, &plugin, &page, "book-07-fb2").unwrap().1,
        None
    );
    // Every offer of book-07 except the two bad ones resolves.
    for offer in hit(&page, "book-07")
        .offers
        .iter()
        .filter(|o| !o.id.ends_with("fail") && !o.id.ends_with("blocked"))
    {
        assert!(
            resolve(&bench, &plugin, &page, &offer.id).is_ok(),
            "{}",
            offer.id
        );
    }
    assert!(
        resolve(&bench, &plugin, &page, "book-07-fail")
            .unwrap_err()
            .contains("failed on purpose")
    );
    let blocked = resolve(&bench, &plugin, &page, "book-07-blocked").unwrap_err();
    assert!(blocked.contains("example.org"), "{blocked}");
    assert!(bench.server.requests().is_empty());
}
