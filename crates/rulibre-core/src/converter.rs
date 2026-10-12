pub use crate::conversion_model::{Backend, BackendKind, BackendStatus};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use std::{io, process::Output};

use crate::{
    book::{BookFile, Format},
    scanner,
};

const EBOOK_CONVERT_TARGETS: &[Format] = &[
    Format::Epub,
    Format::Pdf,
    Format::Mobi,
    Format::Azw3,
    Format::Docx,
    Format::Txt,
];

fn invalid_path(path: &Path) -> crate::Error {
    crate::Error {
        operation: "validate conversion destination",
        path: Some(path.to_owned()),
        kind: crate::ErrorKind::InvalidInput,
    }
}

/// Locate the Calibre root without assuming a fixed author/book directory depth.
pub fn library_root(book_path: &Path) -> Result<PathBuf, crate::Error> {
    let book = std::fs::canonicalize(book_path)
        .map_err(|e| crate::Error::io("resolve book path", book_path, e))?;
    for ancestor in book.ancestors() {
        match std::fs::metadata(ancestor.join("metadata.db")) {
            Ok(metadata) if metadata.is_file() => return Ok(ancestor.to_owned()),
            Ok(_) => return Err(invalid_path(ancestor)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(crate::Error::io("inspect library root", ancestor, e)),
        }
    }
    Err(crate::Error::unavailable(
        "find Calibre library root",
        Some(book_path),
    ))
}

/// Validate an existing export directory outside all Calibre libraries.
/// Canonicalization resolves links. A temporary empty file verifies write access
/// only after library protection checks; it is removed before returning.
pub fn validate_output_dir(
    library_root: &Path,
    output_dir: &Path,
) -> Result<PathBuf, crate::Error> {
    let library = std::fs::canonicalize(library_root)
        .map_err(|e| crate::Error::io("resolve library root", library_root, e))?;
    let output = std::fs::canonicalize(output_dir)
        .map_err(|e| crate::Error::io("resolve output directory", output_dir, e))?;
    if !library.is_dir() || !output.is_dir() || output.starts_with(&library) {
        return Err(invalid_path(output_dir));
    }
    // Also reject destinations in a different Calibre library.
    for ancestor in output.ancestors() {
        match std::fs::symlink_metadata(ancestor.join("metadata.db")) {
            Ok(_) => return Err(invalid_path(output_dir)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(crate::Error::io("inspect output directory", ancestor, e)),
        }
    }
    let permissions = std::fs::metadata(&output)
        .map_err(|e| crate::Error::io("inspect output permissions", output_dir, e))?
        .permissions();
    if permissions.readonly() {
        return Err(crate::Error::io(
            "write output directory",
            output_dir,
            io::Error::from(io::ErrorKind::PermissionDenied),
        ));
    }
    static NEXT_PROBE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let probe = output.join(format!(
            ".rulibre-write-check-{}-{}",
            std::process::id(),
            NEXT_PROBE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(file) => {
                drop(file);
                std::fs::remove_file(&probe)
                    .map_err(|e| crate::Error::io("remove output write check", &probe, e))?;
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(crate::Error::io("write output directory", output_dir, e)),
        }
    }
    Ok(output)
}

/// Owns one private conversion directory. Keep it alive while using its files.
/// Normal drop removes its contents; cleanup() exposes deletion errors to callers.
#[derive(Debug)]
pub struct Workspace {
    path: PathBuf,
}

impl Workspace {
    pub fn new(library_root: &Path) -> Result<Self, crate::Error> {
        Self::in_directory(library_root, &std::env::temp_dir())
    }

    pub fn in_directory(library_root: &Path, base: &Path) -> Result<Self, crate::Error> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = validate_output_dir(library_root, base)?;
        loop {
            let path = base.join(format!(
                "rulibre-conversion-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(crate::Error::io("create conversion workspace", &path, e)),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cleanup(&self) -> Result<(), crate::Error> {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(crate::Error::io(
                "clean conversion workspace",
                &self.path,
                e,
            )),
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("{error}");
        }
    }
}
/// Cooperative cancellation, shareable with a worker's caller.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<(), crate::Error> {
        if self.is_cancelled() {
            Err(crate::Error {
                operation: "convert book",
                path: None,
                kind: crate::ErrorKind::Cancelled,
            })
        } else {
            Ok(())
        }
    }
}

trait ProcessRunner {
    fn output(
        &mut self,
        command: &mut Command,
        cancel: &CancellationToken,
        timeout: Option<Duration>,
    ) -> io::Result<Output>;
}

