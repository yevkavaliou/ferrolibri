use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    book::{BookFile, Format},
    converter, scanner,
};

pub use crate::device_model::{DetectionEvidence, Device, DeviceEvent, DeviceId, DeviceKind};

#[derive(Default)]
pub struct DeviceState {
    pub connected: Option<Device>,
}

impl DeviceState {
    /// Process a device event, returning a status message.
    pub fn handle_event(&mut self, event: DeviceEvent) -> Result<String, String> {
        self.connected = event.selected_device().cloned();
        event.message()
    }

    /// Find or convert a compatible file, then copy it to the device.
    pub fn send_book(&self, book_path: &Path, author: &str) -> Result<PathBuf, crate::Error> {
        self.send_book_with_options(book_path, author, PreparationOptions::default())
    }

    pub fn send_book_with_options(
        &self,
        book_path: &Path,
        author: &str,
        options: PreparationOptions,
    ) -> Result<PathBuf, crate::Error> {
        self.send_book_cancellable(
            book_path,
            author,
            options,
            &converter::CancellationToken::default(),
        )
    }

    pub fn send_book_cancellable(
        &self,
        book_path: &Path,
        author: &str,
        options: PreparationOptions,
        cancel: &converter::CancellationToken,
    ) -> Result<PathBuf, crate::Error> {
        let dev = self
            .connected
            .as_ref()
            .ok_or_else(|| crate::Error::unavailable("select device", None))?;

        crate::transfer::validate_device(dev)?;
        let file = prepare_file_cancellable(book_path, dev, options, cancel)?;
        let library = converter::library_root(book_path)?;
        converter::validate_output_dir(&library, &dev.mount_point)?;
        crate::transfer::transfer_file(&file.path, dev, author, cancel, |_, _| {})
    }
}

impl Device {
    pub fn books_dir(&self, author: &str) -> PathBuf {
        match self.kind {
            DeviceKind::Kobo => self
                .mount_point
                .join(crate::transfer::normalize_component(author)),
            DeviceKind::Kindle => self.mount_point.join("documents"),
        }
    }

    pub fn supported_formats(&self) -> &[Format] {
        match self.kind {
            DeviceKind::Kobo => &[Format::Kepub, Format::Epub],
            DeviceKind::Kindle => &[Format::Azw3, Format::Mobi],
        }
    }
}

/// PreferKepub falls back to EPUB only when no KEPUB route is available.
/// A failure executing a selected route is returned to the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KoboPreparation {
    #[default]
    Compatible,
    PreferKepub,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreparationOptions {
    pub kobo: KoboPreparation,
}

/// A complete route selected before any conversion is started.
#[derive(Clone, Debug)]
pub enum PreparationPlan {
    Ready(PathBuf),
    Convert(converter::ConversionPlan),
    ViaEpub {
        epub: converter::ConversionPlan,
        kepub_backend: converter::Backend,
    },
}

/// The workspace owner must remain alive until the file has been used/copied.
#[derive(Debug)]
pub struct PreparedFile {
    pub path: PathBuf,
    _workspace: Option<converter::Workspace>,
}

impl PreparedFile {
    pub fn is_temporary(&self) -> bool {
        self._workspace.is_some()
    }
}

/// Pure planning with the same format/capability policy as normal conversion.
pub fn plan_preparation(
    files: &[BookFile],
    device: &Device,
    options: PreparationOptions,
    backends: &[converter::Backend],
) -> Result<PreparationPlan, crate::Error> {
    let prefer_kepub =
        device.kind == DeviceKind::Kobo && options.kobo == KoboPreparation::PreferKepub;
    if let Some(path) = find_file(
        files,
        if prefer_kepub {
            &[Format::Kepub]
        } else {
            device.supported_formats()
        },
    ) {
        return Ok(PreparationPlan::Ready(path));
    }
    if prefer_kepub {
        if let Ok(plan) = converter::plan_conversion(files, Format::Kepub, backends) {
            return Ok(PreparationPlan::Convert(plan));
        }
        // Calibre without a KEPUB plugin can still create EPUB for kepubify.
        if let Ok(epub) = converter::plan_conversion(files, Format::Epub, backends) {
            let intermediate = BookFile {
                format: Format::Epub,
                path: PathBuf::from("intermediate.epub"),
            };
            if let Ok(kepub) = converter::plan_conversion(&[intermediate], Format::Kepub, backends)
            {
                return Ok(PreparationPlan::ViaEpub {
                    epub,
                    kepub_backend: kepub.backend,
                });
            }
        }
        if let Some(path) = find_file(files, &[Format::Epub]) {
            return Ok(PreparationPlan::Ready(path));
        }
    }
    let target = match device.kind {
        DeviceKind::Kobo => Format::Epub,
        DeviceKind::Kindle => Format::Azw3,
    };
    converter::plan_conversion(files, target, backends)
        .map(PreparationPlan::Convert)
        .map_err(|mut error| {
            error.operation = "plan reader preparation";
            error
        })
}

