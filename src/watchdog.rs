use std::time::{Duration, Instant};

use crate::{ConfigStore, DisplayBackend, MonarchDisplayManager};

pub enum ConfirmationPoll {
    Waiting(Duration),
    Finished { reverted: bool },
}

/// A watchdog belongs to one pending change. An older watcher must not revert a
/// later change that was started while it was asleep or retrying.
pub fn poll_confirmation<B: DisplayBackend, S: ConfigStore>(
    manager: &mut MonarchDisplayManager<B, S>,
    started_at: Instant,
) -> Result<ConfirmationPoll, String> {
    if manager.pending_confirmation_started_at() != Some(started_at) {
        return Ok(ConfirmationPoll::Finished { reverted: false });
    }
    if manager
        .rollback_if_confirmation_expired()
        .map_err(|error| error.to_string())?
    {
        return Ok(ConfirmationPoll::Finished { reverted: true });
    }
    Ok(ConfirmationPoll::Waiting(
        manager
            .pending_confirmation_remaining()
            .unwrap_or_default()
            .max(Duration::from_millis(1)),
    ))
}

/// A nonblocking retry schedule, owned by the display worker for one transaction.
/// Exhaustion leaves durable recovery intact and reports the error once.
pub struct ConfirmationWatchdog {
    next_attempt: Instant,
    failures: usize,
    finished: bool,
}

impl ConfirmationWatchdog {
    pub fn new(now: Instant) -> Self {
        Self {
            next_attempt: now,
            failures: 0,
            finished: false,
        }
    }

    pub fn poll(
        &mut self,
        now: Instant,
        poll: impl FnOnce() -> Result<ConfirmationPoll, String>,
    ) -> Result<ConfirmationPoll, String> {
        if self.finished {
            return Ok(ConfirmationPoll::Finished { reverted: false });
        }
        if now < self.next_attempt {
            return Ok(ConfirmationPoll::Waiting(self.next_attempt - now));
        }
        match poll() {
            Ok(result) => {
                if let ConfirmationPoll::Waiting(delay) = result {
                    self.next_attempt = now + delay;
                } else {
                    self.finished = true;
                }
                Ok(result)
            }
            Err(error) => {
                let delays = [
                    Duration::from_millis(250),
                    Duration::from_secs(1),
                    Duration::from_secs(2),
                ];
                if let Some(delay) = delays.get(self.failures) {
                    self.failures += 1;
                    self.next_attempt = now.max(Instant::now()) + *delay;
                    Ok(ConfirmationPoll::Waiting(*delay))
                } else {
                    self.finished = true;
                    Err(error)
                }
            }
        }
    }
}

// Drive the same nonblocking production schedule with a simulated clock in core tests.
#[cfg(test)]
pub(crate) fn run_confirmation_watchdog(
    mut poll: impl FnMut() -> Result<ConfirmationPoll, String>,
    mut sleep: impl FnMut(Duration),
) -> Result<bool, String> {
    let mut now = Instant::now();
    let mut watchdog = ConfirmationWatchdog::new(now);
    loop {
        match watchdog.poll(now, &mut poll)? {
            ConfirmationPoll::Finished { reverted } => return Ok(reverted),
            ConfirmationPoll::Waiting(delay) => {
                sleep(delay);
                now = now.max(Instant::now()) + delay;
            }
        }
    }
}
