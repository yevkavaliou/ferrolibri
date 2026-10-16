//! UI-independent snapshot comparison, selection and stoppable polling worker.
use std::{
    collections::BTreeMap,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    Error,
    device::{self, DiscoveryReport},
    device_model::{Device, DeviceChange, DeviceEvent, DeviceId},
};

#[derive(Default)]
pub struct DeviceTracker {
    devices: BTreeMap<DeviceId, Device>,
    selected_id: Option<DeviceId>,
    initialized: bool,
    failure: Option<String>,
    warnings: Vec<String>,
}

impl DeviceTracker {
    pub fn selected_id(&self) -> Option<&DeviceId> {
        self.selected_id.as_ref()
    }

    /// Explicit selection is allowed only from the latest successful snapshot.
    pub fn select(&mut self, id: &DeviceId) -> Option<DeviceEvent> {
        if self.failure.is_some() || !self.devices.contains_key(id) {
            return None;
        }
        self.selected_id = Some(id.clone());
        Some(self.snapshot(Vec::new()))
    }

    fn snapshot(&self, changes: Vec<DeviceChange>) -> DeviceEvent {
        DeviceEvent::Snapshot {
            devices: self.devices.values().cloned().collect(),
            selected_id: self.selected_id.clone(),
            changes,
            warnings: self.warnings.clone(),
        }
    }

    pub fn poll(&mut self, result: Result<DiscoveryReport, Error>) -> Option<DeviceEvent> {
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                let diagnostic = error.to_string();
                self.selected_id = None;
                if self.failure.as_ref() == Some(&diagnostic) {
                    return None;
                }
                self.failure = Some(diagnostic.clone());
                return Some(DeviceEvent::PollFailed { diagnostic });
            }
        };
        let devices: BTreeMap<_, _> = report
            .devices
            .into_iter()
            .map(|d| (d.id.clone(), d))
            .collect();
        let mut changes = Vec::new();
        for (id, previous) in &self.devices {
            match devices.get(id) {
                None => changes.push(DeviceChange::Disconnected(id.clone())),
                Some(current) if previous != current => changes.push(DeviceChange::Changed {
                    previous: previous.clone(),
                    current: current.clone(),
                }),
                _ => {}
            }
        }
        for (id, current) in &devices {
            if !self.devices.contains_key(id) {
                changes.push(DeviceChange::Connected(current.clone()));
            }
        }
        let previous_selection = self.selected_id.clone();
        if self
            .selected_id
            .as_ref()
            .is_some_and(|id| !devices.contains_key(id))
        {
            self.selected_id = None;
        }
        if self.selected_id.is_none() && devices.len() == 1 {
            self.selected_id = devices.keys().next().cloned();
        }
        let warnings = report
            .warnings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let recovered = self.failure.take().is_some();
        let notify = !self.initialized
            || recovered
            || !changes.is_empty()
            || previous_selection != self.selected_id
            || warnings != self.warnings;
        self.devices = devices;
        self.warnings = warnings;
        self.initialized = true;
        notify.then(|| self.snapshot(changes))
    }
}

