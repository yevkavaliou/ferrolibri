//! `DownloadAndAdd` through the real `JobExecutor`, a local server in place of the site, a stand-in
//! for `calibredb` (`BookAdder`) and plugins written inline. The `calibredb` itself is exercised by
//! the ignored test in `tests/download_real_calibre.rs`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use ferrolibri_core::{
    Error, ErrorKind,
    book::Format,
    calibre::{AddOutcome, BookAdder, NewBookMetadata},
    converter::CancellationToken,
    jobs::{
        AddedBook, Context, ExternalJob, JobEvent, JobExecutor, JobHandle, JobOutput, JobRequest,
        JobState, Stage, SubmitError,
    },
};
use tempfile::TempDir;

use super::DownloadAndAdd;
use crate::{
    Archive, LoadedPlugin, Manifest, Offer, PluginRuntime, SearchHit, SettingValue,
    testing::{Reply, TestServer, opds, single_file_zip, zip_declaring, zip_stored},
};

const WAIT: Duration = Duration::from_secs(10);

const HTML: &str = "<html><head><title>502 Bad Gateway</title></head><body>nginx</body></html>";

fn epub_bytes() -> Vec<u8> {
    zip_stored(&[
        ("mimetype", b"application/epub+zip"),
        ("OEBPS/a.xhtml", b"x"),
    ])
}

/// The site: the OPDS catalog stand-in (a two-hop redirect chain ending in a zip at
/// `/b/12/fb2`) plus routes for each way a download can go wrong.
fn site(request: &crate::testing::Request, context: &crate::testing::Context) -> Reply {
    match request.path.as_str() {
        "/html200" => Reply::html(200, HTML),
        "/html502" => Reply::html(502, HTML),
        "/html404" => Reply::html(404, "not found"),
        "/zip-html" => Reply::ok("application/zip", HTML),
        "/zip-no-fb2" => Reply::ok("application/zip", zip_stored(&[("a.txt", b"x")])),
        "/zip-two-files" => Reply::ok(
            "application/zip",
            zip_stored(&[("a.fb2", b"<FictionBook/>"), ("b.fb2", b"<FictionBook/>")]),
        ),
        "/zip-bomb" => Reply::ok("application/zip", zip_declaring(&[("a.fb2", 1 << 30)])),
        "/epub" => Reply::ok("application/epub+zip", epub_bytes()),
        "/epub-no-mimetype" => Reply::ok("application/epub+zip", zip_stored(&[("a.xhtml", b"x")])),
        "/epub-named" => Reply::ok("application/epub+zip", epub_bytes()).header(
            "Content-Disposition",
            "attachment; filename=\"../../evil.exe\"",
        ),
        "/big" => Reply::repeated("application/epub+zip", b'x', 5000),
        "/big-chunked" => Reply::ok("application/epub+zip", vec![b'x'; 5000]).chunked(),
        "/chunked-epub" => Reply::ok("application/epub+zip", epub_bytes()).chunked(),
        "/stall" => Reply::ok("application/epub+zip", vec![b'x'; 4000]).stall_after(100),
        "/slow" => Reply::ok("text/plain", "late").delay(Duration::from_secs(20)),
        "/to-other-host" => Reply::redirect(302, &context.url("elsewhere.test", "/epub")),
        _ => opds::handle(request, context),
    }
}

type Behaviour = Box<dyn Fn(&CancellationToken) -> Result<AddOutcome, Error> + Send + Sync>;

/// Records every call, and answers with `behaviour`.
struct FakeAdder {
    calls: Mutex<Vec<Call>>,
    behaviour: Behaviour,
}

#[derive(Debug, Clone)]
struct Call {
    library: PathBuf,
    file: PathBuf,
    /// What the file held when `calibredb` was called.
    bytes: Vec<u8>,
    metadata: NewBookMetadata,
}

impl std::fmt::Debug for FakeAdder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeAdder")
    }
}

