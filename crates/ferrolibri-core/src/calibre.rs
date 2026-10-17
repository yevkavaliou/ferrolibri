//! Calibre command-line integration (`calibredb`). See docs/CALIBRE_CLI.md.
//!
//! The only way FerroLibri writes into a Calibre library: it never edits
//! `metadata.db` or existing book folders itself.
use crate::{
    Error, ErrorKind, config,
    converter::{self, CancellationToken, ProcessRunner, SystemRunner},
};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

const PROGRAM: &str = "calibredb";
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const LIBRARY_TIMEOUT: Duration = Duration::from_secs(120);
const ADD_TIMEOUT: Duration = Duration::from_secs(300);
/// Calibre on Windows rejects library paths of 89 or more characters.
pub const WINDOWS_LIBRARY_PATH_LIMIT: usize = 88;

/// A located `calibredb`. Execution uses exactly this absolute path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibreCli {
    path: PathBuf,
    version: Option<String>,
}

impl CalibreCli {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Best effort: `None` when the version probe failed or was not recognised.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

/// Metadata passed to `calibredb add`. It is best effort: items Calibre could not
/// represent safely (control characters, malformed identifier types, ...) are
/// dropped instead of failing the whole addition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewBookMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub isbn: Option<String>,
    pub tags: Vec<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub languages: Vec<String>,
    /// `(type, value)`, for example `("isbn", "9780000000000")`.
    pub identifiers: Vec<(String, String)>,
    pub cover: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddOutcome {
    Added {
        id: u64,
    },
    /// Calibre refused the book because it is already in the library. This is
    /// reported with exit status 0, so it is recognised from the English output.
    AlreadyExists,
}

/// Something that can put a book file into a Calibre library: [`CalibreCli`], or a stand-in in
/// tests of the code that downloads books.
pub trait BookAdder: Send + Sync + std::fmt::Debug {
    fn add_book(
        &self,
        library: &Path,
        file: &Path,
        metadata: &NewBookMetadata,
        cancel: &CancellationToken,
    ) -> Result<AddOutcome, Error>;
}

impl BookAdder for CalibreCli {
    fn add_book(
        &self,
        library: &Path,
        file: &Path,
        metadata: &NewBookMetadata,
        cancel: &CancellationToken,
    ) -> Result<AddOutcome, Error> {
        CalibreCli::add_book(self, library, file, metadata, cancel)
    }
}

pub(crate) fn is_calibre_program(program: &str) -> bool {
    matches!(program, "calibredb" | "ebook-convert")
}

/// Standard Calibre install locations that may be missing from `PATH`.
pub fn standard_dirs() -> Vec<PathBuf> {
    standard_dirs_from(|name| std::env::var_os(name))
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn standard_dirs_from(env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        for variable in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(root) = env(variable) {
                dirs.push(PathBuf::from(root).join("Calibre2"));
            }
        }
        if let Some(local) = env("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs").join("Calibre2"));
        }
    }
    #[cfg(target_os = "macos")]
    dirs.push(PathBuf::from("/Applications/calibre.app/Contents/MacOS"));
    #[cfg(all(unix, not(target_os = "macos")))]
    dirs.push(PathBuf::from("/opt/calibre"));
    let mut unique = Vec::new();
    for dir in dirs {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

/// Look for `calibredb` in `PATH`, then in the standard Calibre locations.
pub fn find() -> Result<Option<CalibreCli>, Error> {
    find_with(&mut SystemRunner)
}

/// Like [`find`], but a missing `calibredb` is an error the UI can show.
pub fn require() -> Result<CalibreCli, Error> {
    find()?.ok_or_else(|| Error {
        operation: "find calibredb",
        path: None,
        kind: ErrorKind::CalibreUnavailable {
            diagnostic: "calibredb not found in PATH or the standard Calibre locations; \
                         install Calibre to add books to a library"
                .into(),
        },
    })
}

fn find_with(runner: &mut impl ProcessRunner) -> Result<Option<CalibreCli>, Error> {
    let path = match converter::executable(PROGRAM) {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(None),
        Err(error) => return Err(Error::io("find calibredb", Path::new(PROGRAM), error)),
    };
    let mut command = Command::new(&path);
    command.arg("--version");
    let version = runner
        .output(
            &mut command,
            &CancellationToken::default(),
            Some(VERSION_TIMEOUT),
        )
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| parse_version(&String::from_utf8_lossy(&output.stdout)));
    Ok(Some(CalibreCli { path, version }))
}

