use std::time::{Duration, Instant};

use ferrolibri_core::converter::CancellationToken;

use super::error::HostError;
use crate::HostErrorKind;

/// Cancellation and deadline of one operation, checked wherever the code may wait.
#[derive(Clone, Debug)]
pub(crate) struct Control {
    pub cancel: CancellationToken,
    pub deadline: Option<Instant>,
}

impl Control {
    pub fn check(&self) -> Result<(), HostError> {
        if self.cancel.is_cancelled() {
            return Err(HostError::new(
                HostErrorKind::Cancelled,
                "request cancelled",
            ));
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(HostError::new(
                HostErrorKind::Timeout,
                "time limit exceeded",
            ));
        }
        Ok(())
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    /// The same cancellation with the earlier of the two deadlines.
    pub fn bounded_by(&self, deadline: Option<Instant>) -> Self {
        let deadline = match (self.deadline, deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Self {
            cancel: self.cancel.clone(),
            deadline,
        }
    }
}
