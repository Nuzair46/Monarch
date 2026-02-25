#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::mem::size_of;

use monarch::{DisplayInfo, Layout, ManagerError, Position, Resolution};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
    DISPLAYCONFIG_MODE_INFO_TYPE_TARGET, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    QDC_ONLY_ACTIVE_PATHS, QUERY_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

use super::win32_types::{
    luid_to_u64, make_display_id, output_from_display, RawTopologySnapshot, TopologySnapshot,
};

const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 0x0000_0001;
// Query only active paths. The backend cache preserves the richer prior snapshot when needed
// so we can still re-attach a recently detached display without feeding QDC_ALL_PATHS output
// directly into SetDisplayConfig (which can produce invalid path sets for our simple toggler).
const QUERY_FLAGS: QUERY_DISPLAY_CONFIG_FLAGS = QDC_ONLY_ACTIVE_PATHS;

pub fn query_active_topology() -> Result<TopologySnapshot, ManagerError> {
    let (paths, modes) = query_raw_active()?;
    let raw = RawTopologySnapshot {
        paths: paths.clone(),
        modes: modes.clone(),
    };

    let mut displays = Vec::<DisplayInfo>::new();
    let mut outputs = Vec::new();

    let mode_map = modes_by_key(&modes);

    for path in &paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG == 0 {
            continue;
        }

        let adapter_luid = luid_to_u64(
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
        );
        let display_id = make_display_id(adapter_luid, path.targetInfo.id);
        let friendly_name = target_name(path)
            .unwrap_or_else(|_| format!("Display {}:{}", adapter_luid, path.targetInfo.id));

        let source_key = (
            path.sourceInfo.adapterId.HighPart,
            path.sourceInfo.adapterId.LowPart,
            path.sourceInfo.id,
            DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE.0 as u32,
        );
        let target_key = (
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
            path.targetInfo.id,
            DISPLAYCONFIG_MODE_INFO_TYPE_TARGET.0 as u32,
        );

        let (position, resolution) = mode_map
            .get(&source_key)
            .map(source_mode_position_and_resolution)
            .transpose()?
            .unwrap_or((
                Position { x: 0, y: 0 },
                Resolution {
                    width: 0,
                    height: 0,
                },
            ));

        let refresh_rate_mhz = mode_map
            .get(&target_key)
            .map(target_mode_refresh_mhz)
            .transpose()?
            .unwrap_or(60_000);

        let display = DisplayInfo {
            id: display_id,
            friendly_name,
            is_active: true,
            is_primary: position.x == 0 && position.y == 0,
            resolution: resolution.clone(),
            refresh_rate_mhz,
        };
        outputs.push(output_from_display(&display, position));
        displays.push(display);
    }

    if !outputs.iter().any(|o| o.primary && o.enabled) {
        if let Some(first) = outputs.iter_mut().find(|o| o.enabled) {
            first.primary = true;
        }
        if let Some(first_display) = displays.iter_mut().find(|d| d.is_active) {
            first_display.is_primary = true;
        }
    }

    Ok(TopologySnapshot {
        raw,
        layout: Layout { outputs },
        displays,
    })
}

fn query_raw_active(
) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), ManagerError> {
    unsafe {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;

        let mut status = GetDisplayConfigBufferSizes(QUERY_FLAGS, &mut path_count, &mut mode_count);
        if status.0 != 0 {
            return Err(ManagerError::Backend(format!(
                "GetDisplayConfigBufferSizes failed: {}",
                status.0
            )));
        }

        loop {
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];

            let mut out_paths = path_count;
            let mut out_modes = mode_count;

            status = QueryDisplayConfig(
                QUERY_FLAGS,
                &mut out_paths,
                paths.as_mut_ptr(),
                &mut out_modes,
                modes.as_mut_ptr(),
                None,
            );

            if status == ERROR_INSUFFICIENT_BUFFER {
                let retry =
                    GetDisplayConfigBufferSizes(QUERY_FLAGS, &mut path_count, &mut mode_count);
                if retry.0 != 0 {
                    return Err(ManagerError::Backend(format!(
                        "GetDisplayConfigBufferSizes retry failed: {}",
                        retry.0
                    )));
                }
                continue;
            }

            if status.0 != 0 {
                return Err(ManagerError::Backend(format!(
                    "QueryDisplayConfig failed: {}",
                    status.0
                )));
            }

            paths.truncate(out_paths as usize);
            modes.truncate(out_modes as usize);
            return Ok((paths, modes));
        }
    }
}

fn target_name(path: &DISPLAYCONFIG_PATH_INFO) -> Result<String, ManagerError> {
    unsafe {
        let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        name.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        };

        let status = DisplayConfigGetDeviceInfo(&mut name.header);
        if status != 0 {
            return Err(ManagerError::Backend(format!(
                "DisplayConfigGetDeviceInfo failed: {}",
                status
            )));
        }

        let wide = &name.monitorFriendlyDeviceName;
        Ok(wide_array_to_string(wide))
    }
}

fn wide_array_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|ch| *ch == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

fn modes_by_key(
    modes: &[DISPLAYCONFIG_MODE_INFO],
) -> HashMap<(i32, u32, u32, u32), DISPLAYCONFIG_MODE_INFO> {
    let mut map = HashMap::with_capacity(modes.len());
    for mode in modes.iter().cloned() {
        map.insert(
            (
                mode.adapterId.HighPart,
                mode.adapterId.LowPart,
                mode.id,
                mode.infoType.0 as u32,
            ),
            mode,
        );
    }
    map
}

fn source_mode_position_and_resolution(
    mode: &DISPLAYCONFIG_MODE_INFO,
) -> Result<(Position, Resolution), ManagerError> {
    unsafe {
        let source = mode.Anonymous.sourceMode;
        Ok((
            Position {
                x: source.position.x,
                y: source.position.y,
            },
            Resolution {
                width: source.width,
                height: source.height,
            },
        ))
    }
}

fn target_mode_refresh_mhz(mode: &DISPLAYCONFIG_MODE_INFO) -> Result<u32, ManagerError> {
    unsafe {
        let target = mode.Anonymous.targetMode;
        let numerator = target.targetVideoSignalInfo.vSyncFreq.Numerator;
        let denominator = target.targetVideoSignalInfo.vSyncFreq.Denominator.max(1);
        Ok(((numerator as u64 * 1000) / denominator as u64) as u32)
    }
}