pub fn prepare_file(
    book_path: &Path,
    device: &Device,
    options: PreparationOptions,
) -> Result<PreparedFile, crate::Error> {
    prepare_file_cancellable(
        book_path,
        device,
        options,
        &converter::CancellationToken::default(),
    )
}

pub fn prepare_file_cancellable(
    book_path: &Path,
    device: &Device,
    options: PreparationOptions,
    cancel: &converter::CancellationToken,
) -> Result<PreparedFile, crate::Error> {
    if cancel.is_cancelled() {
        return Err(crate::Error {
            operation: "prepare reader book",
            path: None,
            kind: crate::ErrorKind::Cancelled,
        });
    }
    let files = scanner::scan_files(book_path)?;
    // Ready books do not even probe external programs.
    let ready_formats =
        if device.kind == DeviceKind::Kobo && options.kobo == KoboPreparation::PreferKepub {
            &[Format::Kepub][..]
        } else {
            device.supported_formats()
        };
    let plan = match find_file(&files, ready_formats) {
        Some(path) => PreparationPlan::Ready(path),
        None => plan_preparation(
            &files,
            device,
            options,
            &converter::available_backends_cancellable(cancel),
        )?,
    };
    execute_preparation_with(book_path, plan, |library, output, plan| {
        converter::execute_plan(library, output, plan, cancel).map(|result| result.output_path)
    })
}

fn execute_preparation_with(
    book_path: &Path,
    plan: PreparationPlan,
    mut convert: impl FnMut(&Path, &Path, &converter::ConversionPlan) -> Result<PathBuf, crate::Error>,
) -> Result<PreparedFile, crate::Error> {
    if let PreparationPlan::Ready(path) = plan {
        return Ok(PreparedFile {
            path,
            _workspace: None,
        });
    }
    let library = converter::library_root(book_path)?;
    let workspace = converter::Workspace::new(&library)?;
    let result = (|| match plan {
        PreparationPlan::Convert(plan) => convert(&library, workspace.path(), &plan),
        PreparationPlan::ViaEpub {
            epub,
            kepub_backend,
        } => {
            let path = convert(&library, workspace.path(), &epub)?;
            let plan = converter::plan_conversion(
                &[BookFile {
                    format: Format::Epub,
                    path,
                }],
                Format::Kepub,
                &[kepub_backend],
            )?;
            convert(&library, workspace.path(), &plan)
        }
        PreparationPlan::Ready(_) => unreachable!(),
    })();
    match result {
        Ok(path) => Ok(PreparedFile {
            path,
            _workspace: Some(workspace),
        }),
        Err(error) => {
            workspace.cleanup()?;
            Err(error)
        }
    }
}

