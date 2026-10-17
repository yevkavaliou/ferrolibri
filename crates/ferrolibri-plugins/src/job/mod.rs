//! Downloading a book a plugin found and putting it into the Calibre library (D7, D8, D14).
//!
//! [`DownloadAndAdd`] is a job for the core's `JobExecutor` (`JobRequest::External`): the core
//! schedules it, reserves the library for writing, relays its progress and cancellation, and
//! knows nothing about plugins. Steps, all with the job's cancellation token:
//!
//! 1. `resolveDownload(offer)` in the plugin; the answer is checked against the plugin's
//!    allow-list.
//! 2. A streamed download into a private workspace, through the app's own HTTP session (every
//!    redirect hop is checked against the allow-list, the size is limited). The file name is
//!    made by the app.
//! 3. The bytes are checked ([`sniff`]): a catalog's error page in place of a book is refused.
//! 4. `calibredb add` with the metadata of the result. A duplicate is an outcome, not an error.
//! 5. The workspace is removed whatever happened. If the add failed and an export folder is set,
//!    the file is kept there first and its path goes into the error.

mod name;
mod sniff;
mod zipdir;

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use ferrolibri_core::{
    Error, ErrorKind,
    calibre::{self, BookAdder, NewBookMetadata},
    converter::{self, CancellationToken, Workspace},
    jobs::{AddedBook, Context, ExternalJob, JobOutput, JobRequest, Stage},
};
use url::Url;

use crate::{
    AllowList, DownloadSpec, HostError, HostErrorKind, Identity, LoadedPlugin, Offer,
    PluginFunction, PluginRuntime, SearchHit, limits, parse_download_spec,
};
use sniff::Verdict;

const OPERATION: &str = "download book";
/// What the file is read in, and written in, at a time.
const CHUNK: usize = 64 * 1024;

pub use name::{MAX_NAME_CHARS, file_name};

/// "Download this offer of this result and add it to this library" as a job for the executor.
///
/// Build it, then hand it over with [`DownloadAndAdd::into_request`] (or
/// `JobRequest::External(Arc::new(job))`). It is cheap to clone.
#[derive(Clone, Debug)]
pub struct DownloadAndAdd {
    runtime: PluginRuntime,
    plugin: LoadedPlugin,
    hit: SearchHit,
    offer: Offer,
    library: PathBuf,
    export_dir: Option<PathBuf>,
    /// `None` when Calibre is not installed: the executor then refuses the job (`admit`).
    adder: Option<Arc<dyn BookAdder>>,
    max_bytes: u64,
}

impl DownloadAndAdd {
    /// `runtime` runs `resolveDownload` and its HTTP client does the download, so that the
    /// per-host queues count both. `adder` is [`DownloadAndAdd::detect_calibre`] (cache it: the
    /// search runs a program).
    pub fn new(
        runtime: PluginRuntime,
        plugin: LoadedPlugin,
        hit: SearchHit,
        offer: Offer,
        library: PathBuf,
        adder: Option<Arc<dyn BookAdder>>,
    ) -> Self {
        Self {
            runtime,
            plugin,
            hit,
            offer,
            library,
            export_dir: None,
            adder,
            max_bytes: limits::MAX_DOWNLOAD_BYTES,
        }
    }

    /// Where a downloaded file is kept if `calibredb add` fails. It must exist and be outside
    /// every Calibre library; it is checked when the file is saved.
    pub fn with_export_dir(mut self, dir: PathBuf) -> Self {
        self.export_dir = Some(dir);
        self
    }

    /// Lowers the size limit of the download (`limits::MAX_DOWNLOAD_BYTES` is the ceiling).
    pub fn with_max_bytes(mut self, max: u64) -> Self {
        self.max_bytes = max.min(limits::MAX_DOWNLOAD_BYTES);
        self
    }