impl FakeAdder {
    fn new(
        behaviour: impl Fn(&CancellationToken) -> Result<AddOutcome, Error> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            behaviour: Box::new(behaviour),
        })
    }

    fn adds(id: u64) -> Arc<Self> {
        Self::new(move |_| Ok(AddOutcome::Added { id }))
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl BookAdder for FakeAdder {
    fn add_book(
        &self,
        library: &Path,
        file: &Path,
        metadata: &NewBookMetadata,
        cancel: &CancellationToken,
    ) -> Result<AddOutcome, Error> {
        self.calls.lock().unwrap().push(Call {
            library: library.to_owned(),
            file: file.to_owned(),
            bytes: std::fs::read(file).expect("the file exists when calibredb is called"),
            metadata: metadata.clone(),
        });
        (self.behaviour)(cancel)
    }
}

fn add_failure() -> Error {
    Error {
        operation: "add book to Calibre library",
        path: None,
        kind: ErrorKind::InvalidData("calibredb refused".into()),
    }
}

fn cancelled() -> Error {
    Error {
        operation: "add book to Calibre library",
        path: None,
        kind: ErrorKind::Cancelled,
    }
}

/// A plugin that hands the offer's `id` back as the address, as `sample-opds` does.
fn plugin(id: &str, source: &str, server: &TestServer) -> LoadedPlugin {
    let manifest = Manifest::parse(&format!(
        "id = \"{id}\"\nname = \"Test\"\nversion = \"0.1.0\"\napi_version = 1\n\
         provides = [\"search\"]\n\
         [[settings]]\nkey = \"base_url\"\ntype = \"url\"\nlabel = \"Address\"\nnetwork = true\n"
    ))
    .unwrap();
    LoadedPlugin {
        manifest,
        source: source.to_owned(),
        settings: BTreeMap::from([("base_url".to_owned(), SettingValue::Text(server.url("/")))]),
    }
}

const IDENTITY: &str = "export function resolveDownload(offer, ctx) { return { url: offer.id }; }";

fn offer(server: &TestServer, path: &str, format: Format, archive: Option<Archive>) -> Offer {
    Offer {
        id: server.url(path),
        format,
        archive,
        size: None,
        label: None,
    }
}

fn hit(title: &str) -> SearchHit {
    SearchHit {
        id: "1".into(),
        title: title.into(),
        authors: vec!["Толстой, Лев".into()],
        isbn: Some("9780140447934".into()),
        language: Some("ru".into()),
        series: Some("Series".into()),
        series_index: Some(2.0),
        tags: vec!["classic".into()],
        ..SearchHit::default()
    }
}

struct Setup {
    server: TestServer,
    dir: TempDir,
    library: PathBuf,
    executor: JobExecutor,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            server: TestServer::start(site),
            library: library_in(dir.path(), "library"),
            executor: JobExecutor::new(2, 8).unwrap(),
            dir,
        }
    }

    fn runtime(&self) -> PluginRuntime {
        PluginRuntime::new(self.server.http()).with_max_call_time(Duration::from_secs(5))
    }

    fn job(
        &self,
        source: &str,
        offer: Offer,
        title: &str,
        adder: &Arc<FakeAdder>,
    ) -> DownloadAndAdd {
        DownloadAndAdd::new(
            self.runtime(),
            plugin("test", source, &self.server),
            hit(title),
            offer,
            self.library.clone(),
            Some(adder.clone() as Arc<dyn BookAdder>),
        )
    }

    /// Downloads `path` as `format` and returns the events and the result.
    fn run(
        &self,
        path: &str,
        format: Format,
        archive: Option<Archive>,
        adder: &Arc<FakeAdder>,
    ) -> (Vec<JobEvent>, Result<JobOutput, Error>) {
        let job = self.job(
            IDENTITY,
            offer(&self.server, path, format, archive),
            "Война и мир",
            adder,
        );
        self.finish(job)
    }

    fn finish(&self, job: DownloadAndAdd) -> (Vec<JobEvent>, Result<JobOutput, Error>) {
        let handle = self
            .executor
            .submit(job.into_request())
            .unwrap_or_else(|e| {
                panic!("not accepted: {e:?}");
            });
        (events_until_end(&handle), handle.wait())
    }
}

/// A folder the core takes for a Calibre library (it only looks for `metadata.db`).
fn library_in(root: &Path, name: &str) -> PathBuf {
    let library = root.join(name);
    std::fs::create_dir(&library).unwrap();
    std::fs::write(library.join("metadata.db"), b"synthetic").unwrap();
    library
}