/// `calibredb.exe (calibre 8.16.2)` -> `8.16.2`.
fn parse_version(text: &str) -> Option<String> {
    let line = text.lines().next()?.trim();
    let rest = line.split("(calibre").nth(1)?;
    let version = rest.trim().trim_end_matches(')').trim();
    (!version.is_empty()).then(|| version.to_owned())
}

impl CalibreCli {
    /// Create an empty Calibre library in an existing, empty directory.
    ///
    /// `calibredb` creates a library the first time it opens an empty folder
    /// (verified with Calibre 8.16.2); a listing is the cheapest command that does it.
    pub fn create_library(&self, dir: &Path, cancel: &CancellationToken) -> Result<(), Error> {
        self.create_library_with(&mut SystemRunner, dir, cancel)
    }

    /// Like [`create_library`](Self::create_library) for a folder that already holds
    /// other files (checked with Calibre 8.16.2: they are left untouched and the
    /// library files are created next to them). Callers must get the user's explicit
    /// confirmation first. A folder that already has `metadata.db` is refused.
    pub fn create_library_among_files(
        &self,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        self.create_library_inner(&mut SystemRunner, dir, cancel, true)
    }

    fn create_library_with(
        &self,
        runner: &mut impl ProcessRunner,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        self.create_library_inner(runner, dir, cancel, false)
    }

    fn create_library_inner(
        &self,
        runner: &mut impl ProcessRunner,
        dir: &Path,
        cancel: &CancellationToken,
        allow_existing_files: bool,
    ) -> Result<(), Error> {
        const OPERATION: &str = "create Calibre library";
        let dir = std::path::absolute(dir).map_err(|e| Error::io(OPERATION, dir, e))?;
        let metadata = std::fs::metadata(&dir).map_err(|e| Error::io(OPERATION, &dir, e))?;
        if !metadata.is_dir() {
            return Err(invalid_input("create Calibre library: not a folder", &dir));
        }
        if allow_existing_files {
            if config::is_calibre_library(&dir)? {
                return Err(invalid_input(
                    "create Calibre library: folder already holds a library",
                    &dir,
                ));
            }
        } else {
            let mut entries = std::fs::read_dir(&dir).map_err(|e| Error::io(OPERATION, &dir, e))?;
            if let Some(entry) = entries.next() {
                entry.map_err(|e| Error::io(OPERATION, &dir, e))?;
                return Err(invalid_input(
                    "create Calibre library: folder must be empty",
                    &dir,
                ));
            }
        }
        check_library_path(&dir)?;
        let mut command = self.command();
        command
            .arg("list")
            .arg("--with-library")
            .arg(&dir)
            .args(["--fields", "title", "--limit", "1"]);
        self.run(
            runner,
            &mut command,
            cancel,
            LIBRARY_TIMEOUT,
            OPERATION,
            &dir,
        )?;
        let database = dir.join("metadata.db");
        match std::fs::metadata(&database) {
            Ok(metadata) if metadata.is_file() => Ok(()),
            Ok(_) => Err(Error::invalid_data(
                OPERATION,
                &database,
                "metadata.db is not a file",
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Error::invalid_data(
                OPERATION,
                &dir,
                "calibredb finished without creating metadata.db",
            )),
            Err(e) => Err(Error::io(OPERATION, &database, e)),
        }
    }

    /// Add one file to an existing library with `calibredb add`.
    ///
    /// Duplicates are skipped by Calibre and reported as [`AddOutcome::AlreadyExists`].
    /// If `calibredb` succeeds but prints something unrecognised the result is an
    /// error: the caller may rescan the library to find out what happened.
    pub fn add_book(
        &self,
        library: &Path,
        file: &Path,
        metadata: &NewBookMetadata,
        cancel: &CancellationToken,
    ) -> Result<AddOutcome, Error> {
        self.add_book_with(&mut SystemRunner, library, file, metadata, cancel)
    }

    fn add_book_with(
        &self,
        runner: &mut impl ProcessRunner,
        library: &Path,
        file: &Path,
        metadata: &NewBookMetadata,
        cancel: &CancellationToken,
    ) -> Result<AddOutcome, Error> {
        const OPERATION: &str = "add book to Calibre library";
        if !config::is_calibre_library(library)? {
            return Err(Error::unavailable(OPERATION, Some(library)));
        }
        let library = std::path::absolute(library).map_err(|e| Error::io(OPERATION, library, e))?;
        check_library_path(&library)?;
        let file = regular_file(OPERATION, file)?;
        let cover = metadata
            .cover
            .as_deref()
            .map(|cover| regular_file(OPERATION, cover))
            .transpose()?;
        let mut command = self.command();
        command.arg("add").arg("--with-library").arg(&library);
        command.args(metadata_arguments(metadata, cover.as_deref()));
        command.arg("--").arg(&file);
        let output = self.run(
            runner,
            &mut command,
            cancel,
            ADD_TIMEOUT,
            OPERATION,
            &library,
        )?;
        parse_add_output(&output, &library)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.path);
        // Calibre localises its output; the outcome text is only parsed in English.
        command.env("CALIBRE_OVERRIDE_LANG", "en");
        command
    }

