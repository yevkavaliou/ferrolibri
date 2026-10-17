//! At most one request per host at a time, with a pause between two requests (D4 notes in
//! 05-http-layer.md). Waiting is not an error: a request queues until its turn.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::control::Control;
use super::error::HostError;

/// How often a waiting request looks at cancellation and its deadline.
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(crate) struct HostGate {
    gap: Duration,
    state: Mutex<HashMap<String, Slot>>,
    turn: Condvar,
}

#[derive(Debug, Default)]
struct Slot {
    busy: bool,
    last_end: Option<Instant>,
}

/// The right to talk to one host. Dropping it starts the pause for the next request.
#[derive(Debug)]
pub(crate) struct Permit {
    gate: Arc<HostGate>,
    host: String,
}

impl HostGate {
    pub fn new(gap: Duration) -> Self {
        Self {
            gap,
            state: Mutex::default(),
            turn: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for the host to be free and for the pause after its previous request to pass.
    /// Fails when `control` is cancelled or its deadline passes while waiting.
    pub fn acquire(self: &Arc<Self>, host: &str, control: &Control) -> Result<Permit, HostError> {
        let mut state = self.lock();
        loop {
            control.check()?;
            let now = Instant::now();
            // Forget hosts whose pause is over, so the map does not grow without bound.
            state.retain(|_, slot| {
                slot.busy || slot.last_end.is_some_and(|end| now < end + self.gap)
            });
            let slot = state.entry(host.to_owned()).or_default();
            let wait = if slot.busy {
                POLL
            } else {
                match slot.last_end {
                    Some(end) if now < end + self.gap => (end + self.gap - now).min(POLL),
                    _ => {
                        slot.busy = true;
                        return Ok(Permit {
                            gate: Arc::clone(self),
                            host: host.to_owned(),
                        });
                    }
                }
            };
            state = self
                .turn
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        if let Some(slot) = state.get_mut(&self.host) {
            slot.busy = false;
            slot.last_end = Some(Instant::now());
        }
        drop(state);
        self.gate.turn.notify_all();
    }
}