fn is_end(event: &JobEvent) -> bool {
    matches!(
        event,
        JobEvent::State {
            state: JobState::Succeeded | JobState::Failed | JobState::Cancelled,
            ..
        }
    )
}

fn events_until_end(handle: &JobHandle) -> Vec<JobEvent> {
    let mut events = Vec::new();
    loop {
        let event = handle.events.recv_timeout(WAIT).expect("the job ends");
        let end = is_end(&event);
        events.push(event);
        if end {
            return events;
        }
    }
}

/// Reads events until `wanted` matches one; returns what was read.
fn wait_for(handle: &JobHandle, wanted: impl Fn(&JobEvent) -> bool) -> Vec<JobEvent> {
    let mut seen = Vec::new();
    loop {
        let event = handle.events.recv_timeout(WAIT).expect("the event comes");
        let found = wanted(&event);
        seen.push(event);
        if found {
            return seen;
        }
    }
}

fn stages(events: &[JobEvent]) -> Vec<Stage> {
    events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Stage { stage, .. } => Some(*stage),
            _ => None,
        })
        .collect()
}

fn progress(events: &[JobEvent]) -> Vec<(u64, Option<u64>)> {
    events
        .iter()
        .filter_map(|e| match e {
            JobEvent::DownloadProgress {
                completed, total, ..
            } => Some((*completed, *total)),
            _ => None,
        })
        .collect()
}

fn added(output: Result<JobOutput, Error>) -> AddedBook {
    match output.expect("the job succeeded") {
        JobOutput::Added(book) => book,
        other => panic!("not an addition: {other:?}"),
    }
}

fn message(error: &Error) -> String {
    error.to_string()
}

fn is_clean(setup: &Setup, call: &Call) {
    assert!(!call.file.exists(), "the workspace was not removed");
    assert!(!call.file.parent().unwrap().exists());
    assert_eq!(
        std::fs::read_dir(&setup.library).unwrap().count(),
        1,
        "only metadata.db is in the library"
    );
}

// ---- the successful path ------------------------------------------------------------------

#[test]
fn a_zipped_fb2_comes_through_two_redirects_and_is_added_with_the_metadata_of_the_result() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(7);
    let (events, result) = setup.run("/b/12/fb2", Format::Fb2, Some(Archive::Zip), &adder);

    let book = added(result);
    assert_eq!(book.outcome, AddOutcome::Added { id: 7 });
    assert_eq!(book.title, "Война и мир");
    assert_eq!(book.library, setup.library);

    // The stages come in order, and the job ends in success.
    assert_eq!(stages(&events), [Stage::Downloading, Stage::Adding]);
    assert!(matches!(
        events.last(),
        Some(JobEvent::State {
            state: JobState::Succeeded,
            ..
        })
    ));

    // The second redirect went to a sibling subdomain of the address the user entered.
    let hosts: Vec<_> = setup
        .server
        .requests()
        .iter()
        .map(|r| r.host.clone())
        .collect();
    assert!(
        hosts.contains(&"static.catalog.test".to_owned()),
        "{hosts:?}"
    );

    let calls = adder.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.library, setup.library);
    assert_eq!(call.file.file_name().unwrap(), "Война и мир.fb2.zip");
    assert_eq!(call.bytes, opds::book_zip(&opds::BOOKS[1]));
    assert_eq!(call.metadata.title.as_deref(), Some("Война и мир"));
    assert_eq!(call.metadata.authors, ["Толстой, Лев"]);
    assert_eq!(call.metadata.isbn.as_deref(), Some("9780140447934"));
    assert_eq!(call.metadata.languages, ["ru"]);
    assert_eq!(call.metadata.series.as_deref(), Some("Series"));
    assert_eq!(call.metadata.series_index, Some(2.0));
    assert_eq!(call.metadata.tags, ["classic"]);
    is_clean(&setup, call);
}

#[test]
fn progress_has_the_total_when_the_server_announces_it_and_ends_complete() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let (events, result) = setup.run("/epub", Format::Epub, None, &adder);
    added(result);
    let figures = progress(&events);
    let size = epub_bytes().len() as u64;
    assert_eq!(figures.first(), Some(&(0, Some(size))));
    assert_eq!(figures.last(), Some(&(size, Some(size))));
    assert!(figures.windows(2).all(|w| w[0].0 <= w[1].0));
}

