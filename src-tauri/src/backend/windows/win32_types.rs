#![cfg(target_os = "windows")]

use monarch::{DisplayId, DisplayInfo, Layout, OutputConfig, Position, Resolution};
use windows::Win32::Devices::Display::{DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO};

#[derive(Clone)]
pub struct RawTopologySnapshot {
    pub paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

#[derive(Clone)]
pub struct TopologySnapshot {
    pub raw: RawTopologySnapshot,
    pub layout: Layout,
    pub displays: Vec<DisplayInfo>,
}

pub fn luid_to_u64(high_part: i32, low_part: u32) -> u64 {
    ((high_part as i64 as u64) << 32) | (low_part as u64)
}

pub fn make_display_id(adapter_luid: u64, target_id: u32) -> DisplayId {
    DisplayId {
        adapter_luid,
        target_id,
        edid_hash: None,
    }
}

pub fn output_from_display(display: &DisplayInfo, position: Position) -> OutputConfig {
    OutputConfig {
        display_id: display.id.clone(),
        enabled: display.is_active,
        position,
        resolution: Resolution {
            width: display.resolution.width,
            height: display.resolution.height,
        },
        refresh_rate_mhz: display.refresh_rate_mhz,
        primary: display.is_primary,
    }
}
