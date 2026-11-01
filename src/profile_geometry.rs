//! Repair an old profile's target-sized portrait rectangle using live mode evidence.
use crate::{capabilities::DisplayCapabilities, Layout, ManagerError, OutputConfig, Rotation};

fn candidate(output: &OutputConfig) -> bool {
    output.enabled
        && output.clone_group.is_none()
        && matches!(
            output.rotation,
            Some(Rotation::Portrait | Rotation::PortraitFlipped)
        )
        && output.resolution.width > output.resolution.height
        && output.resolution.height > 0
}

pub fn needs_mode_evidence(layout: &Layout) -> bool {
    layout.outputs.iter().any(candidate)
}

/// Only saved profiles use this compatibility repair. Explicit layout edits and
/// recovery remain exact. A partial list cannot prove that a mode is unsupported.
pub fn repair(
    saved: &Layout,
    capabilities: &[DisplayCapabilities],
) -> Result<Layout, ManagerError> {
    let mut result = saved.clone();
    let mut changed = false;
    for output in result.outputs.iter_mut().filter(|o| candidate(o)) {
        let Some(cap) = capabilities.iter().find(|cap| {
            cap.display_id == output.display_id && cap.modes_unavailable_reason.is_none()
        }) else {
            continue;
        };
        let supports = |width, height| {
            cap.modes.iter().any(|mode| {
                mode.resolution.width == width
                    && mode.resolution.height == height
                    && mode.refresh_rate_mhz.abs_diff(output.refresh_rate_mhz) <= 2
            })
        };
        // Modes are in the target's unrotated orientation. A valid portrait
        // source must have a matching transposed target mode. Leave it alone if
        // that mode exists, including native portrait panels and custom modes.
        let width = output.resolution.width;
        let height = output.resolution.height;
        if !supports(height, width) && supports(width, height) {
            output.resolution.width = height;
            output.resolution.height = width;
            changed = true;
        }
    }
    if changed {
        // Resizing a monitor on the left can leave a gap. Keep connected groups
        // together and still reject overlaps instead of guessing a new layout.
        crate::placement::close_missing_display_gaps(&mut result)?;
    }
    Ok(result)
}