    /// `calibredb` as an adder, or `None` if Calibre is not installed (the download icon must
    /// then be unavailable, "Нужен Calibre"). Runs `calibredb --version`: not for the UI thread.
    pub fn detect_calibre() -> Option<Arc<dyn BookAdder>> {
        calibre::find()
            .ok()
            .flatten()
            .map(|cli| Arc::new(cli) as Arc<dyn BookAdder>)
    }

    pub fn into_request(self) -> JobRequest {
        JobRequest::External(Arc::new(self))
    }

    fn metadata(&self) -> NewBookMetadata {
        NewBookMetadata {
            title: Some(self.hit.title.clone()),
            authors: self.hit.authors.clone(),
            isbn: self.hit.isbn.clone(),
            tags: self.hit.tags.clone(),
            series: self.hit.series.clone(),
            series_index: self.hit.series_index,
            languages: self.hit.language.iter().cloned().collect(),
            identifiers: Vec::new(),
            cover: None,
        }
    }

    /// Step 1: the plugin turns the offer into an address.
    fn resolve(
        &self,
        allow: &AllowList,
        cancel: &CancellationToken,
    ) -> Result<DownloadSpec, Error> {
        let offer = serde_json::to_value(&self.offer).expect("an offer always serializes");
        let answer = self.runtime.call(
            &self.plugin,
            PluginFunction::ResolveDownload,
            &offer,
            cancel,
        )?;
        let allowed = |host: &str| host_allowed(allow, host);
        parse_download_spec(&answer.to_string(), &self.offer, &allowed)
            .map_err(|e| e.into_core("plugin resolveDownload", &self.plugin.manifest.id))
    }

    /// Step 2: the file, into `path`. Nothing but a 2xx answer is written; a partial file is
    /// the caller's workspace's business.
    fn download(
        &self,
        spec: &DownloadSpec,
        allow: &AllowList,
        path: &Path,
        ctx: &Context,
    ) -> Result<(), Error> {
        let session = self
            .runtime
            .http()
            .session(Identity::App, allow.clone(), ctx.cancellation().clone())
            .with_request_budget(1);
        let mut stream = session
            .open(&spec.url, &[], self.max_bytes)
            .map_err(from_host)?;
        let head = stream.head().clone();
        if !head.is_success() {
            return Err(invalid(format!(
                "the site answered with status {} for {}",
                head.status,
                shown(&head.url)
            )));
        }
        let total = head.content_length();
        let mut progress = ctx.download_progress();
        // The headers are in: the first figure tells the UI the size, if the server announced one.
        progress(0, total);
        let mut file = File::create(path).map_err(|e| Error::io(OPERATION, path, e))?;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let n = stream.read_chunk(&mut buffer).map_err(from_host)?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])
                .map_err(|e| Error::io(OPERATION, path, e))?;
            progress(stream.bytes_read(), total);
        }
        file.flush().map_err(|e| Error::io(OPERATION, path, e))?;
        let received = stream.bytes_read();
        // The final figure always goes out, even if the last chunk fell inside the throttle.
        progress(received, Some(received));
        Ok(())
    }

    /// Step 5 on failure: copies the file into the export folder, under a name that does not
    /// replace anything. `None` if there is no folder or it cannot be used.
    fn keep(&self, file: &Path) -> Option<PathBuf> {
        let dir = self.export_dir.as_deref()?;
        let dir = converter::validate_output_dir(&self.library, dir)
            .inspect_err(|e| log::warn!("the export folder cannot be used: {e}"))
            .ok()?;
        let name = file.file_name()?.to_string_lossy().into_owned();
        let (stem, extension) = match name.split_once('.') {
            Some((stem, extension)) => (stem.to_owned(), format!(".{extension}")),
            None => (name.clone(), String::new()),
        };
        for attempt in 0u32..1000 {
            let candidate = if attempt == 0 {
                dir.join(&name)
            } else {
                dir.join(format!("{stem} ({attempt}){extension}"))
            };
            match File::options()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(mut target) => {
                    let copied =
                        File::open(file).and_then(|mut source| io::copy(&mut source, &mut target));
                    if let Err(e) = copied {
                        log::warn!("could not keep {}: {e}", candidate.display());
                        drop(target);
                        let _ = fs::remove_file(&candidate);
                        return None;
                    }
                    return Some(candidate);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    log::warn!("could not keep {}: {e}", candidate.display());
                    return None;
                }
            }
        }
        None
    }
}