#[test]
fn progress_has_no_total_when_the_server_does_not_announce_one() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let (events, result) = setup.run("/chunked-epub", Format::Epub, None, &adder);
    added(result);
    let figures = progress(&events);
    assert_eq!(figures.first(), Some(&(0, None)));
    let size = epub_bytes().len() as u64;
    // Only the very last report knows the size: what arrived.
    assert_eq!(figures.last(), Some(&(size, Some(size))));
    assert!(
        figures[..figures.len() - 1]
            .iter()
            .all(|(_, total)| total.is_none())
    );
}

#[test]
fn a_duplicate_is_a_result_and_not_an_error() {
    let setup = Setup::new();
    let adder = FakeAdder::new(|_| Ok(AddOutcome::AlreadyExists));
    let (events, result) = setup.run("/epub", Format::Epub, None, &adder);
    assert_eq!(added(result).outcome, AddOutcome::AlreadyExists);
    assert!(matches!(
        events.last(),
        Some(JobEvent::State {
            state: JobState::Succeeded,
            ..
        })
    ));
    is_clean(&setup, &adder.calls()[0]);
}

// ---- not a book --------------------------------------------------------------------------

fn refused(setup: &Setup, path: &str, format: Format, archive: Option<Archive>) -> String {
    let adder = FakeAdder::adds(1);
    let (events, result) = setup.run(path, format, archive, &adder);
    let error = result.expect_err("refused");
    assert!(
        adder.calls().is_empty(),
        "{path}: calibredb must not be called"
    );
    assert!(matches!(
        events.last(),
        Some(JobEvent::State {
            state: JobState::Failed,
            ..
        })
    ));
    assert!(!stages(&events).contains(&Stage::Adding), "{path}");
    message(&error)
}

#[test]
fn an_html_page_is_refused_whatever_its_status_or_type_and_the_message_says_what_came() {
    let setup = Setup::new();
    // 200 text/html, and 200 application/zip with an HTML body: the type is not looked at.
    for path in ["/html200", "/zip-html"] {
        let text = refused(&setup, path, Format::Fb2, Some(Archive::Zip));
        assert!(
            text.contains("did not return a book")
                && text.contains(&format!("HTML, {} bytes", HTML.len())),
            "{path}: {text}"
        );
        let text = refused(&setup, path, Format::Epub, None);
        assert!(
            text.contains(&format!("HTML, {} bytes", HTML.len())),
            "{path}: {text}"
        );
    }
}

#[test]
fn an_error_status_is_refused_before_anything_is_written() {
    let setup = Setup::new();
    let text = refused(&setup, "/html502", Format::Epub, None);
    assert!(text.contains("status 502"), "{text}");
    let text = refused(&setup, "/html404", Format::Epub, None);
    assert!(text.contains("status 404"), "{text}");
}

#[test]
fn a_zip_without_an_fb2_is_not_a_zipped_fb2() {
    let setup = Setup::new();
    let text = refused(&setup, "/zip-no-fb2", Format::Fb2, Some(Archive::Zip));
    assert!(
        text.contains("did not return a book") && text.contains("ZIP archive"),
        "{text}"
    );
}

#[test]
fn an_archive_with_several_files_is_refused() {
    let setup = Setup::new();
    let text = refused(&setup, "/zip-two-files", Format::Fb2, Some(Archive::Zip));
    assert!(text.contains("did not return a book"), "{text}");
}

#[test]
fn an_archive_that_claims_to_unpack_beyond_the_limit_is_refused_without_unpacking() {
    let setup = Setup::new();
    let text = refused(&setup, "/zip-bomb", Format::Fb2, Some(Archive::Zip));
    assert!(text.contains("did not return a book"), "{text}");
}

#[test]
fn an_epub_without_a_mimetype_entry_is_refused() {
    let setup = Setup::new();
    let text = refused(&setup, "/epub-no-mimetype", Format::Epub, None);
    assert!(text.contains("did not return a book"), "{text}");
    // The same bytes as a zipped FB2 fail too: there is no fb2 in them.
    let text = refused(&setup, "/epub-no-mimetype", Format::Fb2, Some(Archive::Zip));
    assert!(text.contains("did not return a book"), "{text}");
}