/// Select the exact format, preserving device priority and deterministic paths.
fn find_file(files: &[BookFile], formats: &[Format]) -> Option<PathBuf> {
    formats.iter().find_map(|format| {
        files
            .iter()
            .filter(|f| f.format == *format)
            .min_by_key(|f| &f.path)
            .map(|f| f.path.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(kind: converter::BackendKind, kepub_output: bool) -> converter::Backend {
        converter::Backend {
            kind,
            path: Some("synthetic-tool.exe".into()),
            version: Some("test".into()),
            status: converter::BackendStatus::Ready,
            diagnostic: String::new(),
            kepub_output,
        }
    }

    fn prefer_kepub() -> PreparationOptions {
        PreparationOptions {
            kobo: KoboPreparation::PreferKepub,
        }
    }

    fn prepare_file_with(
        book_path: &Path,
        device: &Device,
        mut convert: impl FnMut(&Path, &Path, &Path, Format) -> Result<PathBuf, crate::Error>,
    ) -> Result<PreparedFile, crate::Error> {
        let plan = plan_preparation(
            &scanner::scan_files(book_path)?,
            device,
            prefer_kepub(),
            &[backend(converter::BackendKind::Kepubify, true)],
        )?;
        execute_preparation_with(book_path, plan, |library, output, plan| {
            convert(library, output, &plan.source.path, plan.target)
        })
    }

    #[test]
    fn ready_formats_need_no_converter_and_preserve_library() {
        let fixture = crate::test_support::Fixture::new();
        let options = PreparationOptions::default();
        let kobo = crate::test_support::device(DeviceKind::Kobo, &fixture.kobo);
        let kindle = crate::test_support::device(DeviceKind::Kindle, &fixture.kindle);
        fs::rename(
            fixture.book.join("A small book.kepub.epub"),
            fixture.book.join("BOOK.KEPUB.EPUB"),
        )
        .unwrap();
        let before = crate::test_support::snapshot(&fixture.library);
        let prepared = prepare_file(&fixture.book, &kobo, prefer_kepub()).unwrap();
        assert_eq!(prepared.path, fixture.book.join("BOOK.KEPUB.EPUB"));
        assert!(!prepared.is_temporary());
        let prepared = prepare_file(&fixture.book, &kindle, options).unwrap();
        assert_eq!(prepared.path, fixture.book.join("A small book.mobi"));
        assert!(!prepared.is_temporary());
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);

        fs::remove_file(fixture.book.join("BOOK.KEPUB.EPUB")).unwrap();
        fs::rename(
            fixture.book.join("A small book.epub"),
            fixture.book.join("BOOK.EPUB"),
        )
        .unwrap();
        let before = crate::test_support::snapshot(&fixture.library);
        let prepared = prepare_file(&fixture.book, &kobo, options).unwrap();
        assert_eq!(prepared.path, fixture.book.join("BOOK.EPUB"));
        assert!(!prepared.is_temporary());
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);
    }

    #[test]
    fn kobo_preference_falls_back_only_when_route_is_unavailable() {
        let fixture = crate::test_support::Fixture::new();
        let device = crate::test_support::device(DeviceKind::Kobo, &fixture.kobo);
        let files = vec![BookFile {
            format: Format::Epub,
            path: "BOOK.EPUB".into(),
        }];
        let plan = plan_preparation(&files, &device, prefer_kepub(), &[]).unwrap();
        assert!(matches!(plan, PreparationPlan::Ready(_)));
        let mut broken = backend(converter::BackendKind::Kepubify, true);
        broken.status = converter::BackendStatus::Broken;
        assert!(matches!(
            plan_preparation(&files, &device, prefer_kepub(), &[broken]).unwrap(),
            PreparationPlan::Ready(_)
        ));
        let plan = plan_preparation(
            &files,
            &device,
            prefer_kepub(),
            &[backend(converter::BackendKind::Kepubify, true)],
        )
        .unwrap();
        let error = execute_preparation_with(&fixture.book, plan, |_, _, _| {
            Err(crate::Error::unavailable("selected converter failed", None))
        })
        .unwrap_err();
        assert_eq!(error.operation, "selected converter failed");
    }

    #[test]
    fn missing_compatible_format_uses_available_route_or_explains_failure() {
        let fixture = crate::test_support::Fixture::new();
        let files = vec![BookFile {
            format: Format::Docx,
            path: "BOOK.DOCX".into(),
        }];
        let tool = backend(converter::BackendKind::EbookConvert, false);
        for (kind, target) in [
            (DeviceKind::Kobo, Format::Epub),
            (DeviceKind::Kindle, Format::Azw3),
        ] {
            let device = crate::test_support::device(kind, &fixture.kobo);
            let plan = plan_preparation(
                &files,
                &device,
                PreparationOptions::default(),
                std::slice::from_ref(&tool),
            )
            .unwrap();
            let PreparationPlan::Convert(plan) = plan else {
                panic!("expected conversion");
            };
            assert_eq!(plan.target, target);
            assert_eq!(plan.source, files[0]);
            let mut missing = tool.clone();
            missing.status = converter::BackendStatus::Missing;
            let error =
                plan_preparation(&files, &device, PreparationOptions::default(), &[missing])
                    .unwrap_err();
            assert!(matches!(
                error.kind,
                crate::ErrorKind::ConversionUnavailable { .. }
            ));
            assert!(error.to_string().contains("Missing"));
            assert!(plan_preparation(&[], &device, PreparationOptions::default(), &[]).is_err());
        }
        let kindle = crate::test_support::device(DeviceKind::Kindle, &fixture.kindle);
        let kepub = vec![BookFile {
            format: Format::Kepub,
            path: "BOOK.KEPUB.EPUB".into(),
        }];
        assert!(plan_preparation(&kepub, &kindle, PreparationOptions::default(), &[tool]).is_err());
    }

    #[test]
    fn non_epub_source_can_be_prepared_in_two_steps_without_library_writes() {
        let fixture = crate::test_support::Fixture::new();
        let device = crate::test_support::device(DeviceKind::Kobo, &fixture.kobo);
        let files = vec![BookFile {
            format: Format::Pdf,
            path: fixture.book.join("A small book.pdf"),
        }];
        let tools = [
            backend(converter::BackendKind::EbookConvert, false),
            backend(converter::BackendKind::Kepubify, true),
        ];
        let plan = plan_preparation(&files, &device, prefer_kepub(), &tools).unwrap();
        assert!(matches!(plan, PreparationPlan::ViaEpub { .. }));
        let before = crate::test_support::snapshot(&fixture.library);
        let mut targets = Vec::new();
        let prepared = execute_preparation_with(&fixture.book, plan, |_, output, plan| {
            if plan.target == Format::Kepub {
                assert_eq!(plan.source.path, output.join("intermediate.epub"));
                assert!(plan.source.path.is_file());
            }
            targets.push(plan.target);
            let path = output.join(if plan.target == Format::Epub {
                "intermediate.epub"
            } else {
                "result.kepub.epub"
            });
            fs::write(&path, b"synthetic conversion").unwrap();
            Ok(path)
        })
        .unwrap();
        assert_eq!(targets, [Format::Epub, Format::Kepub]);
        assert!(prepared.is_temporary());
        let path = prepared.path.clone();
        assert!(path.is_file());
        drop(prepared);
        assert!(!path.exists());
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);

        // Without kepubify or a KEPUB plugin, the same request falls back to EPUB.
        assert!(
            matches!(plan_preparation(&files, &device, prefer_kepub(), &tools[..1]).unwrap(), PreparationPlan::Convert(plan) if plan.target == Format::Epub)
        );
        let plugin = backend(converter::BackendKind::EbookConvert, true);
        assert!(
            matches!(plan_preparation(&files, &device, prefer_kepub(), &[plugin]).unwrap(), PreparationPlan::Convert(plan) if plan.target == Format::Kepub)
        );
    }

    #[test]
    fn two_step_failure_cleans_intermediate_and_partial_files() {
        let fixture = crate::test_support::Fixture::new();
        let device = crate::test_support::device(DeviceKind::Kobo, &fixture.kobo);
        let files = vec![BookFile {
            format: Format::Pdf,
            path: fixture.book.join("A small book.pdf"),
        }];
        let tools = [
            backend(converter::BackendKind::EbookConvert, false),
            backend(converter::BackendKind::Kepubify, true),
        ];
        let before = crate::test_support::snapshot(&fixture.library);
        for fail_on in [Format::Epub, Format::Kepub] {
            let plan = plan_preparation(&files, &device, prefer_kepub(), &tools).unwrap();
            let mut workspace_path = None;
            let error = execute_preparation_with(&fixture.book, plan, |_, output, plan| {
                workspace_path = Some(output.to_owned());
                let path = output.join(if plan.target == Format::Epub {
                    "intermediate.epub"
                } else {
                    "result.kepub.epub"
                });
                fs::write(&path, b"synthetic partial conversion").unwrap();
                if plan.target == fail_on {
                    Err(crate::Error::unavailable("synthetic step failure", None))
                } else {
                    Ok(path)
                }
            })
            .unwrap_err();
            assert_eq!(error.operation, "synthetic step failure");
            assert!(!workspace_path.unwrap().exists());
            assert_eq!(crate::test_support::snapshot(&fixture.library), before);
        }
    }

    #[test]
    fn converted_device_file_lives_through_copy_and_is_cleaned_afterwards() {
        let fixture = crate::test_support::Fixture::new();
        fs::remove_file(fixture.book.join("A small book.kepub.epub")).unwrap();
        let before = crate::test_support::snapshot(&fixture.library);
        let device = crate::test_support::device(DeviceKind::Kobo, &fixture.kobo);
        let workspace_path;
        {
            let prepared =
                prepare_file_with(&fixture.book, &device, |library, output, source, format| {
                    assert_eq!(format, Format::Kepub);
                    assert!(fs::canonicalize(source).unwrap().starts_with(library));
                    assert!(!output.starts_with(library));
                    let destination = output.join("A small book.kepub.epub");
                    fs::write(&destination, b"converted fixture").unwrap();
                    Ok(destination)
                })
                .unwrap();
            workspace_path = prepared.path.parent().unwrap().to_owned();
            let copied =
                crate::transfer::transfer_fixture(&prepared.path, &device, "Author").unwrap();
            assert_eq!(fs::read(copied).unwrap(), b"converted fixture");
            assert!(prepared.path.is_file());
        }
        assert!(!workspace_path.exists());
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);
    }

    #[test]
    fn failed_preparation_cleans_partial_files_without_changing_library() {
        let fixture = crate::test_support::Fixture::new();
        fs::remove_file(fixture.book.join("A small book.kepub.epub")).unwrap();
        let before = crate::test_support::snapshot(&fixture.library);
        let device = crate::test_support::device(DeviceKind::Kobo, &fixture.kobo);
        let mut workspace_path = None;
        let result = prepare_file_with(&fixture.book, &device, |_, output, _, _| {
            workspace_path = Some(output.to_owned());
            fs::write(output.join("partial.kepub.epub"), b"partial").unwrap();
            Err(crate::Error::unavailable(
                "synthetic conversion failure",
                None,
            ))
        });
        assert!(result.is_err());
        assert!(!workspace_path.unwrap().exists());
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);
    }

    #[test]
    fn device_destination_cannot_be_the_library() {
        let fixture = crate::test_support::Fixture::new();
        let before = crate::test_support::snapshot(&fixture.library);
        let state = DeviceState {
            connected: Some(crate::test_support::device(
                DeviceKind::Kindle,
                &fixture.library,
            )),
        };
        assert!(state.send_book(&fixture.book, "Author").is_err());
        assert_eq!(crate::test_support::snapshot(&fixture.library), before);
    }

    #[test]
    fn file_selection_respects_exact_format_priority_and_path_order() {
        let files = vec![
            BookFile {
                format: Format::Epub,
                path: "z.EPUB".into(),
            },
            BookFile {
                format: Format::Kepub,
                path: "b.KEPUB.EPUB".into(),
            },
            BookFile {
                format: Format::Epub,
                path: "a.epub".into(),
            },
        ];
        assert_eq!(
            find_file(&files, &[Format::Kepub, Format::Epub]),
            Some("b.KEPUB.EPUB".into())
        );
        assert_eq!(find_file(&files, &[Format::Epub]), Some("a.epub".into()));
        assert!(find_file(&files[1..2], &[Format::Epub]).is_none());
    }
}

