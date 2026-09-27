#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use crate::diagnostics;
use monarch::{Layout, ManagerError};
use windows::core::BOOL;
use windows::core::{w, PCWSTR};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, SetDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, SDC_ALLOW_CHANGES, SDC_APPLY, SDC_NO_OPTIMIZATION,
    SDC_PATH_PERSIST_IF_REQUIRED, SDC_SAVE_TO_DATABASE, SDC_TOPOLOGY_EXTEND,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SDC_VALIDATE,
};
use windows::Win32::Graphics::Gdi::{CreateDCW, DeleteDC};
use windows::Win32::UI::ColorSystem::{
    GetDeviceGammaRamp, SetDeviceGammaRamp, WcsGetCalibrationManagementState,
    WcsSetCalibrationManagementState,
};

use super::win32_types::{luid_to_u64, TopologySnapshot};

const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 0x0000_0001;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const GAMMA_RAMP_WORDS: usize = 3 * 256;
pub(super) type GammaRampKey = (u64, u32);
pub(super) type GammaRampWords = [u16; GAMMA_RAMP_WORDS];

fn enabled_targets(layout: &Layout) -> HashSet<(u64, u32)> {
    layout
        .outputs
        .iter()
        .filter(|o| o.enabled)
        .map(|o| (o.display_id.adapter_luid, o.display_id.target_id))
        .collect()
}

fn ensure_requested_paths(
    desired: &Layout,
    snapshot: &TopologySnapshot,
) -> Result<(), ManagerError> {
    for target in enabled_targets(desired) {
        let count = snapshot
            .raw
            .paths
            .iter()
            .filter(|p| path_target_key(p) == target)
            .count();
        if count != 1 {
            return Err(ManagerError::Backend(format!(
                "display {} has {count} cached paths; fresh connection discovery is required",
                target.1
            )));
        }
    }
    Ok(())
}

pub(super) fn verify_requested_outputs(
    desired: &Layout,
    actual: &Layout,
) -> Result<(), ManagerError> {
    let expected = enabled_targets(desired);
    let observed = enabled_targets(actual);
    if expected != observed {
        let mut missing: Vec<_> = expected
            .difference(&observed)
            .map(|(_, target)| *target)
            .collect();
        let mut unexpected: Vec<_> = observed
            .difference(&expected)
            .map(|(_, target)| *target)
            .collect();
        missing.sort_unstable();
        unexpected.sort_unstable();
        return Err(ManagerError::Backend(format!(
            "Windows did not apply the complete display layout (missing: {missing:?}, unexpected: {unexpected:?})"
        )));
    }
    Ok(())
}