#[test]
fn a_zipped_fb2_that_arrives_as_a_plain_fb2_offer_is_not_accepted() {
    // The offer promised a bare FB2; a zip is not one.
    let setup = Setup::new();
    let text = refused(&setup, "/b/12/fb2", Format::Fb2, None);
    assert!(text.contains("did not return a book"), "{text}");
}

// ---- limits and addresses -----------------------------------------------------------------

#[test]
fn a_body_larger_than_the_limit_is_refused_by_its_announced_size_or_as_it_arrives() {
    let setup = Setup::new();
    for path in ["/big", "/big-chunked"] {
        let adder = FakeAdder::adds(1);
        let job = setup
            .job(
                IDENTITY,
                offer(&setup.server, path, Format::Epub, None),
                "Big",
                &adder,
            )
            .with_max_bytes(1000);
        let (_, result) = setup.finish(job);
        let text = message(&result.expect_err("too large"));
        assert!(text.contains("larger than 1000 bytes"), "{path}: {text}");
        assert!(adder.calls().is_empty());
    }
}

#[test]
fn a_redirect_to_a_host_the_user_did_not_allow_is_blocked() {
    let setup = Setup::new();
    let text = refused(&setup, "/to-other-host", Format::Epub, None);
    assert!(
        text.contains("blocked_domain") && text.contains("elsewhere.test"),
        "{text}"
    );
}

#[test]
fn an_address_on_a_host_the_plugin_may_not_reach_never_leaves_the_plugin_check() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let job = setup.job(
        "export function resolveDownload(offer, ctx) { return { url: 'http://evil.test/x.epub' }; }",
        offer(&setup.server, "/epub", Format::Epub, None),
        "Book",
        &adder,
    );
    let (_, result) = setup.finish(job);
    let error = result.expect_err("refused");
    assert!(
        matches!(&error.kind, ErrorKind::Plugin { id, message } if id == "test" && message.contains("evil.test")),
        "{error}"
    );
    assert!(setup.server.requests().is_empty(), "no request was made");
}

#[test]
fn a_plugin_without_resolve_download_is_a_plugin_error() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let job = setup.job(
        "export function search() { return { hits: [] }; }",
        offer(&setup.server, "/epub", Format::Epub, None),
        "Book",
        &adder,
    );
    let (_, result) = setup.finish(job);
    assert!(matches!(
        result.expect_err("refused").kind,
        ErrorKind::Plugin { .. }
    ));
}

// ---- file names --------------------------------------------------------------------------

#[test]
fn the_file_name_is_made_by_the_app_from_the_title_and_the_format() {
    let setup = Setup::new();
    for (title, expected) in [
        ("../../evil", "evil.epub"),
        ("..\\..\\evil", "evil.epub"),
        ("CON", "_CON.epub"),
        ("nul.txt", "_nul.txt.epub"),
        ("a\0b\u{202E}c", "abc.epub"),
        ("   ", "book.epub"),
    ] {
        let adder = FakeAdder::adds(1);
        // The server names the file `../../evil.exe` and its URL ends in `/epub-named`: neither is used.
        let job = setup.job(
            IDENTITY,
            offer(&setup.server, "/epub-named", Format::Epub, None),
            title,
            &adder,
        );
        let (_, result) = setup.finish(job);
        added(result);
        let call = &adder.calls()[0];
        assert_eq!(call.file.file_name().unwrap(), expected, "{title:?}");
        // Directly inside the private workspace, nowhere else.
        let workspace = call.file.parent().unwrap().to_owned();
        assert!(
            workspace
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ferrolibri-conversion-")
        );
    }
}

#[test]
fn a_very_long_title_gives_a_short_name() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let title = "Очень длинное название ".repeat(40);
    let job = setup.job(
        IDENTITY,
        offer(&setup.server, "/epub", Format::Epub, None),
        &title,
        &adder,
    );
    let (_, result) = setup.finish(job);
    added(result);
    let name = adder.calls()[0]
        .file
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.chars().count() <= super::MAX_NAME_CHARS + ".epub".len(),
        "{name}"
    );
    // The metadata keeps the whole title.
    assert_eq!(
        adder.calls()[0].metadata.title.as_deref(),
        Some(title.as_str())
    );
}

