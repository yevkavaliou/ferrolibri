//! Stoppable core watcher bridged into iced; filesystem queries stay on its worker.
use iced::{
    Subscription,
    futures::{SinkExt, Stream},
};
use rulibre_core::{
    device::{self, Device, DeviceEvent, DeviceId, DeviceKind, DiscoveryReport},
    device_watch::DeviceWatcher,
    volumes::MountedVolume,
};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Space {
    pub id: DeviceId,
    pub path: PathBuf,
    pub bytes: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct DeviceUpdate {
    pub revision: u64,
    pub event: Option<DeviceEvent>,
    pub space: Vec<Space>,
}

#[derive(Default)]
pub struct ReaderState {
    pub devices: Vec<Device>,
    pub selected_id: Option<DeviceId>,
    pub warnings: Vec<String>,
    pub failure: Option<String>,
    pub space: BTreeMap<(DeviceId, PathBuf), Option<u64>>,
    pub revision: u64,
}
pub struct ReaderPicker {
    pub open: bool,
    pub manual: bool,
    pub path: String,
    pub native_path: Option<PathBuf>,
    pub kind: DeviceKind,
    pub busy: bool,
    pub error: Option<String>,
    pub generation: u64,
}
impl Default for ReaderPicker {
    fn default() -> Self {
        Self {
            open: false,
            manual: false,
            path: String::new(),
            native_path: None,
            kind: DeviceKind::Kobo,
            busy: false,
            error: None,
            generation: 0,
        }
    }
}
impl ReaderPicker {
    pub fn close(&mut self) {
        self.open = false;
        self.busy = false;
        self.generation += 1;
    }
}
impl ReaderState {
    pub fn selected(&self) -> Option<&Device> {
        self.devices
            .iter()
            .find(|d| Some(&d.id) == self.selected_id.as_ref())
    }
    pub fn select(&mut self, id: DeviceId) -> bool {
        if self.failure.is_some() || !self.devices.iter().any(|d| d.id == id) {
            return false;
        }
        self.selected_id = Some(id);
        true
    }
    pub fn apply(&mut self, update: DeviceUpdate) {
        if update.revision < self.revision {
            return;
        }
        self.revision = update.revision;
        self.space = update
            .space
            .into_iter()
            .map(|s| ((s.id, s.path), s.bytes))
            .collect();
        match update.event {
            Some(DeviceEvent::PollFailed { diagnostic }) => {
                self.selected_id = None;
                self.devices.clear();
                self.space.clear();
                self.warnings.clear();
                self.failure = Some(diagnostic);
            }
            Some(DeviceEvent::Snapshot {
                devices, warnings, ..
            }) => {
                // Preserve an explicit choice while its identity is still present.
                // A sole reader is automatic; never pick the first of several.
                if !devices
                    .iter()
                    .any(|d| Some(&d.id) == self.selected_id.as_ref())
                {
                    self.selected_id = None;
                }
                if self.selected_id.is_none() && devices.len() == 1 {
                    self.selected_id = Some(devices[0].id.clone());
                }
                self.devices = devices;
                self.warnings = warnings;
                self.failure = None;
            }
            None => {}
        }
    }
    pub fn available(&self) -> Option<u64> {
        let d = self.selected()?;
        self.space
            .get(&(d.id.clone(), d.mount_point.clone()))
            .copied()
            .flatten()
    }
    pub fn can_send(&self, selected_books: usize) -> bool {
        selected_books > 0
            && self.failure.is_none()
            && self.selected().is_some_and(Device::can_transfer)
    }
}

#[derive(Default)]
struct Manual {
    revision: u64,
    reader: Option<(PathBuf, DeviceKind)>,
}
#[derive(Default)]
pub struct DeviceFeed {
    manual: Mutex<Manual>,
    watcher: Mutex<Option<DeviceWatcher>>,
    stopped: AtomicBool,
}
impl DeviceFeed {
    pub fn remember(&self, reader: &Device) -> u64 {
        let mut manual = self.manual.lock().unwrap();
        manual.revision += 1;
        manual.reader = Some((reader.mount_point.clone(), reader.kind));
        manual.revision
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(mut watcher) = self.watcher.lock().unwrap().take()
            && watcher.stop().is_err()
        {
            log::error!("Device watcher panicked during shutdown");
        }
    }
    pub fn subscription(self: &Arc<Self>) -> Subscription<DeviceUpdate> {
        Subscription::run_with(FeedKey(self.clone()), stream)
    }
    fn start_with(
        self: &Arc<Self>,
        interval: Duration,
        mut discover: impl FnMut(
            Option<(PathBuf, DeviceKind)>,
        ) -> Result<DiscoveryReport, rulibre_core::Error>
        + Send
        + 'static,
        sender: tokio::sync::mpsc::UnboundedSender<DeviceUpdate>,
    ) -> Result<(), String> {
        let mut slot = self.watcher.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            return Err("Device watcher is stopped".into());
        }
        if slot.is_some() {
            return Err("Device watcher already running".into());
        }
        let feed = self.clone();
        let pending = Arc::new(Mutex::new((0, Vec::<Space>::new())));
        let publish_pending = pending.clone();
        let poll_sender = sender.clone();
        *slot = Some(
            DeviceWatcher::start_with(
                interval,
                move || {
                    let (revision, manual) = {
                        let manual = feed.manual.lock().unwrap();
                        (manual.revision, manual.reader.clone())
                    };
                    let report = discover(manual);
                    let space: Vec<_> = report
                        .as_ref()
                        .map(|r| {
                            r.devices
                                .iter()
                                .map(|d| Space {
                                    id: d.id.clone(),
                                    path: d.mount_point.clone(),
                                    bytes: if d.is_mtp() {
                                        None
                                    } else {
                                        rulibre_core::volumes::free_space(&d.mount_point).ok()
                                    },
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    *pending.lock().unwrap() = (revision, space.clone());
                    // DeviceTracker suppresses unchanged snapshots; space still refreshes every poll.
                    if poll_sender
                        .send(DeviceUpdate {
                            revision,
                            event: None,
                            space,
                        })
                        .is_err()
                    {
                        return Err(rulibre_core::Error::unavailable(
                            "deliver reader poll",
                            None,
                        ));
                    }
                    report
                },
                move |event| {
                    let (revision, space) = publish_pending.lock().unwrap().clone();
                    sender
                        .send(DeviceUpdate {
                            revision,
                            event: Some(event),
                            space,
                        })
                        .is_ok()
                },
            )
            .map_err(|e| format!("Could not start reader detection. {e}"))?,
        );
        Ok(())
    }
}
impl Drop for DeviceFeed {
    fn drop(&mut self) {
        self.stop();
    }
}

fn discover(manual: Option<(PathBuf, DeviceKind)>) -> Result<DiscoveryReport, rulibre_core::Error> {
    let volumes = rulibre_core::volumes::mounted_volumes()?;
    let mut report = from_volumes(&volumes.volumes, manual);
    report.warnings.extend(volumes.warnings);
    device::merge_portable_devices(&mut report);
    Ok(report)
}
fn from_volumes(
    volumes: &[MountedVolume],
    manual: Option<(PathBuf, DeviceKind)>,
) -> DiscoveryReport {
    let mut report = device::detect_devices_from_volumes(volumes);
    if let Some((path, kind)) = manual {
        match device::manual_device_from_volumes(&path, kind, volumes) {
            Ok(reader) => {
                report
                    .devices
                    .retain(|d| d.mount_point != reader.mount_point);
                report.devices.push(reader);
                report.devices.sort_by(|a, b| a.id.cmp(&b.id));
            }
            Err(error) => report.warnings.push(error),
        }
    }
    report
}

#[derive(Clone)]
struct FeedKey(Arc<DeviceFeed>);
impl Hash for FeedKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}
struct StopOnDrop(Arc<DeviceFeed>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}
fn stream(key: &FeedKey) -> impl Stream<Item = DeviceUpdate> + Send + 'static + use<> {
    stream_with(key.0.clone(), discover)
}
fn stream_with(
    feed: Arc<DeviceFeed>,
    discover: impl FnMut(Option<(PathBuf, DeviceKind)>) -> Result<DiscoveryReport, rulibre_core::Error>
    + Send
    + 'static,
) -> impl Stream<Item = DeviceUpdate> + Send + 'static {
    iced::stream::channel(8, async move |mut output| {
        let _stop = StopOnDrop(feed.clone());
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        if let Err(diagnostic) = feed.start_with(Duration::from_secs(2), discover, sender) {
            let _ = output
                .send(DeviceUpdate {
                    revision: 0,
                    event: Some(DeviceEvent::PollFailed { diagnostic }),
                    space: vec![],
                })
                .await;
            return;
        }
        while let Some(update) = receiver.recv().await {
            if output.send(update).await.is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::StreamExt;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn mtp_reader_selection_and_transfer_availability() {
        let mut state = ReaderState::default();
        let reader = Device {
            id: DeviceId("Kindle:windows:mtp:fixture".into()),
            kind: DeviceKind::Kindle,
            mount_point: PathBuf::new(),
            label: Some("Kindle Paperwhite".into()),
            filesystem: "MTP".into(),
            read_only: false,
            evidence: vec![device::DetectionEvidence::WindowsPortableDevice],
        };
        state.apply(DeviceUpdate {
            revision: 0,
            event: Some(DeviceEvent::Snapshot {
                devices: vec![reader.clone()],
                selected_id: Some(reader.id.clone()),
                changes: vec![],
                warnings: vec![],
            }),
            space: vec![],
        });
        assert_eq!(state.selected(), Some(&reader));
        assert_eq!(state.can_send(1), cfg!(windows));
        assert_eq!(state.available(), None);
        state.apply(DeviceUpdate {
            revision: 0,
            event: Some(DeviceEvent::Snapshot {
                devices: vec![],
                selected_id: None,
                changes: vec![],
                warnings: vec![],
            }),
            space: vec![],
        });
        assert!(state.selected().is_none());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an attached MTP Kindle"]
    fn native_mtp_discovery_reaches_ui() {
        let report = discover(None).unwrap();
        assert!(report.devices.iter().any(Device::is_mtp));
        let mut state = ReaderState::default();
        state.apply(DeviceUpdate {
            revision: 0,
            event: Some(DeviceEvent::Snapshot {
                devices: report.devices,
                selected_id: None,
                changes: vec![],
                warnings: report.warnings.iter().map(ToString::to_string).collect(),
            }),
            space: vec![],
        });
        let reader = state.devices.iter().find(|d| d.is_mtp()).unwrap();
        println!("UI discovered: {}", reader.label.as_deref().unwrap());
        assert!(state.select(reader.id.clone()));
        assert!(state.selected().unwrap().is_mtp());
        assert!(state.can_send(1));
    }

    fn volume(root: &std::path::Path, id: &str, kind: DeviceKind) -> MountedVolume {
        let path = root.join(id);
        std::fs::create_dir_all(path.join(if kind == DeviceKind::Kobo {
            ".kobo"
        } else {
            "documents"
        }))
        .unwrap();
        if kind == DeviceKind::Kobo {
            std::fs::write(
                path.join(".kobo/version"),
                b"SYNTHETIC-KOBO,unused,4.38.21908,00000000-0000-0000-0000-000000000000\n",
            )
            .unwrap();
            std::fs::write(
                path.join(".kobo/KoboReader.sqlite"),
                b"SQLite format 3\0fixture",
            )
            .unwrap();
        } else {
            std::fs::create_dir_all(path.join("system")).unwrap();
            std::fs::write(
                path.join("system/version.txt"),
                b"Kindle 5.4.0 (206145 050)\n",
            )
            .unwrap();
        }
        MountedVolume {
            id: id.into(),
            mount_point: path,
            label: None,
            filesystem: "FAT32".into(),
            read_only: false,
        }
    }
    fn event(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<DeviceUpdate>) -> DeviceUpdate {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(update) = receiver.try_recv()
                && update.event.is_some()
            {
                return update;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "watcher event timed out"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn core_folder_snapshot_feed_connect_replace_select_fail_recover_and_stop() {
        let fixture = tempfile::tempdir().unwrap();
        let a = volume(fixture.path(), "a", DeviceKind::Kobo);
        let b = volume(fixture.path(), "b", DeviceKind::Kindle);
        let volumes = Arc::new(Mutex::new(Some(vec![])));
        let polls = Arc::new(AtomicUsize::new(0));
        let source = volumes.clone();
        let count = polls.clone();
        let feed = Arc::new(DeviceFeed::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        feed.start_with(
            Duration::from_millis(2),
            move |manual| {
                count.fetch_add(1, Ordering::SeqCst);
                source
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|v| from_volumes(v, manual))
                    .ok_or_else(|| rulibre_core::Error::unavailable("fixture poll", None))
            },
            tx,
        )
        .unwrap();
        let mut state = ReaderState::default();
        state.apply(event(&mut rx));
        assert!(state.devices.is_empty());
        *volumes.lock().unwrap() = Some(vec![a.clone()]);
        state.apply(event(&mut rx));
        let a_id = state.selected_id.clone().unwrap();
        assert!(state.available().is_some());
        assert!(!state.can_send(0));
        assert!(state.can_send(2));
        // Start with two readers: there must be no arbitrary first selection.
        *volumes.lock().unwrap() = Some(vec![]);
        state.apply(event(&mut rx));
        *volumes.lock().unwrap() = Some(vec![a.clone(), b.clone()]);
        state.apply(event(&mut rx));
        assert_eq!(state.devices.len(), 2);
        assert!(state.selected().is_none());
        let b_id = state
            .devices
            .iter()
            .find(|d| d.kind == DeviceKind::Kindle)
            .unwrap()
            .id
            .clone();
        assert!(state.select(b_id.clone()));
        assert!(!state.select(DeviceId("unknown".into())));
        // A changed device updates selected attributes, including read-only and path.
        let mut moved_b = b.clone();
        moved_b.mount_point = fixture.path().join("b moved");
        // Publish the filesystem move and synthetic volume snapshot together:
        // the watcher must not observe the old path disappearing mid-update.
        let mut snapshot = volumes.lock().unwrap();
        std::fs::rename(&b.mount_point, &moved_b.mount_point).unwrap();
        moved_b.read_only = true;
        *snapshot = Some(vec![a.clone(), moved_b.clone()]);
        drop(snapshot);
        state.apply(event(&mut rx));
        assert_eq!(state.selected_id, Some(b_id.clone()));
        assert_eq!(state.selected().unwrap().mount_point, moved_b.mount_point);
        assert!(!state.can_send(1));
        *volumes.lock().unwrap() = None;
        state.apply(event(&mut rx));
        assert!(state.failure.as_ref().unwrap().contains("fixture poll"));
        assert!(state.devices.is_empty());
        assert!(!state.select(b_id));
        *volumes.lock().unwrap() = Some(vec![a.clone(), moved_b]);
        state.apply(event(&mut rx));
        assert!(state.failure.is_none());
        assert!(state.selected().is_none());
        *volumes.lock().unwrap() = Some(vec![a.clone()]);
        state.apply(event(&mut rx));
        assert_eq!(state.selected_id, Some(a_id));
        let mut replacement = a;
        replacement.id = "replacement".into();
        *volumes.lock().unwrap() = Some(vec![replacement]);
        state.apply(event(&mut rx));
        assert!(
            state
                .selected_id
                .as_ref()
                .unwrap()
                .0
                .contains("replacement")
        );
        *volumes.lock().unwrap() = Some(vec![]);
        state.apply(event(&mut rx));
        assert!(state.selected().is_none());
        feed.stop();
        feed.stop();
        let stopped = polls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(polls.load(Ordering::SeqCst), stopped);
        assert!(feed.watcher.lock().unwrap().is_none());
        assert_eq!(Arc::strong_count(&feed), 1);
    }
    #[test]
    fn iced_subscription_drop_joins_and_closed_receiver_stops_callback() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let feed = Arc::new(DeviceFeed::default());
        runtime.block_on(async {
            let mut stream = Box::pin(stream_with(
                feed.clone(),
                |_| Ok(DiscoveryReport::default()),
            ));
            stream.next().await.unwrap();
            drop(stream);
        });
        assert!(feed.watcher.lock().unwrap().is_none());
        assert!(feed.stopped.load(Ordering::SeqCst));
        assert_eq!(Arc::strong_count(&feed), 1);
        let second = Arc::new(DeviceFeed::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        second
            .start_with(
                Duration::from_millis(1),
                move |_| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(DiscoveryReport::default())
                },
                tx,
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(10));
        second.stop();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn manual_folder_revalidated_each_poll_and_stale_poll_cannot_replace_choice() {
        let fixture = tempfile::tempdir().unwrap();
        let mut a = volume(fixture.path(), "manual", DeviceKind::Kobo);
        // Enough for explicit manual selection, insufficient for automatic discovery.
        std::fs::remove_file(a.mount_point.join(".kobo/version")).unwrap();
        std::fs::remove_file(a.mount_point.join(".kobo/KoboReader.sqlite")).unwrap();
        assert!(from_volumes(&[a.clone()], None).devices.is_empty());
        let manual = Some((a.mount_point.clone(), DeviceKind::Kobo));
        let report = from_volumes(&[a.clone()], manual.clone());
        assert_eq!(report.devices.len(), 1);
        let reader = report.devices[0].clone();
        assert!(
            reader
                .evidence
                .contains(&device::DetectionEvidence::ManualSelection)
        );
        let feed = DeviceFeed::default();
        let revision = feed.remember(&reader);
        let mut state = ReaderState {
            revision,
            devices: report.devices,
            selected_id: Some(reader.id),
            ..Default::default()
        };
        state.apply(DeviceUpdate {
            revision: 0,
            event: Some(DeviceEvent::Snapshot {
                devices: vec![],
                selected_id: None,
                changes: vec![],
                warnings: vec![],
            }),
            space: vec![],
        });
        assert!(state.selected().is_some());
        a.read_only = true;
        let report = from_volumes(&[a], manual.clone());
        assert!(report.devices.is_empty());
        assert!(!report.warnings.is_empty());
        assert!(from_volumes(&[], manual).devices.is_empty());
    }
    #[test]
    fn space_updates_without_snapshot_and_changed_mount_never_uses_old_bytes() {
        let fixture = tempfile::tempdir().unwrap();
        let a = volume(fixture.path(), "a", DeviceKind::Kobo);
        let mut reader = from_volumes(&[a], None).devices.remove(0);
        let mut state = ReaderState::default();
        let snapshot = |reader: Device, warnings| DeviceEvent::Snapshot {
            devices: vec![reader],
            selected_id: None,
            changes: vec![],
            warnings,
        };
        state.apply(DeviceUpdate {
            revision: 0,
            event: Some(snapshot(reader.clone(), vec!["volume warning".into()])),
            space: vec![],
        });
        assert_eq!(state.warnings, ["volume warning"]);
        assert!(state.available().is_none());
        for bytes in [Some(7), Some(0), None] {
            state.apply(DeviceUpdate {
                revision: 0,
                event: None,
                space: vec![Space {
                    id: reader.id.clone(),
                    path: reader.mount_point.clone(),
                    bytes,
                }],
            });
            assert_eq!(state.available(), bytes);
        }
        let old = Space {
            id: reader.id.clone(),
            path: reader.mount_point.clone(),
            bytes: Some(123),
        };
        reader.mount_point = "new-mount".into();
        state.apply(DeviceUpdate {
            revision: 0,
            event: Some(snapshot(reader, vec![])),
            space: vec![old],
        });
        assert!(state.available().is_none());
    }
}