impl ExternalJob for DownloadAndAdd {
    fn title(&self) -> String {
        format!("Download “{}”", self.hit.title)
    }

    /// Two additions to the same library never run together.
    fn writes_to(&self) -> Vec<PathBuf> {
        vec![self.library.clone()]
    }

    fn admit(&self) -> Result<(), String> {
        if self.adder.is_none() {
            return Err(
                "Calibre is needed to add books to the library; install it and try again".into(),
            );
        }
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<JobOutput, Error> {
        let adder = self.adder.as_ref().ok_or_else(calibre_missing)?;
        ctx.check()?;
        ctx.stage(Stage::Downloading);
        let workspace = Workspace::new(&self.library)?;
        let allow = self.plugin.allow_list().map_err(|e| {
            crate::plugin_error(
                OPERATION,
                &self.plugin.manifest.id,
                format!("the plugin's network settings are invalid: {e}"),
            )
        })?;
        let spec = self.resolve(&allow, ctx.cancellation())?;
        ctx.check()?;

        let path =
            workspace
                .path()
                .join(file_name(&self.hit.title, self.offer.format, spec.archive));
        self.download(&spec, &allow, &path, ctx)?;
        ctx.check()?;

        match sniff::check(&path, self.offer.format, spec.archive, self.max_bytes)
            .map_err(|e| Error::io(OPERATION, &path, e))?
        {
            Verdict::Book => {}
            Verdict::NotBook(found) => {
                return Err(invalid(format!(
                    "the site did not return a book: it sent {found}"
                )));
            }
        }

        ctx.stage(Stage::Adding);
        ctx.check()?;
        match adder.add_book(&self.library, &path, &self.metadata(), ctx.cancellation()) {
            Ok(outcome) => Ok(JobOutput::Added(AddedBook {
                library: self.library.clone(),
                title: self.hit.title.clone(),
                outcome,
            })),
            Err(error) if matches!(error.kind, ErrorKind::Cancelled) => Err(error),
            Err(cause) => {
                let kept = self.keep(&path);
                Err(Error {
                    operation: OPERATION,
                    path: None,
                    kind: ErrorKind::BookNotAdded {
                        cause: Box::new(cause),
                        kept,
                    },
                })
            }
        }
    }
}

/// Whether the plugin's allow-list lets `host` through. The scheme is not known here (the
/// answer is checked as a host only); the download checks the whole address again.
fn host_allowed(allow: &AllowList, host: &str) -> bool {
    Url::parse(&format!("https://{host}/"))
        .ok()
        .is_some_and(|url| allow.check(&url).is_ok())
}

fn calibre_missing() -> Error {
    Error {
        operation: OPERATION,
        path: None,
        kind: ErrorKind::CalibreUnavailable {
            diagnostic: "calibredb not found; install Calibre to add books to a library".into(),
        },
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error {
        operation: OPERATION,
        path: None,
        kind: ErrorKind::InvalidData(message.into().into()),
    }
}

/// An address for a message: without the query, which may carry a token.
fn shown(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.to_string()
}

fn from_host(error: HostError) -> Error {
    let kind = match error.kind {
        HostErrorKind::Cancelled => ErrorKind::Cancelled,
        HostErrorKind::Timeout => ErrorKind::TimedOut,
        HostErrorKind::TooLarge | HostErrorKind::BlockedDomain | HostErrorKind::TooManyRequests => {
            ErrorKind::InvalidData(error.to_string().into())
        }
        HostErrorKind::Connect | HostErrorKind::Dns | HostErrorKind::Tls => {
            log::warn!("download failed: {error}");
            ErrorKind::Network
        }
    };
    Error {
        operation: OPERATION,
        path: None,
        kind,
    }
}

#[cfg(test)]
mod tests;