pub(super) fn wait_for_requested_outputs(
    desired: &Layout,
) -> Result<TopologySnapshot, ManagerError> {
    // Some drivers publish their new topology a little after SetDisplayConfig succeeds.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let snapshot = super::enumerate::query_active_only_topology()?;
        match verify_requested_outputs(desired, &snapshot.layout) {
            Ok(()) => return Ok(snapshot),
            Err(error) if std::time::Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

pub fn apply_layout_against_snapshot(
    desired: &Layout,
    snapshot: &TopologySnapshot,
) -> Result<TopologySnapshot, ManagerError> {
    desired.ensure_valid()?;
    ensure_requested_paths(desired, snapshot)?;
    let saved_gamma_ramps = capture_active_gamma_ramps(snapshot);

    let desired_outputs = desired_output_index(desired);
    let mut next_paths: Vec<DISPLAYCONFIG_PATH_INFO> = snapshot.raw.paths.clone();
    let mut next_modes: Vec<DISPLAYCONFIG_MODE_INFO> = snapshot.raw.modes.clone();
    for path in &mut next_paths {
        let key = path_target_key(path);
        let desired_output = desired_outputs.get(&key);
        let enabled = desired_output.map(|output| output.enabled).unwrap_or(false);

        if enabled {
            path.flags |= DISPLAYCONFIG_PATH_ACTIVE_FLAG;
        } else {
            path.flags &= !DISPLAYCONFIG_PATH_ACTIVE_FLAG;
        }

        if enabled {
            if let Some(rotation) = desired_output.and_then(|o| o.rotation) {
                path.targetInfo.rotation =
                    windows::Win32::Devices::Display::DISPLAYCONFIG_ROTATION(match rotation {
                        monarch::Rotation::Landscape => 1,
                        monarch::Rotation::Portrait => 2,
                        monarch::Rotation::LandscapeFlipped => 3,
                        monarch::Rotation::PortraitFlipped => 4,
                    });
            }
            apply_desired_source_mode(path, &mut next_modes, desired_output);
            apply_desired_target_refresh(path, desired_output);
        }
    }
    reorder_paths_for_desired_priority(&mut next_paths, &desired_outputs);

    let mut status = 0;
    for allow_changes in [false, true] {
        let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG
            | if allow_changes {
                SDC_ALLOW_CHANGES
            } else {
                Default::default()
            };
        // APPLY-only flags must not leak into a validation request.
        status =
            unsafe { SetDisplayConfig(Some(&next_paths), Some(&next_modes), SDC_VALIDATE | flags) };
        if status == 0 {
            status = unsafe {
                SetDisplayConfig(
                    Some(&next_paths),
                    Some(&next_modes),
                    SDC_APPLY | flags | SDC_SAVE_TO_DATABASE | SDC_NO_OPTIMIZATION,
                )
            };
        }
        if status == 0 {
            break;
        }
        diagnostics::log(format!(
            "apply:sdc_failed:{status}:allow_changes={allow_changes}"
        ));
    }
    if status != 0 {
        return Err(ManagerError::Backend(format!(
            "SetDisplayConfig failed: {status}"
        )));
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    let next_snapshot = loop {
        let observed = super::enumerate::query_active_only_topology().and_then(|snapshot| {
            monarch::verification::verify_applied_layout(desired, &snapshot.layout)?;
            Ok(snapshot)
        });
        match observed {
            Ok(snapshot) => break snapshot,
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    best_effort_reload_color_calibration();
    best_effort_restore_gamma_ramps(&next_snapshot, &saved_gamma_ramps);
    Ok(next_snapshot)
}

/// Replay the saved extended topology. Validate first; the caller must still observe
/// the requested active outputs, since successful application may be a no-op.
pub(super) fn try_topology_extend() -> i32 {
    let flags = SDC_TOPOLOGY_EXTEND | SDC_PATH_PERSIST_IF_REQUIRED;
    let validation = unsafe { SetDisplayConfig(None, None, SDC_VALIDATE | flags) };
    if validation != 0 {
        diagnostics::log(format!("apply:extend_validation_failed:{validation}"));
        return validation;
    }
    let status = unsafe { SetDisplayConfig(None, None, SDC_APPLY | flags) };
    diagnostics::log(format!("apply:sdc_status:{status}:topology_extend"));
    status
}

/// Drive the same shell path Win+P uses. Escalation of last resort, decided by the caller when
/// the CCD extend did not bring the display back.
pub(super) fn run_display_switch_extend() -> Result<(), ManagerError> {
    let display_switch_child = Command::new("DisplaySwitch.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .arg("/extend")
        .spawn()
        .map_err(|err| {
            ManagerError::Backend(format!("DisplaySwitch /extend launch failed: {err}"))
        })?;

    let Some(display_switch_status) = wait_child_with_timeout(
        display_switch_child,
        "DisplaySwitch.exe",
        Duration::from_secs(10),
    ) else {
        return Err(ManagerError::Backend(
            "DisplaySwitch /extend timed out".to_string(),
        ));
    };

    if !display_switch_status.success() {
        return Err(ManagerError::Backend(format!(
            "DisplaySwitch /extend failed with exit code {:?}",
            display_switch_status.code()
        )));
    }

    Ok(())
}

pub(super) fn reapply_color_calibration_for_active_with_cached_sdr(
    cached_sdr_ramps: &HashMap<GammaRampKey, GammaRampWords>,
) -> Result<(), ManagerError> {
    best_effort_reload_color_calibration();
    let refreshed_snapshot = super::enumerate::query_active_topology()?;
    best_effort_restore_gamma_ramps(&refreshed_snapshot, cached_sdr_ramps);
    Ok(())
}

pub(super) fn capture_sdr_gamma_ramps(
    snapshot: &TopologySnapshot,
) -> HashMap<GammaRampKey, GammaRampWords> {
    let mut ramps = HashMap::new();

    for path in &snapshot.raw.paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG == 0 {
            continue;
        }
        if target_advanced_color_enabled(path).unwrap_or(false) {
            continue;
        }

        let key = (
            luid_to_u64(
                path.targetInfo.adapterId.HighPart,
                path.targetInfo.adapterId.LowPart,
            ),
            path.targetInfo.id,
        );

        let Some(device_name) = source_gdi_device_name(path) else {
            continue;
        };
        let Some(ramp) = get_gamma_ramp_for_device(&device_name) else {
            continue;
        };
        ramps.insert(key, ramp);
    }

    ramps
}

pub(super) fn gamma_ramp_looks_identity(ramp: &GammaRampWords) -> bool {
    // Identity ramp is approximately i * 257 for each channel. Allow small tolerance for
    // driver quantization noise.
    let tolerance = 384u16;
    for channel in 0..3 {
        let base = channel * 256;
        for i in 0..256usize {
            let expected = (i as u32 * 257) as i32;
            let actual = ramp[base + i] as i32;
            if (actual - expected).unsigned_abs() > tolerance as u32 {
                return false;
            }
        }
    }
    true
}

pub(super) fn active_color_state_signature(snapshot: &TopologySnapshot) -> String {
    let mut entries: Vec<(u64, u32, Option<bool>)> = Vec::new();

    for path in &snapshot.raw.paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG == 0 {
            continue;
        }

        let key = (
            luid_to_u64(
                path.targetInfo.adapterId.HighPart,
                path.targetInfo.adapterId.LowPart,
            ),
            path.targetInfo.id,
        );
        entries.push((key.0, key.1, target_advanced_color_enabled(path)));
    }

    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut signature = String::new();
    for (index, (adapter_luid, target_id, hdr_enabled)) in entries.iter().enumerate() {
        if index > 0 {
            signature.push(';');
        }
        let hdr_flag = match hdr_enabled {
            Some(true) => '1',
            Some(false) => '0',
            None => 'x',
        };
        signature.push_str(&format!("{adapter_luid:016x}:{target_id}:{hdr_flag}"));
    }

    signature
}

fn best_effort_reload_color_calibration() {
    if std::env::var_os("MONARCH_SKIP_COLOR_RELOAD").is_some() {
        return;
    }

    // Topology changes can reset gamma/LUT calibration on some drivers. First try a user-mode
    // WCS calibration-management toggle (off->on) to prompt recalibration without admin rights.
    unsafe {
        let mut enabled = BOOL(0);
        if WcsGetCalibrationManagementState(&mut enabled).as_bool() && enabled.as_bool() {
            let disabled = WcsSetCalibrationManagementState(false);
            let reenabled = WcsSetCalibrationManagementState(true);
            if disabled.as_bool() && reenabled.as_bool() {
                return;
            }
        }
    }

    // Fallback: trigger Windows' built-in calibration loader task (may fail under standard user
    // task permissions on some machines; that's fine).
    if let Ok(child) = Command::new("schtasks.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "/Run",
            "/TN",
            r"\Microsoft\Windows\WindowsColorSystem\Calibration Loader",
        ])
        .spawn()
    {
        let _ = wait_child_with_timeout(child, "schtasks.exe", Duration::from_secs(5));
    }
}

/// Poll a child process in 100ms steps until it exits or the timeout elapses. On timeout the
/// child is killed and `None` is returned, so a wedged helper process can never block an apply
/// (and with it the global state mutex) indefinitely.
fn wait_child_with_timeout(mut child: Child, name: &str, timeout: Duration) -> Option<ExitStatus> {
    let poll_step = Duration::from_millis(100);
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(err) => {
                diagnostics::log(format!("child_wait:error:{name}:{err}"));
                let _ = child.kill();
                return None;
            }
        }
        if Instant::now() >= deadline {
            diagnostics::log(format!("child_wait:timeout:{name}"));
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(poll_step);
    }
}

fn capture_active_gamma_ramps(snapshot: &TopologySnapshot) -> HashMap<(u64, u32), GammaRampWords> {
    let mut ramps = HashMap::new();

    for path in &snapshot.raw.paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG == 0 {
            continue;
        }

        let key = (
            luid_to_u64(
                path.targetInfo.adapterId.HighPart,
                path.targetInfo.adapterId.LowPart,
            ),
            path.targetInfo.id,
        );

        let Some(device_name) = source_gdi_device_name(path) else {
            continue;
        };
        let Some(ramp) = get_gamma_ramp_for_device(&device_name) else {
            continue;
        };
        ramps.insert(key, ramp);
    }

    ramps
}

fn best_effort_restore_gamma_ramps(
    snapshot: &TopologySnapshot,
    ramps: &HashMap<(u64, u32), GammaRampWords>,
) {
    for path in &snapshot.raw.paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG == 0 {
            continue;
        }

        let key = (
            luid_to_u64(
                path.targetInfo.adapterId.HighPart,
                path.targetInfo.adapterId.LowPart,
            ),
            path.targetInfo.id,
        );

        let Some(ramp) = ramps.get(&key) else {
            continue;
        };
        let Some(device_name) = source_gdi_device_name(path) else {
            continue;
        };
        let _ = set_gamma_ramp_for_device(&device_name, ramp);
    }
}