// ---- when calibredb fails ------------------------------------------------------------------

#[test]
fn a_failed_addition_keeps_the_file_in_the_export_folder_and_reports_where() {
    let setup = Setup::new();
    let export = tempfile::tempdir().unwrap();
    let adder = FakeAdder::new(|_| Err(add_failure()));
    let job = setup
        .job(
            IDENTITY,
            offer(&setup.server, "/epub", Format::Epub, None),
            "Война и мир",
            &adder,
        )
        .with_export_dir(export.path().to_owned());
    let (events, result) = setup.finish(job);
    let error = result.expect_err("failed");
    let ErrorKind::BookNotAdded { cause, kept } = &error.kind else {
        panic!("{error}");
    };
    assert!(cause.to_string().contains("calibredb refused"));
    let kept = kept.clone().expect("the file was kept");
    assert_eq!(
        kept.parent().unwrap(),
        export.path().canonicalize().unwrap()
    );
    assert_eq!(std::fs::read(&kept).unwrap(), epub_bytes());
    assert!(
        message(&error).contains(&kept.display().to_string()),
        "{error}"
    );
    assert!(matches!(
        events.last(),
        Some(JobEvent::State {
            state: JobState::Failed,
            ..
        })
    ));
    // The workspace is gone all the same.
    is_clean(&setup, &adder.calls()[0]);
}

#[test]
fn a_kept_file_does_not_replace_one_that_is_already_there() {
    let setup = Setup::new();
    let export = tempfile::tempdir().unwrap();
    std::fs::write(export.path().join("Книга.epub"), b"mine").unwrap();
    let adder = FakeAdder::new(|_| Err(add_failure()));
    let job = setup
        .job(
            IDENTITY,
            offer(&setup.server, "/epub", Format::Epub, None),
            "Книга",
            &adder,
        )
        .with_export_dir(export.path().to_owned());
    let (_, result) = setup.finish(job);
    let ErrorKind::BookNotAdded { kept, .. } = result.expect_err("failed").kind else {
        panic!("wrong kind");
    };
    assert_eq!(kept.unwrap().file_name().unwrap(), "Книга (1).epub");
    assert_eq!(
        std::fs::read(export.path().join("Книга.epub")).unwrap(),
        b"mine"
    );
}

#[test]
fn without_an_export_folder_a_failed_addition_keeps_nothing() {
    let setup = Setup::new();
    let adder = FakeAdder::new(|_| Err(add_failure()));
    let (_, result) = setup.run("/epub", Format::Epub, None, &adder);
    let ErrorKind::BookNotAdded { kept, .. } = result.expect_err("failed").kind else {
        panic!("wrong kind");
    };
    assert!(kept.is_none());
    is_clean(&setup, &adder.calls()[0]);
}

#[test]
fn an_export_folder_inside_the_library_is_not_used() {
    let setup = Setup::new();
    let inside = setup.library.join("export");
    std::fs::create_dir(&inside).unwrap();
    let adder = FakeAdder::new(|_| Err(add_failure()));
    let job = setup
        .job(
            IDENTITY,
            offer(&setup.server, "/epub", Format::Epub, None),
            "Book",
            &adder,
        )
        .with_export_dir(inside.clone());
    let (_, result) = setup.finish(job);
    let ErrorKind::BookNotAdded { kept, .. } = result.expect_err("failed").kind else {
        panic!("wrong kind");
    };
    assert!(kept.is_none());
    assert_eq!(std::fs::read_dir(&inside).unwrap().count(), 0);
}

#[test]
fn a_file_that_was_refused_as_not_a_book_is_not_kept() {
    let setup = Setup::new();
    let export = tempfile::tempdir().unwrap();
    let adder = FakeAdder::adds(1);
    let job = setup
        .job(
            IDENTITY,
            offer(&setup.server, "/html200", Format::Epub, None),
            "Book",
            &adder,
        )
        .with_export_dir(export.path().to_owned());
    let (_, result) = setup.finish(job);
    result.expect_err("refused");
    assert_eq!(std::fs::read_dir(export.path()).unwrap().count(), 0);
}

