#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::hash::Hasher;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};

use crate::diagnostics;
use monarch::{DisplayInfo, Layout, ManagerError, OutputConfig, Position, Resolution};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
    DISPLAYCONFIG_MODE_INFO_TYPE_TARGET, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_ROTATION,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DISPLAYCONFIG_TOPOLOGY_ID, QDC_ALL_PATHS,
    QDC_ONLY_ACTIVE_PATHS, QUERY_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

use super::win32_types::{luid_to_u64, make_display_id, RawTopologySnapshot, TopologySnapshot};

const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 0x0000_0001;

/// Per-enumeration observability counters. Logged (prefix "enum:") only when the resulting
/// summary changes, because the worker polls periodically.
#[derive(Default)]
struct EnumerationStats {
    active_paths: usize,
    seeded: Vec<String>,
    discarded: Vec<String>,
}

pub fn query_active_topology() -> Result<TopologySnapshot, ManagerError> {
    query_inventory(false)
}

pub(super) fn query_connected_topology() -> Result<TopologySnapshot, ManagerError> {
    query_inventory(true)
}

fn query_inventory(include_routes: bool) -> Result<TopologySnapshot, ManagerError> {
    let (paths, modes) = query_raw_with_flags(QDC_ALL_PATHS, false)?;
    let active = paths
        .iter()
        .filter(|p| p.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG != 0)
        .copied()
        .collect::<Vec<_>>();
    let mut stats = EnumerationStats {
        active_paths: active.len(),
        ..Default::default()
    };
    let mut snapshot = snapshot_from_raw(RawTopologySnapshot {
        paths: active,
        modes: modes.clone(),
    })?;
    seed_connected_inactive_displays(&mut snapshot, &mut stats, &paths, &modes);
    if include_routes {
        snapshot.raw = RawTopologySnapshot { paths, modes };
    }
    log_enumeration_if_changed(&stats);
    Ok(snapshot)
}

fn log_enumeration_if_changed(stats: &EnumerationStats) {
    let line = format!(
        "enum:active={}:seeded=[{}]:discarded=[{}]",
        stats.active_paths,
        stats.seeded.join(", "),
        stats.discarded.join(", ")
    );

    static LAST: OnceLock<Mutex<String>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(String::new()));
    let Ok(mut last) = last.lock() else {
        return;
    };
    if *last != line {
        *last = line.clone();
        diagnostics::log(line);
    }
}

/// Add inventory from the same query as the active topology; alternative routes
/// never become evidence that an output is active.
fn seed_connected_inactive_displays(
    snapshot: &mut TopologySnapshot,
    stats: &mut EnumerationStats,
    all_paths: &[DISPLAYCONFIG_PATH_INFO],
    all_modes: &[DISPLAYCONFIG_MODE_INFO],
) {
    let mode_map = modes_by_key(all_modes);

    let mut known_connectors = snapshot
        .displays
        .iter()
        .map(|display| (display.id.adapter_luid, display.id.target_id))
        .collect::<std::collections::HashSet<_>>();
    for path in all_paths {
        let adapter_luid = luid_to_u64(
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
        );
        let connector = (adapter_luid, path.targetInfo.id);
        if known_connectors.contains(&connector) {
            // ALL_PATHS yields one entry per source combination; connectors already represented
            // (active, enriched or seeded by an earlier combination) are the normal case.
            continue;
        }
        // Every branch below decides this connector once; never revisit later combinations.
        known_connectors.insert(connector);

        if !path.targetInfo.targetAvailable.as_bool() {
            stats
                .discarded
                .push(format!("target={}:unavailable", path.targetInfo.id));
            continue;
        }
        let Ok((friendly_name, edid_hash, identity)) = target_name_and_stable_hash(path) else {
            stats
                .discarded
                .push(format!("target={}:name-fail", path.targetInfo.id));
            continue;
        };
        // Best-effort refresh rate: the target mode key is specific to this target, so a hit
        // genuinely belongs to this display.
        let target_key = (
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
            path.targetInfo.id,
            DISPLAYCONFIG_MODE_INFO_TYPE_TARGET.0 as u32,
        );
        let refresh_rate_mhz = mode_map
            .get(&target_key)
            .and_then(|mode| target_mode_refresh_mhz(mode).ok())
            .unwrap_or(60_000);
        // Resolution/position are deliberately a 0x0 sentinel: QDC_ALL_PATHS only carries modes
        // for ACTIVE paths, so a source-mode lookup here would alias another display's geometry
        // (the source id of an inactive path points at a source that belongs to whoever is
        // currently driving it). Downstream, the cache merge restores the last real geometry and
        // the attach recovery fills it from the post-extend snapshot.
        let resolution = Resolution {
            width: 0,
            height: 0,
        };

        let mut display_id = make_display_id(adapter_luid, path.targetInfo.id, edid_hash);
        display_id.identity = identity;
        stats
            .seeded
            .push(format!("'{friendly_name}':{}", path.targetInfo.id));
        snapshot.layout.outputs.push(OutputConfig {
            display_id: display_id.clone(),
            enabled: false,
            position: Position { x: 0, y: 0 },
            resolution: resolution.clone(),
            refresh_rate_mhz,
            primary: false,
            rotation: None,
            hdr_enabled: None,
            scale_percent: None,
            clone_group: None,
        });
        snapshot.displays.push(DisplayInfo {
            id: display_id,
            friendly_name,
            is_active: false,
            is_primary: false,
            resolution,
            refresh_rate_mhz,
        });
    }
}