    fn run(
        &self,
        runner: &mut impl ProcessRunner,
        command: &mut Command,
        cancel: &CancellationToken,
        timeout: Duration,
        operation: &'static str,
        path: &Path,
    ) -> Result<Output, Error> {
        match runner.output(command, cancel, Some(timeout)) {
            Ok(output) if output.status.success() => Ok(output),
            Ok(output) => Err(Error {
                operation,
                path: Some(path.to_owned()),
                kind: ErrorKind::ExternalProcess {
                    program: PROGRAM,
                    status: output.status,
                    stderr: output.stderr,
                },
            }),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Err(Error {
                operation,
                path: Some(path.to_owned()),
                kind: ErrorKind::Cancelled,
            }),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => Err(Error {
                operation,
                path: Some(path.to_owned()),
                kind: ErrorKind::TimedOut,
            }),
            Err(error) => Err(Error::io(operation, &self.path, error)),
        }
    }
}

fn invalid_input(operation: &'static str, path: &Path) -> Error {
    Error {
        operation,
        path: Some(path.to_owned()),
        kind: ErrorKind::InvalidInput,
    }
}

fn regular_file(operation: &'static str, path: &Path) -> Result<PathBuf, Error> {
    let absolute = std::path::absolute(path).map_err(|e| Error::io(operation, path, e))?;
    let metadata = std::fs::metadata(&absolute).map_err(|e| Error::io(operation, &absolute, e))?;
    if metadata.is_file() {
        Ok(absolute)
    } else {
        Err(invalid_input(
            "add book to Calibre library: not a regular file",
            &absolute,
        ))
    }
}

/// Calibre on Windows counts the whole library path against MAX_PATH.
fn check_library_path(path: &Path) -> Result<(), Error> {
    let length = path.as_os_str().to_string_lossy().chars().count();
    if library_path_too_long(length, cfg!(windows)) {
        Err(invalid_input(
            "library path is too long for Calibre on Windows (limit 88 characters)",
            path,
        ))
    } else {
        Ok(())
    }
}

fn library_path_too_long(characters: usize, windows: bool) -> bool {
    windows && characters > WINDOWS_LIBRARY_PATH_LIMIT
}