// ---- cancellation ------------------------------------------------------------------------

fn submit(setup: &Setup, job: DownloadAndAdd) -> JobHandle {
    setup
        .executor
        .submit(job.into_request())
        .unwrap_or_else(|e| panic!("{e:?}"))
}

fn assert_cancelled(handle: JobHandle, within: Duration) {
    let started = Instant::now();
    let error = handle.wait().expect_err("cancelled");
    assert!(matches!(error.kind, ErrorKind::Cancelled), "{error}");
    assert!(started.elapsed() < within, "took {:?}", started.elapsed());
}

#[test]
fn cancelling_a_queued_download_never_touches_the_network() {
    let setup = Setup {
        executor: JobExecutor::new(1, 4).unwrap(),
        ..Setup::new()
    };
    // The only worker is busy with a job on another path.
    let other = library_in(setup.dir.path(), "other");
    let (release, gate) = mpsc::channel();
    let (started, running) = mpsc::channel();
    let blocker = setup
        .executor
        .submit(JobRequest::External(Arc::new(Blocker {
            path: other,
            started,
            gate: Mutex::new(gate),
        })))
        .unwrap_or_else(|e| panic!("{e:?}"));
    running.recv_timeout(WAIT).unwrap();

    let adder = FakeAdder::adds(1);
    let job = setup.job(
        IDENTITY,
        offer(&setup.server, "/epub", Format::Epub, None),
        "Book",
        &adder,
    );
    let queued = submit(&setup, job);
    queued.cancel();
    assert_cancelled(queued, Duration::from_secs(2));
    release.send(()).unwrap();
    blocker.wait().unwrap();
    assert!(setup.server.requests().is_empty());
    assert!(adder.calls().is_empty());
}

#[test]
fn cancelling_while_the_plugin_resolves_the_address_stops_the_call() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let job = setup.job(
        "export function resolveDownload(offer, ctx) { host.fetch(offer.id); return { url: offer.id }; }",
        offer(&setup.server, "/slow", Format::Epub, None),
        "Book",
        &adder,
    );
    let handle = submit(&setup, job);
    wait_for(&handle, |e| {
        matches!(
            e,
            JobEvent::Stage {
                stage: Stage::Downloading,
                ..
            }
        )
    });
    thread::sleep(Duration::from_millis(300));
    handle.cancel();
    assert_cancelled(handle, Duration::from_secs(3));
    assert!(adder.calls().is_empty());
}

#[test]
fn cancelling_during_the_download_stops_a_silent_server_within_a_moment() {
    let setup = Setup::new();
    let adder = FakeAdder::adds(1);
    let job = setup.job(
        IDENTITY,
        offer(&setup.server, "/stall", Format::Epub, None),
        "Book",
        &adder,
    );
    let handle = submit(&setup, job);
    // The first report means the headers are in and the body is not coming.
    wait_for(&handle, |e| matches!(e, JobEvent::DownloadProgress { .. }));
    handle.cancel();
    assert_cancelled(handle, Duration::from_secs(3));
    assert!(adder.calls().is_empty());
}

#[test]
fn cancelling_during_the_addition_is_a_cancellation_and_keeps_nothing() {
    let setup = Setup::new();
    let export = tempfile::tempdir().unwrap();
    let adder = FakeAdder::new(|cancel| {
        let deadline = Instant::now() + WAIT;
        while !cancel.is_cancelled() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        Err(cancelled())
    });
    let job = setup
        .job(
            IDENTITY,
            offer(&setup.server, "/epub", Format::Epub, None),
            "Book",
            &adder,
        )
        .with_export_dir(export.path().to_owned());
    let handle = submit(&setup, job);
    wait_for(&handle, |e| {
        matches!(
            e,
            JobEvent::Stage {
                stage: Stage::Adding,
                ..
            }
        )
    });
    // The stage is announced just before the adder is called: cancel only once it is running.
    let deadline = Instant::now() + WAIT;
    while adder.calls().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    handle.cancel();
    assert_cancelled(handle, Duration::from_secs(3));
    assert_eq!(std::fs::read_dir(export.path()).unwrap().count(), 0);
    is_clean(&setup, &adder.calls()[0]);
}

