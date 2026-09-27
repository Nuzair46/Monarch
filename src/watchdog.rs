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

/// Retry transient display or persistence failures. Exhaustion leaves the manager's
/// pending recovery intact for a manual retry and returns an error to the UI.
pub fn run_confirmation_watchdog(
    mut poll: impl FnMut() -> Result<ConfirmationPoll, String>,
    mut sleep: impl FnMut(Duration),
) -> Result<bool, String> {
    let delays = [
        Duration::from_millis(250),
        Duration::from_secs(1),
        Duration::from_secs(2),
    ];
    let mut failures = 0;
    loop {
        match poll() {
            Ok(ConfirmationPoll::Finished { reverted }) => return Ok(reverted),
            Ok(ConfirmationPoll::Waiting(delay)) => sleep(delay),
            Err(error) => {
                let Some(delay) = delays.get(failures) else {
                    return Err(error);
                };
                failures += 1;
                sleep(*delay);
            }
        }
    }
}
