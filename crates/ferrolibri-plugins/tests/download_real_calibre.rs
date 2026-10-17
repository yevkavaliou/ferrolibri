//! The download job with the real `calibredb`: the one place where the whole chain — a plugin's
//! `resolveDownload`, a download over two redirects, the check of the zip, `calibredb add` and a
//! scan of the library — runs without a stand-in for Calibre. The site is the local stand-in for
//! the OPDS catalog; nothing leaves the machine.
//!
//! Run with: `cargo test -p ferrolibri-plugins --test download_real_calibre -- --ignored`

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use ferrolibri_core::{
    book::Format,
    calibre::{self, AddOutcome, BookAdder},
    converter::CancellationToken,
    jobs::{JobExecutor, JobOutput},
    scanner,
};
use ferrolibri_plugins::{
    Archive, DownloadAndAdd, LoadedPlugin, Manifest, Offer, PluginRuntime, SearchHit, SettingValue,
    testing::{TestServer, opds},
};

fn plugin(server: &TestServer) -> LoadedPlugin {
    let manifest = Manifest::parse(
        "id = \"real-calibre\"\nname = \"Test\"\nversion = \"0.1.0\"\napi_version = 1\n\
         provides = [\"search\"]\n\
         [[settings]]\nkey = \"base_url\"\ntype = \"url\"\nlabel = \"Address\"\nnetwork = true\n",
    )
    .unwrap();
    LoadedPlugin {
        manifest,
        source: "export function resolveDownload(offer, ctx) { return { url: offer.id }; }".into(),
        settings: BTreeMap::from([("base_url".to_owned(), SettingValue::Text(server.url("/")))]),
    }
}

#[test]
#[ignore = "needs Calibre: runs the real calibredb"]
fn a_zipped_fb2_is_downloaded_added_and_listed_as_fb2_and_a_second_try_finds_it_there() {
    let cli = calibre::require().expect("install Calibre to run this test");
    eprintln!(
        "calibredb {} at {}",
        cli.version().unwrap_or("?"),
        cli.path().display()
    );

    // A short path: Calibre refuses library paths over 88 characters on Windows.
    let root = tempfile::Builder::new().prefix("fl").tempdir().unwrap();
    let library = root.path().join("lib");
    std::fs::create_dir(&library).unwrap();
    cli.create_library(&library, &CancellationToken::default())
        .unwrap();

    let server = TestServer::start(opds::handle);
    let book = &opds::BOOKS[1];
    let make_job = || {
        DownloadAndAdd::new(
            PluginRuntime::new(server.http()).with_max_call_time(Duration::from_secs(10)),
            plugin(&server),
            SearchHit {
                id: book.id.to_string(),
                title: book.title.into(),
                authors: vec![book.author.into()],
                language: Some("ru".into()),
                ..SearchHit::default()
            },
            Offer {
                id: server.url(&format!("/b/{}/fb2", book.id)),
                format: Format::Fb2,
                archive: Some(Archive::Zip),
                size: None,
                label: None,
            },
            library.clone(),
            Some(Arc::new(cli.clone()) as Arc<dyn BookAdder>),
        )
    };

    let executor = JobExecutor::new(1, 2).unwrap();
    let started = Instant::now();
    let JobOutput::Added(first) = executor
        .submit(make_job().into_request())
        .unwrap_or_else(|e| panic!("{e:?}"))
        .wait()
        .unwrap()
    else {
        panic!("not an addition");
    };
    eprintln!("downloaded and added in {:?}", started.elapsed());
    assert!(
        matches!(first.outcome, AddOutcome::Added { .. }),
        "{:?}",
        first.outcome
    );

    // The library lists the book, and its file is an FB2.
    let report = scanner::scan_library(&library).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.books.len(), 1, "{:?}", report.books);
    assert_eq!(report.books[0].title, book.title);
    assert_eq!(
        report.books[0]
            .files
            .iter()
            .map(|f| f.format)
            .collect::<Vec<_>>(),
        [Format::Fb2]
    );

    // The same book again: Calibre refuses it as a duplicate, which is a result.
    let JobOutput::Added(second) = executor
        .submit(make_job().into_request())
        .unwrap_or_else(|e| panic!("{e:?}"))
        .wait()
        .unwrap()
    else {
        panic!("not an addition");
    };
    assert_eq!(second.outcome, AddOutcome::AlreadyExists);
    assert_eq!(scanner::scan_library(&library).unwrap().books.len(), 1);
}