fn desired_output_index(desired: &Layout) -> HashMap<(u64, u32), &monarch::OutputConfig> {
    desired
        .outputs
        .iter()
        .map(|output| {
            (
                (output.display_id.adapter_luid, output.display_id.target_id),
                output,
            )
        })
        .collect()
}

fn path_target_key(path: &DISPLAYCONFIG_PATH_INFO) -> (u64, u32) {
    (
        luid_to_u64(
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
        ),
        path.targetInfo.id,
    )
}

fn apply_desired_source_mode(
    path: &DISPLAYCONFIG_PATH_INFO,
    modes: &mut [DISPLAYCONFIG_MODE_INFO],
    desired_output: Option<&&monarch::OutputConfig>,
) {
    let Some(output) = desired_output.copied() else {
        return;
    };
    let monarch::model::ModePreference::Exact {
        position,
        resolution,
    } = output.mode_preference()
    else {
        // The recovery planner resolves automatic modes from Windows' observed
        // source mode before verification; never write a 0x0 mode into CCD.
        return;
    };

    let mode_index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
    let Some(mode) = modes.get_mut(mode_index) else {
        return;
    };
    if mode.infoType.0 != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE.0 {
        return;
    }

    unsafe {
        let source = &mut mode.Anonymous.sourceMode;
        source.position.x = position.x;
        source.position.y = position.y;
        source.width = resolution.width;
        source.height = resolution.height;
    }
}

