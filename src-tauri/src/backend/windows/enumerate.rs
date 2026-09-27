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
    DISPLAYCONFIG_ROTATION_ROTATE270, DISPLAYCONFIG_ROTATION_ROTATE90,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DISPLAYCONFIG_TOPOLOGY_ID, QDC_ALL_PATHS,
    QDC_DATABASE_CURRENT, QDC_ONLY_ACTIVE_PATHS, QUERY_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

use super::win32_types::{
    luid_to_u64, make_display_id, AttachablePath, RawTopologySnapshot, TopologySnapshot,
};

const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 0x0000_0001;

/// Per-enumeration observability counters. Logged (prefix "enum:") only when the resulting
/// summary changes, because the watchdogs enumerate every 1.2s/1.8s.
#[derive(Default)]
struct EnumerationStats {
    active_paths: usize,
    db_paths: usize,
    enriched: Vec<String>,
    seeded: Vec<String>,
    discarded: Vec<String>,
}

pub fn query_active_topology() -> Result<TopologySnapshot, ManagerError> {
    let mut stats = EnumerationStats::default();
    let (active_paths, active_modes) = query_raw_active()?;
    stats.active_paths = active_paths.len();
    let db_raw = query_raw_database_current().ok();
    stats.db_paths = db_raw.as_ref().map(|(paths, _)| paths.len()).unwrap_or(0);
    let (paths, modes) =
        enrich_with_missing_target_paths(active_paths, active_modes, db_raw, &mut stats);
    let mut snapshot = snapshot_from_raw(RawTopologySnapshot { paths, modes })?;
    seed_connected_inactive_displays(&mut snapshot, &mut stats);
    log_enumeration_if_changed(&stats);
    Ok(snapshot)
}