/// Option values use the `--name=value` form so a value beginning with `-` is never
/// read as an option. Every value is passed as one argument, never through a shell.
fn metadata_arguments(metadata: &NewBookMetadata, cover: Option<&Path>) -> Vec<OsString> {
    let mut arguments = Vec::new();
    let mut option = |name: &str, value: OsString| {
        let mut argument = OsString::from(format!("--{name}="));
        argument.push(value);
        arguments.push(argument);
    };
    if let Some(title) = metadata
        .title
        .as_deref()
        .map(clean_text)
        .filter(|t| !t.is_empty())
    {
        option("title", title.into());
    }
    // Calibre separates authors with `&`, tags and languages with `,`.
    let authors = clean_list(&metadata.authors, '&', "and");
    if !authors.is_empty() {
        option("authors", authors.join(" & ").into());
    }
    if let Some(isbn) = metadata
        .isbn
        .as_deref()
        .map(clean_text)
        .filter(|t| !t.is_empty())
    {
        option("isbn", isbn.into());
    }
    let tags = clean_list(&metadata.tags, ',', " ");
    if !tags.is_empty() {
        option("tags", tags.join(",").into());
    }
    if let Some(series) = metadata
        .series
        .as_deref()
        .map(clean_text)
        .filter(|t| !t.is_empty())
    {
        option("series", series.into());
        if let Some(index) = metadata.series_index.filter(|index| index.is_finite()) {
            option("series-index", index.to_string().into());
        }
    }
    let languages: Vec<String> = clean_list(&metadata.languages, ',', " ")
        .into_iter()
        .filter(|language| {
            (2..=12).contains(&language.len())
                && language
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .collect();
    if !languages.is_empty() {
        option("languages", languages.join(",").into());
    }
    for (kind, value) in &metadata.identifiers {
        let value = clean_text(value);
        let valid_kind = !kind.is_empty()
            && kind
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if valid_kind && !value.is_empty() {
            option("identifier", format!("{kind}:{value}").into());
        }
    }
    if let Some(cover) = cover {
        option("cover", cover.as_os_str().to_owned());
    }
    arguments
}

fn clean_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn clean_list(items: &[String], separator: char, replacement: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| clean_text(&item.replace(separator, replacement)))
        .filter(|item| !item.is_empty())
        .collect()
}

fn parse_add_output(output: &Output, library: &Path) -> Result<AddOutcome, Error> {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(rest) = text.split("Added book ids:").nth(1) {
        let first = rest.lines().next().unwrap_or("");
        if let Some(id) = first
            .split(',')
            .next()
            .and_then(|id| id.trim().parse().ok())
        {
            return Ok(AddOutcome::Added { id });
        }
    }
    if text.contains("were not added as they already exist") {
        return Ok(AddOutcome::AlreadyExists);
    }
    Err(Error::invalid_data(
        "add book to Calibre library",
        library,
        format!(
            "calibredb finished successfully but its output was not recognised: {}",
            text.trim()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;
    use std::collections::VecDeque;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn output(success: bool, stdout: &str, stderr: &str) -> io::Result<Output> {
        #[cfg(unix)]
        let status = ExitStatus::from_raw(if success { 0 } else { 1 << 8 });
        #[cfg(windows)]
        let status = ExitStatus::from_raw(if success { 0 } else { 1 });
        Ok(Output {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    type Hook = Box<dyn FnMut(&Command)>;
    type Call = (Vec<OsString>, Vec<(OsString, Option<OsString>)>);
    struct FakeRunner {
        results: VecDeque<io::Result<Output>>,
        hook: Option<Hook>,
        calls: Vec<Call>,
    }
    impl FakeRunner {
        fn new(results: Vec<io::Result<Output>>) -> Self {
            Self {
                results: results.into(),
                hook: None,
                calls: Vec::new(),
            }
        }
    }
    impl ProcessRunner for FakeRunner {
        fn output(
            &mut self,
            command: &mut Command,
            _: &CancellationToken,
            _: Option<Duration>,
        ) -> io::Result<Output> {
            self.calls.push((
                command.get_args().map(OsString::from).collect(),
                command
                    .get_envs()
                    .map(|(k, v)| (k.to_owned(), v.map(OsString::from)))
                    .collect(),
            ));
            if let Some(hook) = self.hook.as_mut() {
                hook(command);
            }
            self.results.pop_front().expect("unexpected calibredb call")
        }
    }

    fn cli() -> CalibreCli {
        CalibreCli {
            path: PathBuf::from("calibredb"),
            version: Some("8.16.2".into()),
        }
    }
    fn strings(arguments: &[OsString]) -> Vec<String> {
        arguments
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
    fn plain_file(fixture: &Fixture, name: &str) -> PathBuf {
        let path = fixture.root.join(name);
        std::fs::write(&path, b"book").unwrap();
        path
    }

    #[test]
    fn version_is_parsed_from_calibredb_output() {
        assert_eq!(
            parse_version("calibredb.exe (calibre 8.16.2)\n").as_deref(),
            Some("8.16.2")
        );
        assert_eq!(
            parse_version("calibredb (calibre 9.0.0)").as_deref(),
            Some("9.0.0")
        );
        assert_eq!(parse_version("something else"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn standard_dirs_name_calibre_install_locations_without_duplicates() {
        let dirs = standard_dirs_from(|name| match name {
            "ProgramFiles" | "ProgramW6432" => Some(OsString::from(r"C:\Program Files")),
            "LOCALAPPDATA" => Some(OsString::from(r"C:\Users\u\AppData\Local")),
            _ => None,
        });
        #[cfg(windows)]
        {
            assert_eq!(dirs.len(), 2, "{dirs:?}");
            assert!(dirs.iter().all(|d| d.ends_with("Calibre2")));
        }
        #[cfg(not(windows))]
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        assert!(is_calibre_program("calibredb") && is_calibre_program("ebook-convert"));
        assert!(!is_calibre_program("kepubify"));
    }

    #[test]
    fn long_library_paths_are_rejected_only_where_calibre_requires_it() {
        assert!(!library_path_too_long(88, true));
        assert!(library_path_too_long(89, true));
        assert!(!library_path_too_long(500, false));
    }

    #[test]
    fn version_probe_failure_does_not_hide_an_installed_calibredb() {
        let mut runner = FakeRunner::new(vec![output(false, "", "broken")]);
        let found = find_with(&mut runner).unwrap();
        // Only meaningful when a real calibredb is installed on this machine.
        if let Some(found) = found {
            assert_eq!(found.version(), None);
        }
    }

    #[test]
    fn create_library_runs_a_listing_and_verifies_the_database() {
        let fixture = Fixture::new();
        let empty = fixture.root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        let target = empty.clone();
        let mut runner = FakeRunner::new(vec![output(true, "id title\n", "")]);
        runner.hook = Some(Box::new(move |_| {
            std::fs::write(target.join("metadata.db"), b"db").unwrap();
        }));
        cli()
            .create_library_with(&mut runner, &empty, &CancellationToken::default())
            .unwrap();
        let (arguments, envs) = &runner.calls[0];
        let arguments = strings(arguments);
        assert_eq!(arguments[0], "list");
        assert_eq!(arguments[1], "--with-library");
        assert!(Path::new(&arguments[2]).ends_with("empty"));
        assert!(envs.contains(&("CALIBRE_OVERRIDE_LANG".into(), Some("en".into()))));
    }

    #[test]
    fn create_library_refuses_non_empty_missing_and_file_targets_without_running() {
        let fixture = Fixture::new();
        let mut runner = FakeRunner::new(vec![]);
        let cancel = CancellationToken::default();
        let error = cli()
            .create_library_with(&mut runner, &fixture.library, &cancel)
            .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvalidInput), "{error}");
        let missing = fixture.root.join("missing");
        assert!(matches!(
            cli()
                .create_library_with(&mut runner, &missing, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::Io(e) if e.kind() == io::ErrorKind::NotFound
        ));
        let file = plain_file(&fixture, "file.txt");
        let error = cli()
            .create_library_with(&mut runner, &file, &cancel)
            .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvalidInput), "{error}");
        assert!(runner.calls.is_empty());
    }

    #[test]
    fn create_library_among_files_keeps_files_and_refuses_existing_libraries() {
        let fixture = Fixture::new();
        let documents = fixture.root.join("documents");
        std::fs::create_dir(&documents).unwrap();
        std::fs::write(documents.join("notes.txt"), b"keep").unwrap();
        let target = documents.clone();
        let mut runner = FakeRunner::new(vec![output(true, "", "")]);
        runner.hook = Some(Box::new(move |_| {
            std::fs::write(target.join("metadata.db"), b"db").unwrap();
        }));
        let cancel = CancellationToken::default();
        cli()
            .create_library_inner(&mut runner, &documents, &cancel, true)
            .unwrap();
        assert_eq!(runner.calls.len(), 1);
        assert_eq!(std::fs::read(documents.join("notes.txt")).unwrap(), b"keep");
        // The folder is a library now: a second creation is refused without running.
        let error = cli()
            .create_library_inner(&mut runner, &documents, &cancel, true)
            .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvalidInput), "{error}");
        assert_eq!(runner.calls.len(), 1);
        let file = plain_file(&fixture, "file.txt");
        let error = cli()
            .create_library_inner(&mut runner, &file, &cancel, true)
            .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::InvalidInput), "{error}");
    }

    #[test]
    fn create_library_reports_failure_cancellation_timeout_and_missing_database() {
        let fixture = Fixture::new();
        let empty = fixture.root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        let cancel = CancellationToken::default();
        let mut runner = FakeRunner::new(vec![output(
            false,
            "",
            "Another calibre program is running",
        )]);
        let error = cli()
            .create_library_with(&mut runner, &empty, &cancel)
            .unwrap_err();
        assert!(
            matches!(&error.kind, ErrorKind::ExternalProcess { program: "calibredb", stderr, .. }
                if String::from_utf8_lossy(stderr).contains("Another calibre")),
            "{error}"
        );
        let mut runner = FakeRunner::new(vec![Err(io::ErrorKind::Interrupted.into())]);
        assert!(matches!(
            cli()
                .create_library_with(&mut runner, &empty, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        ));
        let mut runner = FakeRunner::new(vec![Err(io::ErrorKind::TimedOut.into())]);
        assert!(matches!(
            cli()
                .create_library_with(&mut runner, &empty, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::TimedOut
        ));
        let mut runner = FakeRunner::new(vec![output(true, "", "")]);
        assert!(matches!(
            cli()
                .create_library_with(&mut runner, &empty, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidData(_)
        ));
    }

    #[test]
    fn add_book_builds_safe_arguments_and_recognises_success() {
        let fixture = Fixture::new();
        let book = plain_file(&fixture, "-odd name.fb2.zip");
        let metadata = NewBookMetadata {
            title: Some("-Leading dash\nTitle".into()),
            authors: vec!["A & B".into(), "  ".into(), "C".into()],
            isbn: Some("978-0".into()),
            tags: vec!["sf, fantasy".into(), "classic".into()],
            series: Some("Cycle".into()),
            series_index: Some(2.0),
            languages: vec!["ru".into(), "not a language".into(), "en-GB".into()],
            identifiers: vec![
                ("isbn".into(), "978-0".into()),
                ("Bad Type".into(), "x".into()),
                ("mobi-asin".into(), "x".into()),
            ],
            cover: None,
        };
        let mut runner = FakeRunner::new(vec![output(true, "Added book ids: 7\n", "")]);
        let outcome = cli()
            .add_book_with(
                &mut runner,
                &fixture.library,
                &book,
                &metadata,
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(outcome, AddOutcome::Added { id: 7 });
        let arguments = strings(&runner.calls[0].0);
        assert_eq!(arguments[0], "add");
        assert_eq!(arguments[1], "--with-library");
        assert!(arguments.contains(&"--title=-Leading dash Title".to_owned()));
        assert!(arguments.contains(&"--authors=A and B & C".to_owned()));
        assert!(arguments.contains(&"--isbn=978-0".to_owned()));
        assert!(arguments.contains(&"--tags=sf  fantasy,classic".to_owned()));
        assert!(arguments.contains(&"--series=Cycle".to_owned()));
        assert!(arguments.contains(&"--series-index=2".to_owned()));
        assert!(arguments.contains(&"--languages=ru,en-GB".to_owned()));
        assert!(arguments.contains(&"--identifier=isbn:978-0".to_owned()));
        assert!(
            !arguments
                .iter()
                .any(|a| a.contains("Bad Type") || a.contains("mobi-asin"))
        );
        let separator = arguments.iter().position(|a| a == "--").unwrap();
        assert_eq!(separator, arguments.len() - 2);
        assert!(arguments[separator + 1].ends_with("-odd name.fb2.zip"));
    }

    #[test]
    fn add_book_distinguishes_duplicates_failures_and_unrecognised_output() {
        let fixture = Fixture::new();
        let book = plain_file(&fixture, "b.epub");
        let cancel = CancellationToken::default();
        let metadata = NewBookMetadata::default();
        let mut runner = FakeRunner::new(vec![output(
            true,
            "The following books were not added as they already exist in the database (see --duplicates option or --automerge option):\n  T\n",
            "",
        )]);
        assert_eq!(
            cli()
                .add_book_with(&mut runner, &fixture.library, &book, &metadata, &cancel)
                .unwrap(),
            AddOutcome::AlreadyExists
        );
        let mut runner = FakeRunner::new(vec![output(true, "Hallo\n", "")]);
        assert!(matches!(
            cli()
                .add_book_with(&mut runner, &fixture.library, &book, &metadata, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidData(_)
        ));
        let mut runner = FakeRunner::new(vec![output(false, "", "boom")]);
        assert!(matches!(
            cli()
                .add_book_with(&mut runner, &fixture.library, &book, &metadata, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::ExternalProcess {
                program: "calibredb",
                ..
            }
        ));
        let mut runner = FakeRunner::new(vec![Err(io::ErrorKind::Interrupted.into())]);
        assert!(matches!(
            cli()
                .add_book_with(&mut runner, &fixture.library, &book, &metadata, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        ));
    }

    #[test]
    fn add_book_validates_library_and_files_before_running() {
        let fixture = Fixture::new();
        let book = plain_file(&fixture, "b.epub");
        let cancel = CancellationToken::default();
        let metadata = NewBookMetadata::default();
        let mut runner = FakeRunner::new(vec![]);
        let not_a_library = fixture.root.join("devices");
        assert!(matches!(
            cli()
                .add_book_with(&mut runner, &not_a_library, &book, &metadata, &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::Unavailable
        ));
        assert!(matches!(
            cli()
                .add_book_with(
                    &mut runner,
                    &fixture.library,
                    &fixture.root.join("none.epub"),
                    &metadata,
                    &cancel
                )
                .unwrap_err()
                .kind,
            ErrorKind::Io(_)
        ));
        assert!(matches!(
            cli()
                .add_book_with(
                    &mut runner,
                    &fixture.library,
                    &fixture.book,
                    &metadata,
                    &cancel
                )
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        ));
        let with_missing_cover = NewBookMetadata {
            cover: Some(fixture.root.join("cover.jpg")),
            ..NewBookMetadata::default()
        };
        assert!(
            cli()
                .add_book_with(
                    &mut runner,
                    &fixture.library,
                    &book,
                    &with_missing_cover,
                    &cancel
                )
                .is_err()
        );
        assert!(runner.calls.is_empty());
    }

    #[test]
    fn add_book_passes_an_existing_cover() {
        let fixture = Fixture::new();
        let book = plain_file(&fixture, "b.epub");
        let cover = plain_file(&fixture, "cover.jpg");
        let metadata = NewBookMetadata {
            cover: Some(cover),
            ..NewBookMetadata::default()
        };
        let mut runner = FakeRunner::new(vec![output(true, "Added book ids: 1", "")]);
        cli()
            .add_book_with(
                &mut runner,
                &fixture.library,
                &book,
                &metadata,
                &CancellationToken::default(),
            )
            .unwrap();
        assert!(
            strings(&runner.calls[0].0)
                .iter()
                .any(|a| a.starts_with("--cover=") && a.ends_with("cover.jpg"))
        );
    }

    /// Needs an installed Calibre. Uses a short path: Calibre on Windows rejects
    /// library paths of 89 or more characters.
    #[test]
    #[ignore = "requires installed calibredb; creates, fills and removes a real library"]
    fn real_calibre_creates_a_library_and_adds_books() {
        let cli = require().expect("calibredb must be installed");
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = std::env::temp_dir().join(format!("fl-cal-{}", std::process::id()));
        let library = root.join("lib");
        std::fs::create_dir_all(&library).unwrap();
        let _cleanup = Cleanup(root.clone());
        let cancel = CancellationToken::default();
        cli.create_library(&library, &cancel).unwrap();
        assert!(config::is_calibre_library(&library).unwrap());
        let book = root.join("book.txt");
        std::fs::write(&book, "Hello\n").unwrap();
        let metadata = NewBookMetadata {
            title: Some("-A dashed title".into()),
            authors: vec!["Some Author".into()],
            tags: vec!["test".into()],
            languages: vec!["en".into()],
            ..NewBookMetadata::default()
        };
        let first = cli.add_book(&library, &book, &metadata, &cancel).unwrap();
        assert!(matches!(first, AddOutcome::Added { .. }), "{first:?}");
        let second = cli.add_book(&library, &book, &metadata, &cancel).unwrap();
        assert_eq!(second, AddOutcome::AlreadyExists);
        let scan = crate::scanner::scan_library(&library).unwrap();
        assert_eq!(scan.books.len(), 1);
        assert_eq!(scan.books[0].title, "-A dashed title");
    }
}