fn apply_desired_target_refresh(
    path: &mut DISPLAYCONFIG_PATH_INFO,
    desired_output: Option<&&monarch::OutputConfig>,
) {
    let Some(output) = desired_output.copied() else {
        return;
    };
    let desired_refresh_mhz = output.refresh_rate_mhz.max(1);
    path.targetInfo.refreshRate.Numerator = desired_refresh_mhz;
    path.targetInfo.refreshRate.Denominator = 1000;
}

fn reorder_paths_for_desired_priority(
    paths: &mut [DISPLAYCONFIG_PATH_INFO],
    desired_outputs: &HashMap<(u64, u32), &monarch::OutputConfig>,
) {
    paths.sort_by(|left, right| {
        let left_rank = path_priority_rank(left, desired_outputs);
        let right_rank = path_priority_rank(right, desired_outputs);
        left_rank.cmp(&right_rank)
    });
}

fn path_priority_rank(
    path: &DISPLAYCONFIG_PATH_INFO,
    desired_outputs: &HashMap<(u64, u32), &monarch::OutputConfig>,
) -> (u8, i32, i32, u64, u32) {
    let key = path_target_key(path);
    let Some(output) = desired_outputs.get(&key) else {
        return (3, 0, 0, key.0, key.1);
    };

    if !output.enabled {
        return (2, 0, 0, key.0, key.1);
    }

    let bucket = if output.primary { 0 } else { 1 };
    (bucket, output.position.y, output.position.x, key.0, key.1)
}

fn source_gdi_device_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    unsafe {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };

        let status = DisplayConfigGetDeviceInfo(&mut source.header);
        if status != 0 {
            return None;
        }

        Some(wide_array_to_string(&source.viewGdiDeviceName))
    }
}

pub(super) fn target_monitor_device_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    unsafe {
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };

        let status = DisplayConfigGetDeviceInfo(&mut target.header);
        if status != 0 {
            return None;
        }

        Some(wide_array_to_string(&target.monitorDevicePath))
    }
}

pub(super) fn target_advanced_color_enabled(path: &DISPLAYCONFIG_PATH_INFO) -> Option<bool> {
    unsafe {
        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };

        let status = DisplayConfigGetDeviceInfo(&mut info.header);
        if status != 0 {
            return None;
        }

        let flags = info.Anonymous.value;
        Some((flags & (1 << 1)) != 0)
    }
}

fn get_gamma_ramp_for_device(device_name: &str) -> Option<GammaRampWords> {
    let hdc = create_display_dc(device_name)?;
    let mut ramp = [0u16; GAMMA_RAMP_WORDS];
    let ok = unsafe { GetDeviceGammaRamp(hdc, ramp.as_mut_ptr().cast()) }.as_bool();
    unsafe {
        let _ = DeleteDC(hdc);
    }
    if ok {
        Some(ramp)
    } else {
        None
    }
}

fn set_gamma_ramp_for_device(device_name: &str, ramp: &GammaRampWords) -> bool {
    let Some(hdc) = create_display_dc(device_name) else {
        return false;
    };
    let ok = unsafe { SetDeviceGammaRamp(hdc, ramp.as_ptr().cast()) }.as_bool();
    unsafe {
        let _ = DeleteDC(hdc);
    }
    ok
}