/// Complete discovery result: per-volume failures do not hide other readers.
#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub devices: Vec<Device>,
    pub warnings: Vec<crate::Error>,
}

impl DiscoveryReport {
    /// Existing clients have no chooser yet. Never select the first of several.
    pub fn single_device(&self) -> Option<&Device> {
        if self.devices.len() == 1 {
            self.devices.first()
        } else {
            None
        }
    }
}

pub fn detect_devices() -> Result<DiscoveryReport, crate::Error> {
    let volumes = crate::volumes::mounted_volumes()?;
    let mut report = detect_devices_from_volumes(&volumes.volumes);
    report.warnings.extend(volumes.warnings);
    Ok(report)
}

/// Recognize a snapshot supplied by a filesystem-volume enumerator.
/// This boundary also permits tests without a physical USB reader.
pub fn detect_devices_from_volumes(volumes: &[crate::volumes::MountedVolume]) -> DiscoveryReport {
    use std::collections::BTreeMap;
    let mut devices = BTreeMap::new();
    let mut report = DiscoveryReport::default();
    let mut ordered = volumes.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| (&a.id, &a.mount_point).cmp(&(&b.id, &b.mount_point)));
    for volume in ordered {
        if !crate::volumes::local_filesystem(&volume.filesystem) {
            continue;
        }
        match recognize(volume) {
            Ok(Some(device)) => {
                devices.entry(device.id.clone()).or_insert(device);
            }
            Ok(None) => {}
            Err(error) => report.warnings.push(error),
        }
    }
    report.devices = devices.into_values().collect();
    report
}

