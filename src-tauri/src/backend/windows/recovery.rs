#![cfg(target_os = "windows")]

use std::collections::HashSet;

use monarch::{Layout, ManagerError};
use windows::Win32::Devices::Display::{
    SetDisplayConfig, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_ROTATION_IDENTITY,
    DISPLAYCONFIG_SCALING_PREFERRED, DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED,
    SDC_ALLOW_CHANGES, SDC_ALLOW_PATH_ORDER_CHANGES, SDC_APPLY, SDC_TOPOLOGY_SUPPLIED,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SDC_VALIDATE,
};

use super::apply::wait_for_requested_outputs;
use super::win32_types::{luid_to_u64, TopologySnapshot};

const PATH_ACTIVE: u32 = 1;
const MODE_INDEX_INVALID: u32 = u32::MAX;
type PathEndpoint = (u64, u32);

/// Reconnect the requested targets using fresh QDC_ALL_PATHS data. The caller remaps the
/// desired IDs against this inventory, verifies the result, and reapplies saved geometry.
pub(super) fn recover_layout(
    desired: &Layout,
    connected: &TopologySnapshot,
) -> Result<TopologySnapshot, ManagerError> {
    let paths = build_recovery_paths(desired, &connected.raw.paths)?;

    // A topology-only request must contain one chosen path per target, no mode table, and
    // invalid source/target mode indices. Try Windows' saved modes for this exact topology.
    // https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setdisplayconfig
    let database_flags = SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES;
    let mut database_status =
        unsafe { SetDisplayConfig(Some(paths.as_slice()), None, SDC_VALIDATE | database_flags) };
    if database_status == 0 {
        database_status =
            unsafe { SetDisplayConfig(Some(paths.as_slice()), None, SDC_APPLY | database_flags) };
    }
    let database_result = if database_status == 0 {
        wait_for_requested_outputs(desired)
    } else {
        Err(ManagerError::Backend(format!(
            "saved topology status {database_status}"
        )))
    };
    match database_result {
        Ok(snapshot) => return Ok(snapshot),
        Err(database_error) => {
            // The database may only know the reduced desktop. With unspecified modes, Windows
            // can compute a working configuration for the explicit complete target set instead.
            let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
            let mut best_mode_status =
                unsafe { SetDisplayConfig(Some(paths.as_slice()), None, SDC_VALIDATE | flags) };
            if best_mode_status == 0 {
                best_mode_status =
                    unsafe { SetDisplayConfig(Some(paths.as_slice()), None, SDC_APPLY | flags) };
            }
            if best_mode_status != 0 {
                return Err(ManagerError::Backend(format!(
                    "display reconnect failed: {database_error}; best-mode status {best_mode_status}"
                )));
            }
        }
    }

    wait_for_requested_outputs(desired)
}

fn build_recovery_paths(
    desired: &Layout,
    available: &[DISPLAYCONFIG_PATH_INFO],
) -> Result<Vec<DISPLAYCONFIG_PATH_INFO>, ManagerError> {
    desired.ensure_valid()?;
    let mut outputs: Vec<_> = desired
        .outputs
        .iter()
        .filter(|output| output.enabled)
        .collect();
    outputs.sort_by_key(|output| {
        (
            !output.primary,
            output.position.y,
            output.position.x,
            output.display_id.adapter_luid,
            output.display_id.target_id,
        )
    });

    let mut targets = HashSet::new();
    let mut candidates = Vec::with_capacity(outputs.len());
    for output in outputs {
        let target = (output.display_id.adapter_luid, output.display_id.target_id);
        if !targets.insert(target) {
            return Err(ManagerError::Validation(
                "layout enables the same Windows display target more than once".to_string(),
            ));
        }
        let mut target_candidates: Vec<_> = available
            .iter()
            .filter(|path| path.targetInfo.targetAvailable.as_bool() && target_key(path) == target)
            .copied()
            .collect();
        // Preserve an existing source assignment whenever a complete assignment allows it.
        target_candidates.sort_by_key(|path| (path.flags & PATH_ACTIVE == 0, source_key(path)));
        if target_candidates.is_empty() {
            return Err(ManagerError::Backend(format!(
                "cannot reconnect display {} on adapter {:016x}: Windows reports no available display path",
                target.1, target.0
            )));
        }
        candidates.push(target_candidates);
    }

    // QDC_ALL_PATHS contains alternative routes, not a ready-to-apply topology. A greedy
    // choice can consume the only source available to a later display. Backtrack to find
    // a complete matching, keeping sources distinct so displays are extended, not cloned.
    let mut selected = Vec::with_capacity(candidates.len());
    if !select_distinct_sources(&candidates, 0, &mut HashSet::new(), &mut selected) {
        return Err(ManagerError::Backend(
            "Windows reports no extended-desktop path assignment for all requested displays"
                .to_string(),
        ));
    }

    for path in &mut selected {
        path.flags |= PATH_ACTIVE;
        path.sourceInfo.Anonymous.modeInfoIdx = MODE_INDEX_INVALID;
        path.targetInfo.Anonymous.modeInfoIdx = MODE_INDEX_INVALID;
        // No target mode is supplied. Zero/zero asks Windows for its optimal refresh rate;
        // Windows requires unspecified scan-line ordering in that case.
        path.targetInfo.refreshRate.Numerator = 0;
        path.targetInfo.refreshRate.Denominator = 0;
        path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED;
        if path.targetInfo.rotation.0 == 0 {
            path.targetInfo.rotation = DISPLAYCONFIG_ROTATION_IDENTITY;
        }
        if path.targetInfo.scaling.0 == 0 {
            path.targetInfo.scaling = DISPLAYCONFIG_SCALING_PREFERRED;
        }
    }
    Ok(selected)
}

