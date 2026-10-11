//! Preserve detached outputs across a settled change in connected monitors.
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::identity::{resolve, Resolution};
use crate::{ConfigStore, DisplayBackend, Layout, ManagerError, MonarchDisplayManager};

const SETTLE_TIME: Duration = Duration::from_secs(1);
const MAX_SETTLE_TIME: Duration = Duration::from_secs(5);

struct Change {
    latest: Layout,
    started: Instant,
    changed: Instant,
    connections_changed: bool,
}

/// Owned by the display worker. Observations and deadlines are separate from
/// notifications so repeated broadcasts cannot keep postponing a correction.
#[derive(Default)]
pub struct ConnectionMonitor {
    baseline: Option<Layout>,
    change: Option<Change>,
}

impl ConnectionMonitor {
    /// Explicit user operations and recovery discard any queued correction.
    pub fn reset(&mut self) {
        self.baseline = None;
        self.change = None;
    }

    pub fn is_settling(&self) -> bool {
        self.change.is_some()
    }

    pub fn poll<B: DisplayBackend, S: ConfigStore>(
        &mut self,
        manager: &mut MonarchDisplayManager<B, S>,
        now: Instant,
    ) -> Result<bool, ManagerError> {
        if manager.has_pending_confirmation() {
            self.reset();
            return Ok(false);
        }
        let current = manager.get_layout()?;
        if let Err(error) = current.ensure_supported() {
            self.reset();
            return Err(error);
        }
        let Some(desired) = self.observe(&current, now)? else {
            return Ok(false);
        };
        // Adopt a fresh observation after either success or failure. In particular,
        // our own notifications and a failed correction must not start a retry loop.
        self.reset();
        let result = manager.correct_connection_change(&current, desired);
        if !manager.has_pending_confirmation() {
            self.baseline = manager.get_layout().ok();
        }
        result
    }

    fn observe(&mut self, current: &Layout, now: Instant) -> Result<Option<Layout>, ManagerError> {
        let Some(baseline) = &self.baseline else {
            self.baseline = Some(current.clone());
            return Ok(None);
        };
        if self.change.is_none() && same_layout(baseline, current) {
            return Ok(None);
        }
        let change = self.change.get_or_insert_with(|| Change {
            latest: current.clone(),
            started: now,
            changed: now,
            connections_changed: !same_connections(baseline, current),
        });
        if !same_layout(&change.latest, current) {
            change.latest = current.clone();
            change.changed = now;
            change.connections_changed |= !same_connections(baseline, current);
        }
        if now.duration_since(change.changed) < SETTLE_TIME
            && now.duration_since(change.started) < MAX_SETTLE_TIME
        {
            return Ok(None);
        }
        let previous = self.baseline.replace(current.clone()).unwrap();
        let change = self.change.take().unwrap();
        if change.connections_changed {
            detach_previous_outputs(&previous, current)
        } else {
            // A Windows display edit without a connection change is authoritative.
            Ok(None)
        }
    }
}

pub(crate) fn same_layout(a: &Layout, b: &Layout) -> bool {
    let sorted = |layout: &Layout| {
        let mut outputs = layout.outputs.clone();
        outputs.sort_by(|a, b| a.display_id.cmp(&b.display_id));
        outputs
    };
    sorted(a) == sorted(b)
}

fn same_connections(previous: &Layout, current: &Layout) -> bool {
    if previous.outputs.len() != current.outputs.len() {
        return false;
    }
    let mut matched = HashSet::new();
    previous
        .outputs
        .iter()
        .all(|output| match resolve(&output.display_id, current) {
            Resolution::Resolved(id) => matched.insert(id.endpoint()),
            _ => false,
        })
}

fn detach_previous_outputs(
    previous: &Layout,
    current: &Layout,
) -> Result<Option<Layout>, ManagerError> {
    let mut desired = current.clone();
    let mut changed = false;
    for output in previous.outputs.iter().filter(|output| !output.enabled) {
        let Resolution::Resolved(id) = resolve(&output.display_id, current) else {
            continue;
        };
        // Require a one-to-one match in both directions when monitors share
        // identity evidence, rather than assigning two old outputs to one target.
        if resolve(&id, previous) != Resolution::Resolved(output.display_id.clone()) {
            continue;
        }
        let target = desired
            .outputs
            .iter_mut()
            .find(|o| o.display_id == id)
            .unwrap();
        if target.enabled {
            target.enabled = false;
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    crate::manager::normalize_primary(&mut desired);
    desired.ensure_supported()?;
    Ok(Some(desired))
}