fn invalid_device(path: &Path, reason: &str) -> crate::Error {
    crate::Error::invalid_data("validate reader filesystem", path, reason)
}

fn directory(path: &Path) -> Result<bool, crate::Error> {
    match fs::symlink_metadata(path) {
        // Do not follow symlinks or junctions to signatures on another filesystem.
        Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(crate::Error::io("inspect reader directory", path, error)),
    }
}

fn read_marker(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, crate::Error> {
    use std::io::Read;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::Error::io("inspect reader marker", path, error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid_device(path, "marker must be a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(limit as u64).read_to_end(&mut bytes))
        .map_err(|error| crate::Error::io("read reader marker", path, error))?;
    Ok(Some(bytes))
}

fn dotted_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn label_kind(label: &str) -> Option<DeviceKind> {
    match label.trim().to_ascii_lowercase().as_str() {
        "kobo" | "koboereader" => Some(DeviceKind::Kobo),
        "kindle" | "kindle main memory" | "kindle storage card" => Some(DeviceKind::Kindle),
        _ => None,
    }
}

fn recognized_markers(
    root: &Path,
) -> Result<Vec<(DeviceKind, Vec<DetectionEvidence>)>, crate::Error> {
    let mut found = Vec::new();
    if directory(&root.join(".kobo"))? {
        let version_path = root.join(".kobo/version");
        let version = read_marker(&version_path, 4097)?;
        let database_path = root.join(".kobo/KoboReader.sqlite");
        let database = read_marker(&database_path, 16)?;
        let valid_version = version.as_ref().is_some_and(|bytes| {
            if bytes.len() > 4096 {
                return false;
            }
            std::str::from_utf8(bytes).is_ok_and(|text| {
                let fields = text.trim().split(',').collect::<Vec<_>>();
                fields.len() >= 3
                    && !fields[0].trim().is_empty()
                    && dotted_version(fields[2].trim())
            })
        });
        if valid_version && database.as_deref() == Some(b"SQLite format 3\0") {
            found.push((
                DeviceKind::Kobo,
                vec![
                    DetectionEvidence::KoboVersion,
                    DetectionEvidence::KoboDatabase,
                ],
            ));
        }
    }
    if directory(&root.join("documents"))? && directory(&root.join("system"))? {
        let version = read_marker(&root.join("system/version.txt"), 4097)?;
        let valid_version = version.as_ref().is_some_and(|bytes| {
            bytes.len() <= 4096
                && std::str::from_utf8(bytes).is_ok_and(|text| {
                    text.trim()
                        .strip_prefix("Kindle ")
                        .and_then(|rest| rest.split_whitespace().next())
                        .is_some_and(dotted_version)
                })
        });
        if valid_version {
            found.push((
                DeviceKind::Kindle,
                vec![
                    DetectionEvidence::KindleDocuments,
                    DetectionEvidence::KindleVersion,
                ],
            ));
        }
    }
    Ok(found)
}

fn make_device(
    volume: &crate::volumes::MountedVolume,
    kind: DeviceKind,
    mut evidence: Vec<DetectionEvidence>,
) -> Device {
    if let Some(label) = &volume.label {
        evidence.push(DetectionEvidence::VolumeLabel(label.clone()));
    }
    Device {
        id: DeviceId(format!("{:?}:{}", kind, volume.id)),
        kind,
        mount_point: volume.mount_point.clone(),
        label: volume.label.clone(),
        filesystem: volume.filesystem.clone(),
        read_only: volume.read_only,
        evidence,
    }
}

fn recognize(volume: &crate::volumes::MountedVolume) -> Result<Option<Device>, crate::Error> {
    if !volume.mount_point.is_dir() {
        return fs::metadata(&volume.mount_point)
            .map_err(|error| {
                crate::Error::io("inspect mounted filesystem", &volume.mount_point, error)
            })
            .and_then(|_| {
                Err(invalid_device(
                    &volume.mount_point,
                    "mount point is not a directory",
                ))
            });
    }
    // A Calibre library copied onto a disk must not become a reader destination.
    match fs::symlink_metadata(volume.mount_point.join("metadata.db")) {
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(crate::Error::io(
                "inspect mounted library marker",
                &volume.mount_point,
                error,
            ));
        }
    }
    let mut found = recognized_markers(&volume.mount_point)?;
    if found.len() > 1 {
        return Err(invalid_device(
            &volume.mount_point,
            "conflicting Kobo and Kindle signatures; manual selection required",
        ));
    }
    let Some((kind, evidence)) = found.pop() else {
        if volume.label.as_deref().and_then(label_kind).is_some() {
            return Err(invalid_device(
                &volume.mount_point,
                "reader label without confirmed reader signatures",
            ));
        }
        return Ok(None);
    };
    if volume
        .label
        .as_deref()
        .and_then(label_kind)
        .is_some_and(|label| label != kind)
    {
        return Err(invalid_device(
            &volume.mount_point,
            "volume label conflicts with reader signatures; manual selection required",
        ));
    }
    Ok(Some(make_device(volume, kind, evidence)))
}

/// Explicit type selection for a mounted local filesystem. Does not create any
/// directories or marker files and does not expose MTP as a filesystem device.
pub fn manual_device(path: &Path, kind: DeviceKind) -> Result<Device, crate::Error> {
    let volumes = crate::volumes::mounted_volumes()?;
    manual_device_from_volumes(path, kind, &volumes.volumes)
}

/// Validate a manual path against a current volume snapshot (injectable in tests).
pub fn manual_device_from_volumes(
    path: &Path,
    kind: DeviceKind,
    volumes: &[crate::volumes::MountedVolume],
) -> Result<Device, crate::Error> {
    let root = fs::canonicalize(path)
        .map_err(|error| crate::Error::io("resolve manual device path", path, error))?;
    if !root.is_dir() {
        return Err(invalid_device(
            path,
            "manual path must be a mounted filesystem root",
        ));
    }
    let mut selected = None;
    for volume in volumes {
        if !crate::volumes::local_filesystem(&volume.filesystem) {
            continue;
        }
        match fs::canonicalize(&volume.mount_point) {
            Ok(candidate) if candidate == root => {
                selected = Some(volume);
                break;
            }
            Ok(_) => {}
            Err(error) if volume.mount_point == path => {
                return Err(crate::Error::io(
                    "resolve mounted manual device",
                    path,
                    error,
                ));
            }
            Err(_) => {} // unrelated unavailable volume cannot invalidate this root
        }
    }
    let volume = selected.ok_or_else(|| {
        invalid_device(
            path,
            "path is not a mounted local filesystem root; MTP is unsupported",
        )
    })?;
    if volume.read_only {
        return Err(invalid_device(path, "mounted filesystem is read-only"));
    }
    match fs::symlink_metadata(root.join("metadata.db")) {
        Ok(_) => {
            return Err(invalid_device(
                path,
                "a Calibre library cannot be a reader destination",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(crate::Error::io("inspect manual device root", path, error)),
    }
    let required = root.join(match kind {
        DeviceKind::Kobo => ".kobo",
        DeviceKind::Kindle => "documents",
    });
    if !directory(&required)? {
        return Err(invalid_device(
            &required,
            "required reader directory missing or linked elsewhere",
        ));
    }
    // Explicit selection can resolve missing or conflicting automatic signatures;
    // the evidence distinguishes this user assertion from automatic recognition.
    Ok(make_device(
        volume,
        kind,
        vec![DetectionEvidence::ManualSelection],
    ))
}