/// detach-only applies so database-sourced paths are never fed back into SetDisplayConfig.
pub(super) fn query_active_only_topology() -> Result<TopologySnapshot, ManagerError> {
    let (paths, modes) = query_raw_active()?;
    snapshot_from_raw(RawTopologySnapshot { paths, modes })
}

pub(super) fn snapshot_from_raw(
    raw: RawTopologySnapshot,
) -> Result<TopologySnapshot, ManagerError> {
    let mut displays = Vec::<DisplayInfo>::new();
    let mut outputs = Vec::new();
    let mode_map = modes_by_key(&raw.modes);

    for path in &raw.paths {
        let is_active = path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG != 0;

        let adapter_luid = luid_to_u64(
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
        );
        let (friendly_name, stable_edid_hash, identity) = match target_name_and_stable_hash(path) {
            Ok(value) => value,
            Err(_) if is_active => (
                format!("Display {}:{}", adapter_luid, path.targetInfo.id),
                None,
                Default::default(),
            ),
            Err(_) => continue,
        };
        let mut display_id = make_display_id(adapter_luid, path.targetInfo.id, stable_edid_hash);
        display_id.identity = identity;

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

        let (position, source_resolution) = mode_map
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
            is_active,
            is_primary: is_active && position.x == 0 && position.y == 0,
            resolution: source_resolution.clone(),
            refresh_rate_mhz,
        };
        outputs.push(OutputConfig {
            display_id: display.id.clone(),
            enabled: is_active,
            position,
            resolution: source_resolution,
            refresh_rate_mhz: display.refresh_rate_mhz,
            primary: display.is_primary,
            rotation: Some(rotation_from_windows(path.targetInfo.rotation)),
            hdr_enabled: super::hdr::query(path).map(|h| h.enabled),
            scale_percent: super::scaling::query(path).map(|s| s.current),
            clone_group: None,
        });
        displays.push(display);
    }

    // Group only observed active paths that share a Windows source. Persist a
    // local ordinal, never an adapter/source address; verification compares members.
    let mut source_groups: std::collections::BTreeMap<(u64, u32), Vec<usize>> =
        std::collections::BTreeMap::new();
    for path in raw.paths.iter().filter(|p| p.flags & 1 != 0) {
        if let Some(index) = outputs.iter().position(|o| {
            o.display_id.target_id == path.targetInfo.id
                && o.display_id.adapter_luid
                    == luid_to_u64(
                        path.targetInfo.adapterId.HighPart,
                        path.targetInfo.adapterId.LowPart,
                    )
        }) {
            source_groups
                .entry((
                    luid_to_u64(
                        path.sourceInfo.adapterId.HighPart,
                        path.sourceInfo.adapterId.LowPart,
                    ),
                    path.sourceInfo.id,
                ))
                .or_default()
                .push(index);
        }
    }
    for (ordinal, members) in source_groups.values().filter(|g| g.len() > 1).enumerate() {
        for index in members {
            outputs[*index].clone_group = Some(format!("clone-{}", ordinal + 1));
        }
    }

    if !outputs
        .iter()
        .any(|output| output.primary && output.enabled)
    {
        if let Some(first) = outputs.iter_mut().find(|output| output.enabled) {
            first.primary = true;
        }
        if let Some(first_display) = displays.iter_mut().find(|display| display.is_active) {
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
    query_raw_with_flags(QDC_ONLY_ACTIVE_PATHS, false)
}

fn query_raw_with_flags(
    query_flags: QUERY_DISPLAY_CONFIG_FLAGS,
    needs_topology_id: bool,
) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), ManagerError> {
    unsafe {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;

        let mut status = GetDisplayConfigBufferSizes(query_flags, &mut path_count, &mut mode_count);
        if status.0 != 0 {
            return Err(ManagerError::Backend(format!(
                "GetDisplayConfigBufferSizes failed: {}",
                status.0
            )));
        }

        for _ in 0..8 {
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];

            let mut out_paths = path_count;
            let mut out_modes = mode_count;
            let mut topology_id = DISPLAYCONFIG_TOPOLOGY_ID(0);

            status = QueryDisplayConfig(
                query_flags,
                &mut out_paths,
                paths.as_mut_ptr(),
                &mut out_modes,
                modes.as_mut_ptr(),
                if needs_topology_id {
                    Some(&mut topology_id)
                } else {
                    None
                },
            );

            if status == ERROR_INSUFFICIENT_BUFFER {
                let retry =
                    GetDisplayConfigBufferSizes(query_flags, &mut path_count, &mut mode_count);
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
        Err(ManagerError::Backend(
            "display topology kept changing during enumeration; try again".into(),
        ))
    }
}

fn target_name_and_stable_hash(
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Result<(String, Option<u64>, monarch::MonitorIdentity), ManagerError> {
    unsafe {
        let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };

        let status = DisplayConfigGetDeviceInfo(&mut name.header);
        if status != 0 {
            return Err(ManagerError::Backend(format!(
                "DisplayConfigGetDeviceInfo failed: {}",
                status
            )));
        }

        let friendly_name = wide_array_to_string(&name.monitorFriendlyDeviceName);
        let device_path = wide_array_to_string(&name.monitorDevicePath);
        let stable_hash =
            (name.flags.Anonymous.value & 4 != 0 && !device_path.is_empty()).then(|| {
                stable_display_hash(
                    name.edidManufactureId,
                    name.edidProductCodeId,
                    name.connectorInstance,
                    &device_path,
                )
            });
        let identity = monarch::MonitorIdentity {
            edid_serial: if device_path.is_empty() {
                None
            } else {
                super::identity::monitor_serial(&device_path)
            },
            device_path: (!device_path.is_empty()).then(|| device_path.to_ascii_uppercase()),
        };
        Ok((friendly_name, stable_hash, identity))
    }
}

fn stable_display_hash(
    edid_manufacture_id: u16,
    edid_product_code_id: u16,
    connector_instance: u32,
    monitor_device_path: &str,
) -> u64 {
    let mut hasher = Fnv1a64::new();
    hasher.update(&edid_manufacture_id.to_le_bytes());
    hasher.update(&edid_product_code_id.to_le_bytes());
    hasher.update(&connector_instance.to_le_bytes());

    // Normalize for case-insensitive path handling in Windows identifiers.
    let normalized_path = monitor_device_path.to_ascii_uppercase();
    hasher.update(normalized_path.as_bytes());
    hasher.finish()
}

struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x0000_0100_0000_01B3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }
}

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.update(bytes);
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

fn rotation_from_windows(rotation: DISPLAYCONFIG_ROTATION) -> monarch::Rotation {
    match rotation.0 {
        2 => monarch::Rotation::Portrait,
        3 => monarch::Rotation::LandscapeFlipped,
        4 => monarch::Rotation::PortraitFlipped,
        _ => monarch::Rotation::Landscape,
    }
}