fn log_enumeration_if_changed(stats: &EnumerationStats) {
    let line = format!(
        "enum:active={}:db={}:enriched=[{}]:seeded=[{}]:discarded=[{}]",
        stats.active_paths,
        stats.db_paths,
        stats.enriched.join(", "),
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

/// Make connected-but-inactive displays visible even when QDC_DATABASE_CURRENT enrichment did
/// not surface their paths (seen in the field: a detached TV visible in Windows Display settings
/// but absent from the database query). QDC_ALL_PATHS lists every source combination for every
/// connected target; each yet-unrepresented connected target is added as display info, and its
/// paths are kept in `snapshot.attachable` so the recovery can activate it explicitly.
/// Those paths are deliberately NOT added to `snapshot.raw`, which is the current configuration
/// and goes straight to SetDisplayConfig on every apply.
fn seed_connected_inactive_displays(snapshot: &mut TopologySnapshot, stats: &mut EnumerationStats) {
    let Ok((all_paths, all_modes)) = query_raw_with_flags(QDC_ALL_PATHS, false) else {
        return;
    };
    let mode_map = modes_by_key(&all_modes);

    let mut known_connectors = snapshot
        .displays
        .iter()
        .map(|display| (display.id.adapter_luid, display.id.target_id))
        .collect::<std::collections::HashSet<_>>();
    let mut known_edids = snapshot
        .displays
        .iter()
        .filter_map(|display| display.id.edid_hash)
        .collect::<std::collections::HashSet<_>>();
    let mut seeded_connectors = std::collections::HashSet::new();

    for path in &all_paths {
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
        let Ok((friendly_name, edid_hash)) = target_name_and_stable_hash(path) else {
            stats
                .discarded
                .push(format!("target={}:name-fail", path.targetInfo.id));
            continue;
        };
        if let Some(hash) = edid_hash {
            if known_edids.contains(&hash) {
                stats
                    .discarded
                    .push(format!("target={}:dedupe", path.targetInfo.id));
                continue;
            }
            known_edids.insert(hash);
        }

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

        let display_id = make_display_id(adapter_luid, path.targetInfo.id, edid_hash);
        stats
            .seeded
            .push(format!("'{friendly_name}':{}", path.targetInfo.id));
        seeded_connectors.insert(connector);
        snapshot.layout.outputs.push(OutputConfig {
            display_id: display_id.clone(),
            enabled: false,
            position: Position { x: 0, y: 0 },
            resolution: resolution.clone(),
            refresh_rate_mhz,
            primary: false,
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

    // Keep every (source, target) combination for the seeded targets: the source is only picked
    // at attach time, and it must be one that is currently free (a busy source would clone).
    for path in &all_paths {
        let adapter_luid = luid_to_u64(
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
        );
        let connector = (adapter_luid, path.targetInfo.id);
        if !seeded_connectors.contains(&connector) {
            continue;
        }
        snapshot.attachable.push(AttachablePath {
            path: *path,
            adapter_luid,
            target_id: path.targetInfo.id,
        });
    }
}

/// Active-only snapshot without QDC_DATABASE_CURRENT enrichment. Used as the base for
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
        let (friendly_name, stable_edid_hash) = match target_name_and_stable_hash(path) {
            Ok(value) => value,
            Err(_) if is_active => (
                format!("Display {}:{}", adapter_luid, path.targetInfo.id),
                None,
            ),
            Err(_) => continue,
        };
        let display_id = make_display_id(adapter_luid, path.targetInfo.id, stable_edid_hash);

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

        let display_resolution =
            effective_resolution_for_rotation(source_resolution.clone(), path.targetInfo.rotation);

        let display = DisplayInfo {
            id: display_id,
            friendly_name,
            is_active,
            is_primary: is_active && position.x == 0 && position.y == 0,
            resolution: display_resolution,
            refresh_rate_mhz,
        };
        outputs.push(OutputConfig {
            display_id: display.id.clone(),
            enabled: is_active,
            position,
            resolution: source_resolution,
            refresh_rate_mhz: display.refresh_rate_mhz,
            primary: display.is_primary,
        });
        displays.push(display);
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
        attachable: Vec::new(),
    })
}

fn enrich_with_missing_target_paths(
    mut base_paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    mut base_modes: Vec<DISPLAYCONFIG_MODE_INFO>,
    candidate_raw: Option<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>)>,
    stats: &mut EnumerationStats,
) -> (Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>) {
    let Some((candidate_paths, candidate_modes)) = candidate_raw else {
        return (base_paths, base_modes);
    };

    let mut known_targets = base_paths
        .iter()
        .map(path_target_identity)
        .collect::<std::collections::HashSet<_>>();
    let mut mode_index = base_modes
        .iter()
        .enumerate()
        .map(|(idx, mode)| (mode_identity(mode), idx as u32))
        .collect::<HashMap<_, _>>();

    for candidate in candidate_paths {
        let target_identity = path_target_identity(&candidate);
        if known_targets.contains(&target_identity) {
            // Normal case: every active target also shows up in the database query.
            continue;
        }
        let Ok((candidate_name, _)) = target_name_and_stable_hash(&candidate) else {
            stats
                .discarded
                .push(format!("target={}:name-fail", candidate.targetInfo.id));
            continue;
        };
        if !candidate_path_is_attachable(&candidate, &candidate_modes) {
            stats
                .discarded
                .push(format!("target={}:not-attachable", candidate.targetInfo.id));
            continue;
        }
        stats
            .enriched
            .push(format!("'{candidate_name}':{}", candidate.targetInfo.id));

        let mut next_path = candidate;
        // A database path missing from the live query is inventory, not an active display.
        next_path.flags &= !DISPLAYCONFIG_PATH_ACTIVE_FLAG;
        unsafe {
            let source_idx = next_path.sourceInfo.Anonymous.modeInfoIdx;
            let remapped_source_idx = remap_mode_index(
                source_idx,
                &candidate_modes,
                &mut base_modes,
                &mut mode_index,
            );
            next_path.sourceInfo.Anonymous.modeInfoIdx = remapped_source_idx;
        }
        unsafe {
            let target_idx = next_path.targetInfo.Anonymous.modeInfoIdx;
            let remapped_target_idx = remap_mode_index(
                target_idx,
                &candidate_modes,
                &mut base_modes,
                &mut mode_index,
            );
            next_path.targetInfo.Anonymous.modeInfoIdx = remapped_target_idx;
        }

        base_paths.push(next_path);
        known_targets.insert(target_identity);
    }

    (base_paths, base_modes)
}

fn candidate_path_is_attachable(
    candidate: &DISPLAYCONFIG_PATH_INFO,
    candidate_modes: &[DISPLAYCONFIG_MODE_INFO],
) -> bool {
    let source_idx = unsafe { candidate.sourceInfo.Anonymous.modeInfoIdx };
    let Some(source_mode) = candidate_modes.get(source_idx as usize) else {
        return false;
    };
    if source_mode.infoType.0 != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE.0 {
        return false;
    }
    let Ok((_, resolution)) = source_mode_position_and_resolution(source_mode) else {
        return false;
    };
    if resolution.width == 0 || resolution.height == 0 {
        return false;
    }

    let target_idx = unsafe { candidate.targetInfo.Anonymous.modeInfoIdx };
    if target_idx == u32::MAX {
        return true;
    }
    let Some(target_mode) = candidate_modes.get(target_idx as usize) else {
        return false;
    };
    target_mode.infoType.0 == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET.0
}

fn remap_mode_index(
    original_idx: u32,
    source_modes: &[DISPLAYCONFIG_MODE_INFO],
    base_modes: &mut Vec<DISPLAYCONFIG_MODE_INFO>,
    mode_index: &mut HashMap<(i32, u32, u32, u32), u32>,
) -> u32 {
    if original_idx == u32::MAX {
        return u32::MAX;
    }
    let Some(mode) = source_modes.get(original_idx as usize) else {
        return u32::MAX;
    };

    let identity = mode_identity(mode);
    if let Some(existing) = mode_index.get(&identity) {
        return *existing;
    }

    let next_idx = base_modes.len() as u32;
    base_modes.push(mode.clone());
    mode_index.insert(identity, next_idx);
    next_idx
}

fn mode_identity(mode: &DISPLAYCONFIG_MODE_INFO) -> (i32, u32, u32, u32) {
    (
        mode.adapterId.HighPart,
        mode.adapterId.LowPart,
        mode.id,
        mode.infoType.0 as u32,
    )
}

fn path_target_identity(path: &DISPLAYCONFIG_PATH_INFO) -> (i32, u32, u32) {
    (
        path.targetInfo.adapterId.HighPart,
        path.targetInfo.adapterId.LowPart,
        path.targetInfo.id,
    )
}

fn effective_resolution_for_rotation(
    source_resolution: Resolution,
    rotation: DISPLAYCONFIG_ROTATION,
) -> Resolution {
    if rotation == DISPLAYCONFIG_ROTATION_ROTATE90 || rotation == DISPLAYCONFIG_ROTATION_ROTATE270 {
        return Resolution {
            width: source_resolution.height,
            height: source_resolution.width,
        };
    }
    source_resolution
}

fn query_raw_active(
) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), ManagerError> {
    query_raw_with_flags(QDC_ONLY_ACTIVE_PATHS, false)
}

fn query_raw_database_current(
) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), ManagerError> {
    query_raw_with_flags(QDC_DATABASE_CURRENT, true)
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

        loop {
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
    }
}

fn target_name_and_stable_hash(
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Result<(String, Option<u64>), ManagerError> {
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

        let friendly_name = wide_array_to_string(&name.monitorFriendlyDeviceName);
        let device_path = wide_array_to_string(&name.monitorDevicePath);
        let stable_hash = stable_display_hash(
            name.edidManufactureId,
            name.edidProductCodeId,
            name.connectorInstance,
            &device_path,
        );
        Ok((friendly_name, Some(stable_hash)))
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