// ---- admission and conflicts ---------------------------------------------------------------

/// Holds a path until released: stands in for a long job that writes there.
#[derive(Debug)]
struct Blocker {
    path: PathBuf,
    started: mpsc::Sender<()>,
    gate: Mutex<mpsc::Receiver<()>>,
}

impl ExternalJob for Blocker {
    fn title(&self) -> String {
        "blocker".into()
    }
    fn writes_to(&self) -> Vec<PathBuf> {
        vec![self.path.clone()]
    }
    fn run(&self, _: &Context) -> Result<JobOutput, Error> {
        self.started.send(()).unwrap();
        match self.gate.lock().unwrap().recv_timeout(WAIT) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => Ok(JobOutput::Sent("done".into())),
            Err(RecvTimeoutError::Timeout) => panic!("never released"),
        }
    }
}

#[test]
fn without_calibre_the_job_is_not_accepted_and_says_why() {
    let setup = Setup::new();
    let job = DownloadAndAdd::new(
        setup.runtime(),
        plugin("test", IDENTITY, &setup.server),
        hit("Book"),
        offer(&setup.server, "/epub", Format::Epub, None),
        setup.library.clone(),
        None,
    );
    let Err(SubmitError::Unavailable(reason)) = setup.executor.submit(job.into_request()) else {
        panic!("accepted");
    };
    assert!(reason.contains("Calibre"), "{reason}");
    assert!(setup.server.requests().is_empty());
}

#[test]
fn two_additions_to_one_library_do_not_run_together_but_other_libraries_and_plugins_do() {
    let setup = Setup::new();
    let (entered, in_add) = mpsc::channel();
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let slow = FakeAdder::new(move |_| {
        entered.send(()).unwrap();
        let _ = gate.lock().unwrap().recv_timeout(WAIT);
        Ok(AddOutcome::Added { id: 1 })
    });
    let first = submit(
        &setup,
        setup.job(
            IDENTITY,
            offer(&setup.server, "/epub", Format::Epub, None),
            "First",
            &slow,
        ),
    );
    in_add.recv_timeout(WAIT).unwrap();

    let adder = FakeAdder::adds(2);
    let another = |library: &Path| {
        DownloadAndAdd::new(
            setup.runtime(),
            plugin("other-plugin", IDENTITY, &setup.server),
            hit("Second"),
            offer(&setup.server, "/epub", Format::Epub, None),
            library.to_owned(),
            Some(adder.clone() as Arc<dyn BookAdder>),
        )
    };
    // The same library, even for another plugin: refused for now (the queue of the UI retries).
    assert!(matches!(
        setup
            .executor
            .submit(another(&setup.library).into_request()),
        Err(SubmitError::Conflict)
    ));
    // A folder inside the library counts as the library.
    let inside = setup.library.join("inside");
    std::fs::create_dir(&inside).unwrap();
    assert!(matches!(
        setup.executor.submit(another(&inside).into_request()),
        Err(SubmitError::Conflict)
    ));

    // Another library and another plugin: runs at once, while the first is still adding.
    let elsewhere = library_in(setup.dir.path(), "second-library");
    let third = submit(&setup, another(&elsewhere));
    assert_eq!(added(third.wait()).library, elsewhere);

    // Once the first has finished, the library is free again.
    release.send(()).unwrap();
    assert_eq!(added(first.wait()).outcome, AddOutcome::Added { id: 1 });
    let again = submit(&setup, another(&setup.library));
    assert_eq!(added(again.wait()).outcome, AddOutcome::Added { id: 2 });
}

#[test]
fn a_cut_off_zipped_fb2_is_refused() {
    let bytes = single_file_zip("a.fb2", b"<FictionBook/>");
    let server = TestServer::start(move |_, _| {
        Reply::ok("application/zip", bytes[..bytes.len() - 5].to_vec())
    });
    let setup = Setup {
        server,
        ..Setup::new()
    };
    let text = refused(&setup, "/x", Format::Fb2, Some(Archive::Zip));
    assert!(text.contains("did not return a book"), "{text}");
}