fn create_display_dc(device_name: &str) -> Option<windows::Win32::Graphics::Gdi::HDC> {
    let device_wide = to_wide_null(device_name);
    let hdc = unsafe {
        CreateDCW(
            w!("DISPLAY"),
            PCWSTR(device_wide.as_ptr()),
            PCWSTR::null(),
            None,
        )
    };
    if hdc.is_invalid() {
        None
    } else {
        Some(hdc)
    }
}

fn to_wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_array_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|ch| *ch == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

#[cfg(test)]
mod tests {
    use super::super::win32_types::RawTopologySnapshot;
    use super::*;
    use monarch::{DisplayId, OutputConfig, Position, Resolution};

    fn layout(targets: &[u32]) -> Layout {
        Layout {
            outputs: targets
                .iter()
                .enumerate()
                .map(|(index, target)| OutputConfig {
                    display_id: DisplayId {
                        adapter_luid: 1,
                        target_id: *target,
                        edid_hash: Some(*target as u64),
                        identity: Default::default(),
                    },
                    enabled: true,
                    position: Position {
                        x: index as i32 * 1920,
                        y: 0,
                    },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: index == 0,
                    rotation: None,
                })
                .collect(),
        }
    }

    fn snapshot(targets: &[u32]) -> TopologySnapshot {
        TopologySnapshot {
            raw: RawTopologySnapshot {
                paths: targets
                    .iter()
                    .enumerate()
                    .map(|(index, target)| {
                        let mut path = DISPLAYCONFIG_PATH_INFO::default();
                        path.sourceInfo.adapterId.LowPart = 1;
                        path.sourceInfo.id = index as u32;
                        path.targetInfo.adapterId.LowPart = 1;
                        path.targetInfo.id = *target;
                        path.flags = DISPLAYCONFIG_PATH_ACTIVE_FLAG;
                        path
                    })
                    .collect(),
                modes: Vec::new(),
            },
            layout: layout(targets),
            displays: Vec::new(),
        }
    }

    #[test]
    fn verification_rejects_two_active_displays_when_four_were_requested() {
        let error = verify_requested_outputs(&layout(&[1, 2, 3, 4]), &layout(&[1, 2])).unwrap_err();
        assert!(error.to_string().contains("missing: [3, 4]"));
    }

    #[test]
    fn verification_rejects_wrong_targets_even_when_active_count_matches() {
        let error =
            verify_requested_outputs(&layout(&[1, 2, 3, 4]), &layout(&[1, 2, 3, 5])).unwrap_err();
        assert!(error.to_string().contains("missing: [4]"));
        assert!(error.to_string().contains("unexpected: [5]"));
    }

    #[test]
    fn verification_rejects_a_display_that_remained_active_despite_disable() {
        let mut desired = layout(&[1, 2, 3, 4]);
        desired.outputs[3].enabled = false;
        let error = verify_requested_outputs(&desired, &layout(&[1, 2, 3, 4])).unwrap_err();
        assert!(error.to_string().contains("unexpected: [4]"));
    }

    #[test]
    fn verification_accepts_all_four_requested_displays_in_any_order() {
        assert!(verify_requested_outputs(&layout(&[1, 2, 3, 4]), &layout(&[4, 3, 2, 1])).is_ok());
    }

    #[test]
    fn verification_does_not_confuse_matching_target_ids_on_different_adapters() {
        let desired = layout(&[1, 2]);
        let mut actual = desired.clone();
        actual.outputs[1].display_id.adapter_luid = 2;
        assert!(verify_requested_outputs(&desired, &actual).is_err());
    }

    #[test]
    fn preflight_rejects_missing_requested_raw_paths() {
        assert!(ensure_requested_paths(&layout(&[1, 2, 3, 4]), &snapshot(&[1, 2])).is_err());
    }

    #[test]
    fn preflight_rejects_duplicate_alternative_paths_for_requested_targets() {
        assert!(ensure_requested_paths(&layout(&[1, 2]), &snapshot(&[1, 2, 2])).is_err());
    }

    #[test]
    fn preflight_accepts_exactly_one_path_for_every_requested_target() {
        assert!(ensure_requested_paths(&layout(&[1, 2, 3, 4]), &snapshot(&[1, 2, 3, 4])).is_ok());
    }

    #[test]
    fn preflight_does_not_require_raw_paths_for_disabled_targets() {
        let mut desired = layout(&[1, 2]);
        desired.outputs[1].enabled = false;
        assert!(ensure_requested_paths(&desired, &snapshot(&[1])).is_ok());
    }
}
