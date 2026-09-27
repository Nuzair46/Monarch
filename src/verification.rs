use std::collections::BTreeMap;

use crate::{Layout, ManagerError};

/// Compare the observed desktop to the requested layout, allowing only rounding of
/// Windows' rational refresh rates to millihertz (one millihertz at either end).
pub fn verify_applied_layout(desired: &Layout, actual: &Layout) -> Result<(), ManagerError> {
    let active = |layout: &Layout| {
        layout
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .map(|o| {
                (
                    (o.display_id.adapter_luid, o.display_id.target_id),
                    o.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let expected = active(desired);
    let observed = active(actual);
    if expected.len() != desired.enabled_output_count()
        || observed.len() != actual.enabled_output_count()
        || expected.keys().ne(observed.keys())
    {
        return Err(ManagerError::Backend(format!(
            "Windows applied a different active display set: requested {:?}, observed {:?}",
            expected.keys().collect::<Vec<_>>(),
            observed.keys().collect::<Vec<_>>()
        )));
    }
    for (key, wanted) in expected {
        let got = &observed[&key];
        if wanted.position != got.position
            || wanted.resolution != got.resolution
            || wanted.primary != got.primary
            || wanted.refresh_rate_mhz.abs_diff(got.refresh_rate_mhz) > 2
        {
            return Err(ManagerError::Backend(format!(
                "Windows did not apply the requested placement, primary display, resolution or refresh rate for display {} on adapter {:016x}", key.1, key.0
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisplayId, OutputConfig, Position, Resolution};

    fn layout() -> Layout {
        Layout {
            outputs: (1..=3)
                .map(|id| OutputConfig {
                    display_id: DisplayId {
                        adapter_luid: 1,
                        target_id: id,
                        edid_hash: Some(id as u64),
                    },
                    enabled: true,
                    primary: id == 1,
                    position: Position {
                        x: (id as i32 - 1) * 1920,
                        y: 0,
                    },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 119_880,
                })
                .collect(),
        }
    }

    #[test]
    fn detects_partial_wrong_and_unexpected_active_outputs() {
        let desired = layout();
        let mut actual = desired.clone();
        actual.outputs[2].enabled = false;
        assert!(verify_applied_layout(&desired, &actual).is_err());
        assert!(verify_applied_layout(&actual, &desired).is_err());
        actual = desired.clone();
        actual.outputs[2].display_id.adapter_luid = 2;
        assert!(verify_applied_layout(&desired, &actual).is_err());
    }

    #[test]
    fn rejects_each_incorrect_layout_property() {
        let desired = layout();
        for property in 0..4 {
            let mut actual = desired.clone();
            match property {
                0 => actual.outputs[1].position.x += 1,
                1 => actual.outputs[1].resolution.width = 1280,
                2 => actual.outputs[1].primary = true,
                _ => actual.outputs[1].refresh_rate_mhz = 60_000,
            }
            assert!(verify_applied_layout(&desired, &actual).is_err());
        }
    }

    #[test]
    fn accepts_reordered_outputs_and_refresh_rounding() {
        let desired = layout();
        let mut actual = desired.clone();
        actual.outputs.reverse();
        actual.outputs[0].refresh_rate_mhz += 1;
        assert!(verify_applied_layout(&desired, &actual).is_ok());
    }
}
