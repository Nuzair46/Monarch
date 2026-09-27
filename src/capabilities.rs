//! Live capabilities are separate from saved preferences and geometry history.
use crate::{DisplayId, Layout, ManagerError, Resolution, Rotation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisplayMode {
    pub resolution: Resolution,
    pub refresh_rate_mhz: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisplayCapabilities {
    pub display_id: DisplayId,
    pub modes: Vec<DisplayMode>,
    pub modes_unavailable_reason: Option<String>,
    pub hdr_supported: bool,
    pub hdr_enabled: Option<bool>,
    pub hdr_unavailable_reason: Option<String>,
    pub scale_percent: Option<u32>,
    pub scale_percentages: Vec<u32>,
    pub scaling_unavailable_reason: Option<String>,
}

/// Preflight known capabilities. The backend must also validate the complete
/// request with Windows: detached and cloned targets expose only partial mode lists.
pub fn validate(layout: &Layout, capabilities: &[DisplayCapabilities]) -> Result<(), ManagerError> {
    validate_inner(layout, capabilities, None)
}

/// Check a proposed transition without treating a detached monitor's current
/// HDR/DPI availability as its capabilities once attached. The backend must call
/// `validate` again with fresh capabilities after activating the requested paths,
/// within its recovery transaction and before applying HDR/scaling preferences.
pub fn validate_transition(
    layout: &Layout,
    current: &Layout,
    capabilities: &[DisplayCapabilities],
) -> Result<(), ManagerError> {
    validate_inner(layout, capabilities, Some(current))
}

fn validate_inner(
    layout: &Layout,
    capabilities: &[DisplayCapabilities],
    current: Option<&Layout>,
) -> Result<(), ManagerError> {
    layout.ensure_supported()?;
    for output in layout.outputs.iter().filter(|o| o.enabled) {
        let cap = capabilities
            .iter()
            .find(|c| c.display_id == output.display_id)
            .ok_or_else(|| {
                ManagerError::Validation(format!(
                    "display {} is unavailable; reconnect it and refresh",
                    output.display_id.target_id
                ))
            })?;
        let fail = |why: &str| {
            ManagerError::Validation(format!("display {}: {why}", output.display_id.target_id))
        };
        let mut resolution = output.resolution.clone();
        if matches!(
            output.rotation,
            Some(Rotation::Portrait | Rotation::PortraitFlipped)
        ) {
            std::mem::swap(&mut resolution.width, &mut resolution.height);
        }
        // A missing entry in a partial list is unknown, not unsupported. Leave
        // the exact request intact for native validation; never invent a mode
        // from history or silently substitute the target's preferred mode.
        if resolution.width > 0
            && cap.modes_unavailable_reason.is_none()
            && !cap.modes.iter().any(|mode| {
                mode.resolution == resolution
                    && mode.refresh_rate_mhz.abs_diff(output.refresh_rate_mhz) <= 2
            })
        {
            return Err(fail("resolution/refresh combination is unavailable; attach the display to enumerate modes or select a reported mode"));
        }
        // Only a live, explicitly detached target may defer these checks. A
        // missing target or an incomplete mode list alone is not sufficient.
        if current.is_some_and(|current| {
            current
                .outputs
                .iter()
                .any(|o| o.display_id == output.display_id && !o.enabled)
        }) {
            continue;
        }
        if output.hdr_enabled.is_some()
            && (cap.hdr_enabled.is_none()
                || (!cap.hdr_supported && output.hdr_enabled != cap.hdr_enabled))
        {
            return Err(fail(
                "HDR cannot be controlled in this configuration; choose Preserve HDR",
            ));
        }
        if let Some(scale) = output.scale_percent {
            if cap.scale_percent.is_none() || !cap.scale_percentages.contains(&scale) {
                return Err(fail("scaling is unavailable or outside the supported range; choose Preserve scaling"));
            }
        }
    }
    Ok(())
}
