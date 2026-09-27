#![cfg(target_os = "windows")]

use monarch::{DisplayId, DisplayInfo, Layout};
use windows::Win32::Devices::Display::{DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO};

#[derive(Clone)]
pub struct RawTopologySnapshot {
    pub paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

/// A path for a connected-but-inactive target, harvested from QDC_ALL_PATHS. Kept OUT of
/// `RawTopologySnapshot::paths` on purpose: `apply_layout_against_snapshot` feeds `raw.paths`
/// straight to SetDisplayConfig, and these paths are candidates, not current configuration.
/// ALL_PATHS reports one entry per (source, target) combination; every combination is kept
/// because the source is only chosen at attach time.
#[derive(Clone)]
pub struct AttachablePath {
    pub path: DISPLAYCONFIG_PATH_INFO,
    pub adapter_luid: u64,
    pub target_id: u32,
}

#[derive(Clone)]
pub struct TopologySnapshot {
    pub raw: RawTopologySnapshot,
    pub layout: Layout,
    pub displays: Vec<DisplayInfo>,
    /// Attach candidates for connected-but-detached displays; empty for snapshots rebuilt from
    /// persisted/raw data, which carry no ALL_PATHS enumeration.
    pub attachable: Vec<AttachablePath>,
}

pub fn luid_to_u64(high_part: i32, low_part: u32) -> u64 {
    ((high_part as i64 as u64) << 32) | (low_part as u64)
}

pub fn make_display_id(adapter_luid: u64, target_id: u32, edid_hash: Option<u64>) -> DisplayId {
    DisplayId {
        adapter_luid,
        target_id,
        edid_hash,
    }
}