fn select_distinct_sources(
    candidates: &[Vec<DISPLAYCONFIG_PATH_INFO>],
    index: usize,
    used_sources: &mut HashSet<PathEndpoint>,
    selected: &mut Vec<DISPLAYCONFIG_PATH_INFO>,
) -> bool {
    if index == candidates.len() {
        return true;
    }
    for path in &candidates[index] {
        let source = source_key(path);
        if !used_sources.insert(source) {
            continue;
        }
        selected.push(*path);
        if select_distinct_sources(candidates, index + 1, used_sources, selected) {
            return true;
        }
        selected.pop();
        used_sources.remove(&source);
    }
    false
}

fn source_key(path: &DISPLAYCONFIG_PATH_INFO) -> PathEndpoint {
    (
        luid_to_u64(
            path.sourceInfo.adapterId.HighPart,
            path.sourceInfo.adapterId.LowPart,
        ),
        path.sourceInfo.id,
    )
}

fn target_key(path: &DISPLAYCONFIG_PATH_INFO) -> PathEndpoint {
    (
        luid_to_u64(
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
        ),
        path.targetInfo.id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use monarch::{DisplayId, OutputConfig, Position, Resolution};
    use windows::core::BOOL;

    fn output(adapter: u64, target: u32, primary: bool) -> OutputConfig {
        OutputConfig {
            display_id: DisplayId {
                adapter_luid: adapter,
                target_id: target,
                edid_hash: Some(target as u64),
                identity: Default::default(),
            },
            enabled: true,
            position: Position {
                x: if primary { 0 } else { 1920 },
                y: 0,
            },
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            refresh_rate_mhz: 60_000,
            primary,
            rotation: None,
        }
    }

    fn path(adapter: u64, source: u32, target: u32, active: bool) -> DISPLAYCONFIG_PATH_INFO {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.adapterId.HighPart = (adapter >> 32) as i32;
        path.sourceInfo.adapterId.LowPart = adapter as u32;
        path.sourceInfo.id = source;
        path.sourceInfo.Anonymous.modeInfoIdx = 7;
        path.targetInfo.adapterId = path.sourceInfo.adapterId;
        path.targetInfo.id = target;
        path.targetInfo.Anonymous.modeInfoIdx = 9;
        path.targetInfo.targetAvailable = BOOL(1);
        path.flags = if active { PATH_ACTIVE } else { 0 };
        path
    }

    #[test]
    fn chooses_one_path_per_target_and_prefers_active_source() {
        let desired = Layout {
            outputs: vec![output(1, 10, true), output(1, 20, false)],
        };
        let candidates = [
            path(1, 0, 10, false),
            path(1, 1, 10, true),
            path(1, 0, 20, false),
            path(1, 1, 20, false),
        ];
        let selected = build_recovery_paths(&desired, &candidates).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(source_key(&selected[0]), (1, 1));
        assert_eq!(source_key(&selected[1]), (1, 0));
    }

    #[test]
    fn reconnects_four_requested_targets_when_only_two_paths_are_active() {
        let desired = Layout {
            outputs: (0..4)
                .map(|target| output(1, target, target == 0))
                .collect(),
        };
        let candidates: Vec<_> = (0..4)
            .flat_map(|target| {
                (0..4).map(move |source| path(1, source, target, target < 2 && source == target))
            })
            .collect();
        let selected = build_recovery_paths(&desired, &candidates).unwrap();
        assert_eq!(selected.len(), 4);
        assert_eq!(
            selected
                .iter()
                .map(source_key)
                .collect::<HashSet<_>>()
                .len(),
            4
        );
        assert_eq!(
            selected
                .iter()
                .map(target_key)
                .collect::<HashSet<_>>()
                .len(),
            4
        );
        assert!(selected.iter().all(|path| path.flags & PATH_ACTIVE != 0));
    }

    #[test]
    fn backtracks_when_preferred_source_blocks_another_target() {
        let desired = Layout {
            outputs: vec![output(1, 10, true), output(1, 20, false)],
        };
        let candidates = [
            path(1, 0, 10, true),
            path(1, 1, 10, false),
            path(1, 0, 20, false),
        ];
        let selected = build_recovery_paths(&desired, &candidates).unwrap();
        assert_eq!(source_key(&selected[0]), (1, 1));
        assert_eq!(source_key(&selected[1]), (1, 0));
    }

    #[test]
    fn source_ids_are_scoped_to_their_adapters() {
        let desired = Layout {
            outputs: vec![output(1, 10, true), output(2, 10, false)],
        };
        let candidates = [path(1, 0, 10, true), path(2, 0, 10, false)];
        assert_eq!(
            build_recovery_paths(&desired, &candidates).unwrap().len(),
            2
        );
    }

    #[test]
    fn rejects_a_target_missing_from_the_fresh_inventory() {
        let desired = Layout {
            outputs: vec![output(1, 10, true), output(1, 20, false)],
        };
        assert!(build_recovery_paths(&desired, &[path(1, 0, 10, true)]).is_err());
    }

    #[test]
    fn rejects_unavailable_targets_even_when_still_marked_active() {
        let desired = Layout {
            outputs: vec![output(1, 10, true)],
        };
        let mut unavailable = path(1, 0, 10, true);
        unavailable.targetInfo.targetAvailable = BOOL(0);
        assert!(build_recovery_paths(&desired, &[unavailable]).is_err());
    }

    #[test]
    fn rejects_topologies_that_would_require_cloning() {
        let desired = Layout {
            outputs: vec![output(1, 10, true), output(1, 20, false)],
        };
        let candidates = [path(1, 0, 10, true), path(1, 0, 20, false)];
        assert!(build_recovery_paths(&desired, &candidates).is_err());
    }

    #[test]
    fn excludes_disabled_and_unrequested_targets() {
        let mut disabled = output(1, 20, false);
        disabled.enabled = false;
        let desired = Layout {
            outputs: vec![output(1, 10, true), disabled],
        };
        let candidates = [
            path(1, 0, 10, true),
            path(1, 1, 20, true),
            path(1, 2, 30, false),
        ];
        let selected = build_recovery_paths(&desired, &candidates).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(target_key(&selected[0]), (1, 10));
    }

    #[test]
    fn rejects_duplicate_windows_targets() {
        let desired = Layout {
            outputs: vec![output(1, 10, true), output(1, 10, false)],
        };
        assert!(build_recovery_paths(&desired, &[path(1, 0, 10, true)]).is_err());
    }

    #[test]
    fn discards_mode_indices_from_inventory_and_marks_paths_active() {
        let desired = Layout {
            outputs: vec![output(1, 10, true)],
        };
        let selected = build_recovery_paths(&desired, &[path(1, 0, 10, false)]).unwrap();
        let selected = &selected[0];
        assert_ne!(selected.flags & PATH_ACTIVE, 0);
        assert_eq!(
            unsafe { selected.sourceInfo.Anonymous.modeInfoIdx },
            MODE_INDEX_INVALID
        );
        assert_eq!(
            unsafe { selected.targetInfo.Anonymous.modeInfoIdx },
            MODE_INDEX_INVALID
        );
        assert_eq!(selected.targetInfo.refreshRate.Numerator, 0);
        assert_eq!(selected.targetInfo.refreshRate.Denominator, 0);
        assert_eq!(
            selected.targetInfo.scanLineOrdering,
            DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED
        );
    }
}