/// Owns the worker. Drop requests stop, interrupts the polling delay and joins.
/// An in-flight filesystem query/callback must return before joining can finish.
pub struct DeviceWatcher {
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl DeviceWatcher {
    pub fn start(
        interval: Duration,
        publish: impl FnMut(DeviceEvent) -> bool + Send + 'static,
    ) -> std::io::Result<Self> {
        Self::start_with(interval, device::detect_devices, publish)
    }

    /// Returning false from publish also stops the worker (e.g. a closed UI channel).
    pub fn start_with(
        interval: Duration,
        mut detect: impl FnMut() -> Result<DiscoveryReport, Error> + Send + 'static,
        mut publish: impl FnMut(DeviceEvent) -> bool + Send + 'static,
    ) -> std::io::Result<Self> {
        let (stop, stopped) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("device-watch".into())
            .spawn(move || {
                let mut tracker = DeviceTracker::default();
                loop {
                    if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        break;
                    }
                    let event = tracker.poll(detect());
                    if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        break;
                    }
                    if let Some(event) = event
                        && !publish(event)
                    {
                        break;
                    }
                    if !matches!(
                        stopped.recv_timeout(interval),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    pub fn stop(&mut self) -> thread::Result<()> {
        let _ = self.stop.send(());
        self.worker.take().map_or(Ok(()), |worker| worker.join())
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceKind, DeviceState};

    fn reader(id: &str, path: &str) -> Device {
        let mut device = crate::test_support::device(DeviceKind::Kobo, std::path::Path::new(path));
        device.id = DeviceId(id.into());
        device
    }

    fn report(devices: Vec<Device>) -> Result<DiscoveryReport, Error> {
        Ok(DiscoveryReport {
            devices,
            warnings: vec![],
        })
    }

    fn changes(event: &DeviceEvent) -> &[DeviceChange] {
        let DeviceEvent::Snapshot { changes, .. } = event else {
            panic!("expected snapshot")
        };
        changes
    }

    #[test]
    fn connect_disconnect_reconnect_and_unchanged_snapshot() {
        let a = reader("a", "reader-a");
        let mut tracker = DeviceTracker::default();
        assert!(
            tracker
                .poll(report(vec![]))
                .unwrap()
                .selected_device()
                .is_none()
        );
        let event = tracker.poll(report(vec![a.clone()])).unwrap();
        assert_eq!(changes(&event), &[DeviceChange::Connected(a.clone())]);
        assert_eq!(event.selected_device(), Some(&a));
        assert!(tracker.poll(report(vec![a.clone()])).is_none());
        let event = tracker.poll(report(vec![])).unwrap();
        assert_eq!(changes(&event), &[DeviceChange::Disconnected(a.id.clone())]);
        assert!(tracker.selected_id().is_none());
        assert_eq!(
            tracker
                .poll(report(vec![a.clone()]))
                .unwrap()
                .selected_device(),
            Some(&a)
        );
    }

    #[test]
    fn replacement_without_empty_poll_updates_consumer() {
        let a = reader("a", "reader-a");
        let b = reader("b", "reader-a");
        let mut tracker = DeviceTracker::default();
        let mut state = DeviceState::default();
        state
            .handle_event(tracker.poll(report(vec![a.clone()])).unwrap())
            .unwrap();
        let event = tracker.poll(report(vec![b.clone()])).unwrap();
        assert_eq!(
            changes(&event),
            &[
                DeviceChange::Disconnected(a.id),
                DeviceChange::Connected(b.clone())
            ]
        );
        state.handle_event(event).unwrap();
        assert_eq!(state.connected, Some(b));
    }

    #[test]
    fn same_id_changed_mount_path_and_attributes_refresh_selected_device() {
        let a = reader("a", "old-root");
        let mut current = a.clone();
        current.mount_point = "new-root".into();
        current.read_only = true;
        current.label = Some("Renamed".into());
        let mut tracker = DeviceTracker::default();
        tracker.poll(report(vec![a.clone()]));
        let event = tracker.poll(report(vec![current.clone()])).unwrap();
        assert_eq!(
            changes(&event),
            &[DeviceChange::Changed {
                previous: a,
                current: current.clone()
            }]
        );
        assert_eq!(event.selected_device(), Some(&current));
    }

    #[test]
    fn multiple_readers_preserve_existing_selection_but_never_choose_first() {
        let a = reader("a", "a-root");
        let b = reader("b", "b-root");
        let c = reader("c", "c-root");
        let mut tracker = DeviceTracker::default();
        assert!(
            tracker
                .poll(report(vec![b.clone(), a.clone()]))
                .unwrap()
                .selected_device()
                .is_none()
        );
        assert!(tracker.poll(report(vec![a.clone(), b.clone()])).is_none());
        assert!(tracker.select(&c.id).is_none());
        assert_eq!(tracker.select(&a.id).unwrap().selected_device(), Some(&a));
        assert_eq!(
            tracker
                .poll(report(vec![a.clone(), b.clone(), c.clone()]))
                .unwrap()
                .selected_device(),
            Some(&a)
        );
        assert!(
            tracker
                .poll(report(vec![b.clone(), c]))
                .unwrap()
                .selected_device()
                .is_none()
        );
        assert_eq!(
            tracker
                .poll(report(vec![b.clone()]))
                .unwrap()
                .selected_device(),
            Some(&b)
        );
    }

    #[test]
    fn failed_poll_clears_selection_deduplicates_error_and_recovers() {
        let a = reader("a", "a-root");
        let mut tracker = DeviceTracker::default();
        let mut state = DeviceState::default();
        state
            .handle_event(tracker.poll(report(vec![a.clone()])).unwrap())
            .unwrap();
        let failure = || Err(Error::unavailable("enumerate volumes", None));
        let event = tracker.poll(failure()).unwrap();
        assert!(matches!(event, DeviceEvent::PollFailed { .. }));
        assert!(state.handle_event(event).is_err());
        assert!(state.connected.is_none());
        assert!(tracker.select(&a.id).is_none());
        assert!(tracker.poll(failure()).is_none());
        let event = tracker.poll(report(vec![a.clone()])).unwrap();
        assert!(changes(&event).is_empty());
        assert_eq!(event.selected_device(), Some(&a));
        assert!(tracker.poll(report(vec![a])).is_none());
    }

    #[test]
    fn recovery_before_first_success_emits_once_and_warnings_are_transported() {
        let mut tracker = DeviceTracker::default();
        tracker.poll(Err(Error::unavailable("poll", None)));
        assert!(tracker.poll(report(vec![])).is_some());
        assert!(tracker.poll(report(vec![])).is_none());
        let event = tracker
            .poll(Ok(DiscoveryReport {
                devices: vec![],
                warnings: vec![Error::unavailable("inspect volume", None)],
            }))
            .unwrap();
        assert!(event.message().is_err());
        assert!(tracker.poll(report(vec![])).unwrap().message().is_ok());
    }

    #[test]
    fn stop_interrupts_long_delay_and_joins_worker_idempotently() {
        let (tx, rx) = mpsc::channel();
        let mut watcher = DeviceWatcher::start_with(
            Duration::from_secs(3600),
            || report(vec![]),
            move |event| tx.send(event).is_ok(),
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = std::time::Instant::now();
        watcher.stop().unwrap();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        watcher.stop().unwrap();
    }

    #[test]
    fn drop_joins_and_closed_consumer_stops_worker() {
        let (tx, rx) = mpsc::channel();
        let watcher = DeviceWatcher::start_with(
            Duration::from_secs(3600),
            || report(vec![]),
            move |event| tx.send(event).is_ok(),
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(watcher);
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        let mut watcher =
            DeviceWatcher::start_with(Duration::ZERO, || report(vec![]), |_| false).unwrap();
        watcher.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn stop_during_inflight_poll_waits_for_query_and_suppresses_delivery() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        let mut watcher = DeviceWatcher::start_with(
            Duration::from_secs(3600),
            move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                report(vec![reader("a", "a-root")])
            },
            move |event| events_tx.send(event).is_ok(),
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        watcher.stop.send(()).unwrap();
        release_tx.send(()).unwrap();
        watcher.stop().unwrap();
        assert!(matches!(
            events_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn polling_and_delivery_run_outside_callers_thread() {
        let caller = thread::current().id();
        let (tx, rx) = mpsc::channel();
        let mut watcher = DeviceWatcher::start_with(
            Duration::from_secs(3600),
            move || {
                assert_ne!(thread::current().id(), caller);
                report(vec![])
            },
            move |_| {
                tx.send(thread::current().id()).unwrap();
                true
            },
        )
        .unwrap();
        assert_ne!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), caller);
        watcher.stop().unwrap();
    }
}