struct SystemRunner;

impl ProcessRunner for SystemRunner {
    fn output(
        &mut self,
        command: &mut Command,
        cancel: &CancellationToken,
        timeout: Option<Duration>,
    ) -> io::Result<Output> {
        use std::io::Read;
        use std::process::Stdio;
        if cancel.is_cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // Spool streams instead of pipes: neither a full pipe nor an inherited pipe
        // in a grandchild can prevent cancellation/reaping of the converter.
        static NEXT_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::fs::canonicalize(
            command
                .get_current_dir()
                .map(Path::to_owned)
                .unwrap_or_else(std::env::temp_dir),
        )?;
        for ancestor in base.ancestors() {
            match std::fs::symlink_metadata(ancestor.join("metadata.db")) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "process logs cannot be inside a Calibre library",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let logs = loop {
            let path = base.join(format!(
                "rulibre-process-{}-{}",
                std::process::id(),
                NEXT_LOG.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => break Workspace { path },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let stdout_path = logs.path().join("stdout");
        let stderr_path = logs.path().join("stderr");
        let stdout = std::fs::File::create(&stdout_path)?;
        let stderr = std::fs::File::create(&stderr_path)?;
        command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
        let spawned = command.spawn();
        // Command retains its Stdio handles after spawn (also on spawn failure).
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = spawned?;
        let start = Instant::now();
        let status = loop {
            if cancel.is_cancelled() || timeout.is_some_and(|limit| start.elapsed() >= limit) {
                // Kill and wait before cleanup. A naturally exited child is also reaped.
                if let Err(error) = child.kill()
                    && child.try_wait()?.is_none()
                {
                    return Err(error);
                }
                child.wait()?;
                return Err(if cancel.is_cancelled() {
                    io::ErrorKind::Interrupted
                } else {
                    io::ErrorKind::TimedOut
                }
                .into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
        };
        let read = |path: &Path| -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(1024 * 1024)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        Ok(Output {
            status,
            stdout: read(&stdout_path)?,
            stderr: read(&stderr_path)?,
        })
    }
}

// Discovery resolves an executable once; execution uses that exact absolute path.
fn executable(program: &str) -> io::Result<Option<PathBuf>> {
    let Some(paths) = std::env::var_os("PATH") else {
        return Ok(None);
    };
    for dir in std::env::split_paths(&paths) {
        #[cfg(windows)]
        let candidate = dir.join(format!("{program}.exe"));
        #[cfg(not(windows))]
        let candidate = dir.join(program);
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => {
                return std::fs::canonicalize(candidate).map(Some);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

pub fn available_backends() -> Vec<Backend> {
    available_backends_cancellable(&CancellationToken::default())
}

pub fn available_backends_cancellable(cancel: &CancellationToken) -> Vec<Backend> {
    [BackendKind::Kepubify, BackendKind::EbookConvert]
        .into_iter()
        .map(|kind| match executable(kind.program()) {
            Ok(path) => probe_backend_with(kind, path, &mut SystemRunner, cancel),
            Err(error) => Backend {
                kind,
                path: None,
                version: None,
                status: BackendStatus::Broken,
                diagnostic: error.to_string(),
                kepub_output: false,
            },
        })
        .collect()
}

#[cfg(test)]
fn probe_backend(
    kind: BackendKind,
    path: Option<PathBuf>,
    runner: &mut impl ProcessRunner,
) -> Backend {
    probe_backend_with(kind, path, runner, &CancellationToken::default())
}

fn probe_backend_with(
    kind: BackendKind,
    path: Option<PathBuf>,
    runner: &mut impl ProcessRunner,
    cancel: &CancellationToken,
) -> Backend {
    let mut backend = Backend {
        kind,
        path,
        version: None,
        status: BackendStatus::Missing,
        diagnostic: "executable not found in PATH".into(),
        kepub_output: false,
    };
    let Some(path) = backend.path.as_ref() else {
        return backend;
    };
    let run = runner.output(
        Command::new(path).arg("--version"),
        cancel,
        Some(Duration::from_secs(10)),
    );
    match run {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let version = if version.is_empty() {
                String::from_utf8_lossy(&output.stderr).trim().to_owned()
            } else {
                version
            };
            if version.is_empty() {
                backend.status = BackendStatus::Broken;
                backend.diagnostic = "empty version response".into();
                return backend;
            }
            backend.version = Some(version);
            backend.status = BackendStatus::Ready;
            backend.diagnostic.clear();
        }
        Ok(output) => {
            backend.status = BackendStatus::Broken;
            backend.diagnostic = format!(
                "version probe failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            return backend;
        }
        Err(error) => {
            backend.status = BackendStatus::Broken;
            backend.diagnostic = format!("version probe: {error}");
            return backend;
        }
    }
    if kind == BackendKind::Kepubify {
        backend.kepub_output = true;
    } else {
        // Help initializes installed input/output plugins without converting files.
        match runner.output(
            Command::new(path).args(["rulibre-probe.epub", "rulibre-probe.kepub", "--help"]),
            cancel,
            Some(Duration::from_secs(10)),
        ) {
            Ok(output) if output.status.success() && !output.stdout.is_empty() => {
                backend.kepub_output = true
            }
            Ok(output) => {
                backend.diagnostic = format!(
                    "KEPUB unavailable ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )
            }
            Err(error) => backend.diagnostic = format!("KEPUB probe: {error}"),
        }
    }
    backend
}

#[derive(Clone, Debug)]
pub struct ConversionPlan {
    pub source: BookFile,
    pub target: Format,
    pub backend: Backend,
}

fn source_rank(format: Format) -> usize {
    // Prefer reflowable originals; PDF last because layout reconstruction is lossy.
    match format {
        Format::Epub => 0,
        Format::Azw3 => 1,
        Format::Docx => 2,
        Format::Mobi => 3,
        Format::Txt => 4,
        Format::Pdf => 5,
        Format::Kepub => 6,
    }
}

pub fn find_source_file(files: &[BookFile]) -> Option<PathBuf> {
    files
        .iter()
        .filter(|file| file.format.ebook_convert_input())
        .min_by_key(|file| (source_rank(file.format), &file.path))
        .map(|file| file.path.clone())
}

/// Deterministic single-step routes. KEPUB input is deliberately excluded:
/// .kepub.epub would otherwise be silently handled as an ordinary EPUB.
pub fn plan_conversion(
    files: &[BookFile],
    target: Format,
    backends: &[Backend],
) -> Result<ConversionPlan, crate::Error> {
    for kind in [BackendKind::Kepubify, BackendKind::EbookConvert] {
        let Some(backend) = backends.iter().find(|backend| {
            backend.kind == kind && backend.status == BackendStatus::Ready && backend.path.is_some()
        }) else {
            continue;
        };
        if kind == BackendKind::Kepubify && target != Format::Kepub {
            continue;
        }
        if target == Format::Kepub && !backend.kepub_output {
            continue;
        }
        let source = files
            .iter()
            .filter(|file| match kind {
                BackendKind::Kepubify => file.format.kepubify_input(),
                BackendKind::EbookConvert => file.format.ebook_convert_input(),
            })
            .min_by_key(|file| (source_rank(file.format), &file.path));
        if let Some(source) = source {
            return Ok(ConversionPlan {
                source: source.clone(),
                target,
                backend: backend.clone(),
            });
        }
    }
    let diagnostics = backends
        .iter()
        .map(|backend| {
            format!(
                "{}: {:?} {}",
                backend.kind.program(),
                backend.status,
                backend.diagnostic
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(crate::Error {
        operation: "select conversion route",
        path: None,
        kind: crate::ErrorKind::ConversionUnavailable {
            diagnostic: format!("no supported route to {}; {diagnostics}", target.label()),
        },
    })
}

pub fn target_formats(files: &[BookFile], backends: &[Backend]) -> Vec<(Format, String)> {
    EBOOK_CONVERT_TARGETS
        .iter()
        .copied()
        .chain([Format::Kepub])
        .filter(|target| !files.iter().any(|file| file.format == *target))
        .filter_map(|target| {
            plan_conversion(files, target, backends)
                .ok()
                .map(|plan| (target, plan.backend.kind.program().into()))
        })
        .collect()
}

#[derive(Debug)]
pub struct ConversionResult {
    pub output_path: PathBuf,
    pub size: u64,
    pub source: PathBuf,
    pub target: Format,
    pub backend: Backend,
}

pub fn convert(
    library: &Path,
    output_dir: &Path,
    source: &Path,
    target: Format,
) -> Result<ConversionResult, crate::Error> {
    convert_cancellable(
        library,
        output_dir,
        source,
        target,
        &CancellationToken::default(),
    )
}

pub fn convert_cancellable(
    library: &Path,
    output_dir: &Path,
    source: &Path,
    target: Format,
    cancel: &CancellationToken,
) -> Result<ConversionResult, crate::Error> {
    cancel.check()?;
    // Validate before probing tools; malformed/library destinations never run them.
    validate_source_and_destination(library, output_dir, source, target)?;
    let format = scanner::extract_path_format(source).ok_or_else(|| invalid_path(source))?;
    let backends = available_backends_cancellable(cancel);
    cancel.check()?;
    let plan = plan_conversion(
        &[BookFile {
            format,
            path: source.to_owned(),
        }],
        target,
        &backends,
    )?;
    execute_plan(library, output_dir, &plan, cancel)
}

pub fn execute_plan(
    library: &Path,
    output_dir: &Path,
    plan: &ConversionPlan,
    cancel: &CancellationToken,
) -> Result<ConversionResult, crate::Error> {
    execute_with(library, output_dir, plan, cancel, &mut SystemRunner)
}

fn validate_source_and_destination(
    library: &Path,
    output_dir: &Path,
    source: &Path,
    target: Format,
) -> Result<(PathBuf, PathBuf), crate::Error> {
    let format = scanner::extract_path_format(source);
    if !format.is_some_and(Format::ebook_convert_input) {
        return Err(invalid_path(source));
    }
    let output_dir = validate_output_dir(library, output_dir)?;
    let resolved = std::fs::canonicalize(source)
        .map_err(|error| crate::Error::io("resolve conversion source", source, error))?;
    if !resolved.is_file() {
        return Err(invalid_path(source));
    }
    let name = source.file_name().ok_or_else(|| invalid_path(source))?;
    let extension = if target == Format::Kepub {
        "kepub.epub".into()
    } else {
        target.label().to_ascii_lowercase()
    };
    let destination = output_dir.join(name).with_extension(extension);
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(crate::Error {
                operation: "validate conversion destination",
                path: Some(destination),
                kind: crate::ErrorKind::Conflict,
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(crate::Error::io(
                "inspect conversion output",
                &destination,
                error,
            ));
        }
    }
    if destination == resolved {
        return Err(invalid_path(&destination));
    }
    Ok((resolved, destination))
}

fn execute_with(
    library: &Path,
    output_dir: &Path,
    plan: &ConversionPlan,
    cancel: &CancellationToken,
    runner: &mut impl ProcessRunner,
) -> Result<ConversionResult, crate::Error> {
    cancel.check()?;
    let (source, destination) =
        validate_source_and_destination(library, output_dir, &plan.source.path, plan.target)?;
    if scanner::extract_path_format(&plan.source.path) != Some(plan.source.format) {
        return Err(invalid_path(&plan.source.path));
    }
    // Validate even caller-created plans against the same route policy.
    plan_conversion(
        std::slice::from_ref(&plan.source),
        plan.target,
        std::slice::from_ref(&plan.backend),
    )?;
    let staging = Workspace::in_directory(
        library,
        destination
            .parent()
            .ok_or_else(|| invalid_path(&destination))?,
    )?;
    let result = (|| {
        // Calibre selects its output plugin by the LAST extension. Never pass
        // .kepub.epub to it: that requests an ordinary EPUB, not KEPUB.
        let extension = if plan.target == Format::Kepub {
            "kepub".into()
        } else {
            plan.target.label().to_ascii_lowercase()
        };
        let temporary = staging.path().join("result").with_extension(extension);
        let program = plan.backend.kind.program();
        let mut command = Command::new(
            plan.backend
                .path
                .as_ref()
                .ok_or_else(|| invalid_path(&source))?,
        );
        command.current_dir(staging.path());
        match plan.backend.kind {
            BackendKind::Kepubify => {
                command.arg("-o").arg(&temporary).arg(&source);
            }
            BackendKind::EbookConvert => {
                command.arg(&source).arg(&temporary);
            }
        }
        let output = runner.output(&mut command, cancel, None).map_err(|error| {
            let kind = match error.kind() {
                io::ErrorKind::Interrupted => crate::ErrorKind::Cancelled,
                io::ErrorKind::TimedOut => crate::ErrorKind::TimedOut,
                _ => crate::ErrorKind::Io(error),
            };
            crate::Error {
                operation: "run converter",
                path: Some(source.clone()),
                kind,
            }
        })?;
        cancel.check()?;
        if !output.status.success() {
            return Err(crate::Error {
                operation: "convert book",
                path: Some(source.clone()),
                kind: crate::ErrorKind::ExternalProcess {
                    program,
                    status: output.status,
                    stderr: output.stderr,
                },
            });
        }
        let metadata = std::fs::symlink_metadata(&temporary)
            .map_err(|error| crate::Error::io("verify conversion output", &temporary, error))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(crate::Error::invalid_data(
                "verify conversion output",
                &temporary,
                "converter did not produce a nonempty regular file",
            ));
        }
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| crate::Error::io("sync conversion output", &temporary, error))?;
        cancel.check()?;
        // Atomic publication with no overwrite, including a racing creator.
        // A hard link is same-filesystem because staging is under output_dir.
        std::fs::hard_link(&temporary, &destination).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                crate::Error {
                    operation: "publish conversion output",
                    path: Some(destination.clone()),
                    kind: crate::ErrorKind::Conflict,
                }
            } else {
                crate::Error::io("publish conversion output", &destination, error)
            }
        })?;
        Ok(ConversionResult {
            output_path: destination,
            size: metadata.len(),
            source,
            target: plan.target,
            backend: plan.backend.clone(),
        })
    })();
    // Cleanup errors are observable, including when the converter failed.
    staging.cleanup()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, snapshot};
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    use std::{collections::VecDeque, ffi::OsString, process::ExitStatus};

    #[test]
    fn export_validation_rejects_library_missing_and_readonly_without_library_writes() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.library);
        let export = fixture.root.join("exports");
        std::fs::create_dir(&export).unwrap();
        assert_eq!(
            validate_output_dir(&fixture.library, &export).unwrap(),
            std::fs::canonicalize(&export).unwrap()
        );
        assert_eq!(std::fs::read_dir(&export).unwrap().count(), 0);
        assert!(validate_output_dir(&fixture.library, &fixture.library).is_err());
        assert!(validate_output_dir(&fixture.library, &fixture.book).is_err());
        assert!(validate_output_dir(&fixture.library, &fixture.root.join("missing")).is_err());
        assert!(
            validate_output_dir(&fixture.library, &fixture.library.join("metadata.db")).is_err()
        );
        let original = std::fs::metadata(&export).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&export, readonly).unwrap();
        let result = validate_output_dir(&fixture.library, &export);
        std::fs::set_permissions(&export, original).unwrap();
        assert!(
            matches!(result.unwrap_err().kind, crate::ErrorKind::Io(error) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        let other = fixture.root.join("other-library");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("metadata.db"), b"marker").unwrap();
        assert!(validate_output_dir(&fixture.library, &other).is_err());
        assert_eq!(snapshot(&fixture.library), before);
        assert_eq!(std::fs::read_dir(&export).unwrap().count(), 0);
    }

    fn output(success: bool, stdout: &[u8], stderr: &[u8]) -> io::Result<Output> {
        Ok(Output {
            status: ExitStatus::from_raw(if success { 0 } else { 257 }),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        })
    }
    fn backend(kind: BackendKind) -> Backend {
        Backend {
            kind,
            path: Some(kind.program().into()),
            version: Some("fixture 8.16.2".into()),
            status: BackendStatus::Ready,
            diagnostic: String::new(),
            kepub_output: true,
        }
    }
    struct ProbeRunner(VecDeque<io::Result<Output>>);
    impl ProcessRunner for ProbeRunner {
        fn output(
            &mut self,
            _: &mut Command,
            _: &CancellationToken,
            _: Option<Duration>,
        ) -> io::Result<Output> {
            self.0.pop_front().expect("unexpected probe")
        }
    }
    #[derive(Clone, Copy)]
    enum Behavior {
        Success,
        Failure,
        Missing,
        Empty,
        Directory,
        SpawnFailure,
        Cancel,
        Conflict,
    }
    struct FakeRunner {
        behavior: Behavior,
        calls: Vec<(OsString, Vec<OsString>)>,
        conflict: Option<PathBuf>,
    }
    impl FakeRunner {
        fn new(behavior: Behavior) -> Self {
            Self {
                behavior,
                calls: Vec::new(),
                conflict: None,
            }
        }
    }
    impl ProcessRunner for FakeRunner {
        fn output(
            &mut self,
            command: &mut Command,
            cancel: &CancellationToken,
            _: Option<Duration>,
        ) -> io::Result<Output> {
            let args: Vec<OsString> = command.get_args().map(OsString::from).collect();
            self.calls
                .push((command.get_program().to_owned(), args.clone()));
            if matches!(self.behavior, Behavior::SpawnFailure) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let destination = Path::new(&args[1]);
            match self.behavior {
                Behavior::Missing => {}
                Behavior::Empty => std::fs::write(destination, b"")?,
                Behavior::Directory => std::fs::create_dir(destination)?,
                _ => std::fs::write(destination, b"converted fixture")?,
            }
            if matches!(self.behavior, Behavior::Cancel) {
                cancel.cancel();
                return Err(io::ErrorKind::Interrupted.into());
            }
            if matches!(self.behavior, Behavior::Conflict) {
                std::fs::write(self.conflict.as_ref().unwrap(), b"concurrent file")?;
            }
            output(
                !matches!(self.behavior, Behavior::Failure),
                b"",
                b"synthetic failure",
            )
        }
    }
    fn plan(fixture: &Fixture, kind: BackendKind, target: Format) -> ConversionPlan {
        ConversionPlan {
            source: BookFile {
                format: Format::Epub,
                path: fixture.book.join("A small book.epub"),
            },
            target,
            backend: backend(kind),
        }
    }
    #[test]
    fn discovery_distinguishes_missing_broken_and_ready_with_version_and_path() {
        let mut runner = ProbeRunner(VecDeque::new());
        assert_eq!(
            probe_backend(BackendKind::Kepubify, None, &mut runner).status,
            BackendStatus::Missing
        );
        for response in [
            Err(io::ErrorKind::PermissionDenied.into()),
            output(false, b"", b"bad installation"),
            output(true, b"", b""),
        ] {
            let mut runner = ProbeRunner(VecDeque::from([response]));
            let found = probe_backend(BackendKind::Kepubify, Some("tool.exe".into()), &mut runner);
            assert_eq!(found.status, BackendStatus::Broken);
            assert!(!found.diagnostic.is_empty());
        }
        let mut runner = ProbeRunner(VecDeque::from([output(true, b"kepubify v4.0.4", b"")]));
        let found = probe_backend(BackendKind::Kepubify, Some("tool.exe".into()), &mut runner);
        assert_eq!(found.status, BackendStatus::Ready);
        assert_eq!(found.version.as_deref(), Some("kepubify v4.0.4"));
        assert_eq!(found.path, Some("tool.exe".into()));
    }
    #[test]
    fn calibre_kepub_requires_successful_installed_plugin_probe() {
        for supported in [true, false] {
            let mut runner = ProbeRunner(VecDeque::from([
                output(true, b"calibre 8.16.2", b""),
                output(supported, b"plugin help", b"unsupported format"),
            ]));
            let found = probe_backend(
                BackendKind::EbookConvert,
                Some("tool.exe".into()),
                &mut runner,
            );
            assert_eq!(found.status, BackendStatus::Ready);
            assert_eq!(found.kepub_output, supported);
            if !supported {
                assert!(found.diagnostic.contains("unsupported format"));
            }
        }
    }
    #[test]
    fn routes_are_deterministic_target_aware_and_exclude_kepub_input() {
        let fixture = Fixture::new();
        let mut files = scanner::scan_files(&fixture.book).unwrap();
        files.reverse();
        let tools = [
            backend(BackendKind::Kepubify),
            backend(BackendKind::EbookConvert),
        ];
        let selected = plan_conversion(&files, Format::Kepub, &tools).unwrap();
        assert_eq!(selected.source.format, Format::Epub);
        assert_eq!(selected.backend.kind, BackendKind::Kepubify);
        files.retain(|f| f.format != Format::Epub);
        let selected = plan_conversion(&files, Format::Kepub, &tools).unwrap();
        assert_eq!(selected.source.format, Format::Mobi);
        assert_eq!(selected.backend.kind, BackendKind::EbookConvert);
        let mut old = backend(BackendKind::EbookConvert);
        old.kepub_output = false;
        assert!(plan_conversion(&files, Format::Kepub, &[old]).is_err());
        files.retain(|f| f.format == Format::Kepub);
        assert!(plan_conversion(&files, Format::Epub, &tools).is_err());
    }
    #[test]
    fn missing_backend_returns_diagnostics_without_launching() {
        let file = BookFile {
            format: Format::Txt,
            path: "book.txt".into(),
        };
        let mut missing = backend(BackendKind::EbookConvert);
        missing.status = BackendStatus::Missing;
        missing.diagnostic = "not installed".into();
        let error = plan_conversion(&[file], Format::Epub, &[missing]).unwrap_err();
        assert!(error.to_string().contains("not installed"));
    }
    #[test]
    fn failures_never_publish_and_clean_partial_results_preserving_library() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.library);
        let initial = snapshot(&fixture.root);
        let plan = plan(&fixture, BackendKind::EbookConvert, Format::Docx);
        for behavior in [
            Behavior::Failure,
            Behavior::Missing,
            Behavior::Empty,
            Behavior::Directory,
            Behavior::SpawnFailure,
            Behavior::Cancel,
        ] {
            let mut runner = FakeRunner::new(behavior);
            let error = execute_with(
                &fixture.library,
                &fixture.root,
                &plan,
                &CancellationToken::default(),
                &mut runner,
            )
            .unwrap_err();
            match behavior {
                Behavior::Failure => assert!(
                    matches!(error.kind, crate::ErrorKind::ExternalProcess { ref stderr, .. } if stderr == b"synthetic failure")
                ),
                Behavior::SpawnFailure => assert!(
                    matches!(error.kind, crate::ErrorKind::Io(ref e) if e.kind() == io::ErrorKind::PermissionDenied)
                ),
                Behavior::Cancel => assert!(matches!(error.kind, crate::ErrorKind::Cancelled)),
                _ => {}
            }
            assert_eq!(snapshot(&fixture.root), initial);
            assert_eq!(snapshot(&fixture.library), before);
            assert!(!fixture.root.join("A small book.docx").exists());
        }
    }
    #[test]
    fn success_returns_verified_result_and_separate_native_arguments() {
        for kind in [BackendKind::Kepubify, BackendKind::EbookConvert] {
            let fixture = Fixture::new();
            let before = snapshot(&fixture.library);
            let plan = plan(&fixture, kind, Format::Kepub);
            let mut runner = FakeRunner::new(Behavior::Success);
            let result = execute_with(
                &fixture.library,
                &fixture.root,
                &plan,
                &CancellationToken::default(),
                &mut runner,
            )
            .unwrap();
            assert_eq!(
                result.output_path.file_name().unwrap(),
                "A small book.kepub.epub"
            );
            assert_eq!(
                std::fs::read(&result.output_path).unwrap(),
                b"converted fixture"
            );
            assert_eq!(result.size, 17);
            assert_eq!(result.backend.kind, kind);
            assert_eq!(snapshot(&fixture.library), before);
            let (_, args) = &runner.calls[0];
            assert_eq!(Path::new(&args[1]).extension().unwrap(), "kepub");
            let source_arg = if kind == BackendKind::Kepubify {
                assert_eq!(args[0], "-o");
                &args[2]
            } else {
                &args[0]
            };
            assert_eq!(Path::new(source_arg), result.source);
            assert!(!Path::new(&args[1]).parent().unwrap().exists());
        }
    }
    #[test]
    fn existing_and_racing_destinations_are_never_overwritten() {
        let fixture = Fixture::new();
        let plan = plan(&fixture, BackendKind::EbookConvert, Format::Docx);
        let destination = fixture.root.join("A small book.docx");
        let mut runner = FakeRunner::new(Behavior::Conflict);
        runner.conflict = Some(destination.clone());
        let error = execute_with(
            &fixture.library,
            &fixture.root,
            &plan,
            &CancellationToken::default(),
            &mut runner,
        )
        .unwrap_err();
        assert_eq!(error.operation, "publish conversion output");
        assert!(matches!(error.kind, crate::ErrorKind::Conflict));
        assert_eq!(std::fs::read(&destination).unwrap(), b"concurrent file");
        let mut runner = FakeRunner::new(Behavior::Success);
        assert!(
            execute_with(
                &fixture.library,
                &fixture.root,
                &plan,
                &CancellationToken::default(),
                &mut runner
            )
            .is_err()
        );
        assert!(runner.calls.is_empty());
        assert_eq!(std::fs::read(destination).unwrap(), b"concurrent file");
    }
    #[test]
    fn cancellation_before_launch_does_not_create_outputs() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.root);
        let cancel = CancellationToken::default();
        cancel.cancel();
        let mut runner = FakeRunner::new(Behavior::Success);
        let error = execute_with(
            &fixture.library,
            &fixture.root,
            &plan(&fixture, BackendKind::EbookConvert, Format::Epub),
            &cancel,
            &mut runner,
        )
        .unwrap_err();
        assert!(matches!(error.kind, crate::ErrorKind::Cancelled));
        assert!(runner.calls.is_empty());
        assert_eq!(snapshot(&fixture.root), before);
    }
    #[test]
    fn invalid_sources_and_destinations_do_not_run_tools() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.library);
        let mut runner = FakeRunner::new(Behavior::Success);
        for name in [
            "metadata.opf",
            "cover.jpg",
            "unknown.xyz",
            "book.KEPUB.EPUB",
        ] {
            let mut plan = plan(&fixture, BackendKind::EbookConvert, Format::Epub);
            plan.source.path = fixture.book.join(name);
            assert!(
                execute_with(
                    &fixture.library,
                    &fixture.root,
                    &plan,
                    &CancellationToken::default(),
                    &mut runner
                )
                .is_err()
            );
        }
        let other = fixture.root.join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("metadata.db"), b"other").unwrap();
        for dir in [
            &fixture.library,
            &fixture.book,
            &fixture.book.join(".."),
            &other,
            &fixture.root.join("missing"),
        ] {
            assert!(
                execute_with(
                    &fixture.library,
                    dir,
                    &plan(&fixture, BackendKind::EbookConvert, Format::Epub),
                    &CancellationToken::default(),
                    &mut runner
                )
                .is_err()
            );
            assert!(Workspace::in_directory(&fixture.library, dir).is_err());
        }
        assert!(runner.calls.is_empty());
        assert_eq!(snapshot(&fixture.library), before);
    }
    #[test]
    fn directory_alias_into_library_is_rejected() {
        let fixture = Fixture::new();
        let alias = fixture.root.join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fixture.book, &alias).unwrap();
        #[cfg(windows)]
        {
            let status = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&alias)
                .arg(&fixture.book)
                .output()
                .unwrap();
            assert!(status.status.success());
        }
        assert!(Workspace::in_directory(&fixture.library, &alias).is_err());
        #[cfg(windows)]
        std::fs::remove_dir(alias).unwrap();
    }
    #[test]
    #[ignore = "helper subprocess; only invoked by system_runner test"]
    fn process_helper() {
        if let Some(marker) = std::env::var_os("RULIBRE_PROCESS_MARKER") {
            std::fs::write(marker, b"started").unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
    }
    #[test]
    fn system_runner_kills_and_reaps_on_cancellation_and_probe_timeout() {
        let fixture = Fixture::new();
        for timeout in [false, true] {
            let marker = fixture
                .root
                .join(if timeout { "timeout" } else { "cancel" });
            let cancel = CancellationToken::default();
            let token = cancel.clone();
            let wait_marker = marker.clone();
            let thread = if !timeout {
                Some(std::thread::spawn(move || {
                    let start = Instant::now();
                    while !wait_marker.exists() && start.elapsed() < Duration::from_secs(10) {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    token.cancel();
                }))
            } else {
                None
            };
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "converter::tests::process_helper",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ])
                .env("RULIBRE_PROCESS_MARKER", &marker);
            let started = Instant::now();
            let error = SystemRunner
                .output(
                    &mut command,
                    &cancel,
                    Some(if timeout {
                        Duration::from_millis(500)
                    } else {
                        Duration::from_secs(15)
                    }),
                )
                .unwrap_err();
            if let Some(thread) = thread {
                thread.join().unwrap();
            }
            assert!(marker.exists());
            assert_eq!(
                error.kind(),
                if timeout {
                    io::ErrorKind::TimedOut
                } else {
                    io::ErrorKind::Interrupted
                }
            );
            assert!(started.elapsed() < Duration::from_secs(10));
            std::fs::remove_file(marker).unwrap();
        }
    }
    #[test]
    #[ignore = "requires installed ebook-convert; real synthetic TXT -> EPUB -> KEPUB"]
    fn real_ebook_convert_keeps_library_read_only() {
        let fixture = Fixture::new();
        let before = snapshot(&fixture.library);
        let workspace = Workspace::in_directory(&fixture.library, &fixture.root).unwrap();
        let source = fixture.library.join("Zed/Second book (43)/Second book.txt");
        let epub = convert(&fixture.library, workspace.path(), &source, Format::Epub).unwrap();
        assert!(epub.size > 0);
        let tools = available_backends()
            .into_iter()
            .filter(|tool| tool.kind == BackendKind::EbookConvert)
            .collect::<Vec<_>>();
        let plan = plan_conversion(
            &[BookFile {
                format: Format::Epub,
                path: epub.output_path.clone(),
            }],
            Format::Kepub,
            &tools,
        )
        .unwrap();
        let kepub = execute_plan(
            &fixture.library,
            workspace.path(),
            &plan,
            &CancellationToken::default(),
        )
        .unwrap();
        assert!(kepub.size > 0);
        assert_eq!(kepub.backend.kind, BackendKind::EbookConvert);
        assert_eq!(snapshot(&fixture.library), before);
        workspace.cleanup().unwrap();
        assert!(!workspace.path().exists());
    }
}
