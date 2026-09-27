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
    DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_MODE_INFO_TYPE_TARGET,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, SDC_APPLY,
    SDC_NO_OPTIMIZATION, SDC_SAVE_TO_DATABASE, SDC_USE_SUPPLIED_DISPLAY_CONFIG, SDC_VALIDATE,
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

#[cfg(test)]
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
        let result = monarch::identity::resolve_layout(desired, &snapshot.layout)
            .and_then(|resolved| verify_requested_outputs(&resolved, &snapshot.layout));
        match result {
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
    desired.ensure_supported()?;
    let saved_gamma_ramps = capture_active_gamma_ramps(snapshot);
    let (next_paths, next_modes) = plan_layout(desired, snapshot)?;
    let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG;
    let status =
        unsafe { SetDisplayConfig(Some(&next_paths), Some(&next_modes), SDC_VALIDATE | flags) };
    if status != 0 {
        log_rejected_plan(status, &next_paths, &next_modes);
        return Err(validation_error(status));
    }
    let status = unsafe {
        SetDisplayConfig(
            Some(&next_paths),
            Some(&next_modes),
            SDC_APPLY | flags | SDC_SAVE_TO_DATABASE | SDC_NO_OPTIMIZATION,
        )
    };
    if status != 0 {
        return Err(ManagerError::Backend(format!(
            "SetDisplayConfig failed: {status}"
        )));
    }
    // Routing can change when splitting/joining clones; never set DPI on old IDs.
    let refreshed = wait_for_requested_outputs(desired)?;
    apply_preferences(desired, &refreshed)?;

    let next_snapshot = wait_for_verified_layout(desired)?;
    best_effort_reload_color_calibration();
    best_effort_restore_gamma_ramps(&next_snapshot, &saved_gamma_ramps);
    Ok(next_snapshot)
}

pub(super) fn wait_for_verified_layout(desired: &Layout) -> Result<TopologySnapshot, ManagerError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let next_snapshot = loop {
        let observed = super::enumerate::query_active_only_topology().and_then(|snapshot| {
            let resolved = monarch::identity::resolve_layout(desired, &snapshot.layout)?;
            monarch::verification::verify_applied_layout(&resolved, &snapshot.layout)?;
            Ok(snapshot)
        });
        match observed {
            Ok(snapshot) => break snapshot,
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    Ok(next_snapshot)
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

pub(super) fn best_effort_reload_color_calibration() {
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

pub(super) fn capture_active_gamma_ramps(
    snapshot: &TopologySnapshot,
) -> HashMap<(u64, u32), GammaRampWords> {
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

pub(super) fn best_effort_restore_gamma_ramps(
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

        if target_advanced_color_enabled(path).unwrap_or(true) {
            continue;
        }
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

pub(super) fn plan_layout(
    desired: &Layout,
    snapshot: &TopologySnapshot,
) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), ManagerError> {
    let mut paths = super::recovery::build_recovery_paths(desired, &snapshot.raw.paths)?;
    let outputs = desired_output_index(desired);
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::new();
    let mut sources = HashMap::new();
    for path in &mut paths {
        let output = outputs[&path_target_key(path)];
        // Position/primary/HDR/DPI changes must not round the refresh rational or
        // throw away the driver's working target timing. Reuse only an active
        // route to this target with the same requested source mode and rotation.
        let current = snapshot.layout.outputs.iter().find(|o| {
            o.enabled
                && o.display_id.endpoint() == output.display_id.endpoint()
                && o.resolution == output.resolution
                && o.refresh_rate_mhz == output.refresh_rate_mhz
                && o.rotation.unwrap_or(monarch::Rotation::Landscape)
                    == output.rotation.unwrap_or(monarch::Rotation::Landscape)
        });
        let active_path = current.and_then(|_| {
            snapshot.raw.paths.iter().find(|p| {
                p.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG != 0
                    && path_target_key(p) == path_target_key(path)
                    && p.sourceInfo.adapterId == path.sourceInfo.adapterId
                    && p.sourceInfo.id == path.sourceInfo.id
            })
        });
        let source_mode = active_path.and_then(|p| {
            snapshot
                .raw
                .modes
                .get(unsafe { p.sourceInfo.Anonymous.modeInfoIdx } as usize)
                .filter(|m| {
                    m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                        && m.id == p.sourceInfo.id
                        && m.adapterId == p.sourceInfo.adapterId
                })
        });
        let target_mode = active_path.and_then(|p| {
            snapshot
                .raw
                .modes
                .get(unsafe { p.targetInfo.Anonymous.modeInfoIdx } as usize)
                .filter(|m| {
                    m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET
                        && m.id == p.targetInfo.id
                        && m.adapterId == p.targetInfo.adapterId
                })
        });
        let key = (
            luid_to_u64(
                path.sourceInfo.adapterId.HighPart,
                path.sourceInfo.adapterId.LowPart,
            ),
            path.sourceInfo.id,
        );
        if output.resolution.width > 0 {
            let index = *sources.entry(key).or_insert_with(|| {
                let index = modes.len() as u32;
                let mut mode = source_mode.copied().unwrap_or(DISPLAYCONFIG_MODE_INFO {
                    infoType: DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
                    id: path.sourceInfo.id,
                    adapterId: path.sourceInfo.adapterId,
                    Anonymous: windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO_0 {
                        sourceMode: windows::Win32::Devices::Display::DISPLAYCONFIG_SOURCE_MODE {
                            width: output.resolution.width,
                            height: output.resolution.height,
                            pixelFormat:
                                windows::Win32::Devices::Display::DISPLAYCONFIG_PIXELFORMAT_32BPP,
                            position: windows::Win32::Foundation::POINTL {
                                x: output.position.x,
                                y: output.position.y,
                            },
                        },
                    },
                });
                mode.Anonymous.sourceMode.position = windows::Win32::Foundation::POINTL {
                    x: output.position.x,
                    y: output.position.y,
                };
                modes.push(mode);
                index
            });
            path.sourceInfo.Anonymous.modeInfoIdx = index;
            path.targetInfo.refreshRate.Numerator = output.refresh_rate_mhz;
            path.targetInfo.refreshRate.Denominator = 1000;
            // The enumerated DXGI mode list excludes interlaced modes. A concrete
            // refresh requires concrete scan-line ordering; recovery paths start
            // with UNSPECIFIED, which Windows permits only with a 0/0 refresh.
            path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
            if let (Some(active), Some(target)) = (active_path, target_mode) {
                path.targetInfo = active.targetInfo;
                path.targetInfo.Anonymous.modeInfoIdx = modes.len() as u32;
                modes.push(*target);
            }
        }
        if let Some(rotation) = output.rotation {
            path.targetInfo.rotation =
                windows::Win32::Devices::Display::DISPLAYCONFIG_ROTATION(match rotation {
                    monarch::Rotation::Landscape => 1,
                    monarch::Rotation::Portrait => 2,
                    monarch::Rotation::LandscapeFlipped => 3,
                    monarch::Rotation::PortraitFlipped => 4,
                });
        }
    }
    reorder_paths_for_desired_priority(&mut paths, &outputs);
    Ok((paths, modes))
}

pub(super) fn validation_error(status: i32) -> ManagerError {
    let message = match status {
        87 => "Windows rejected the display configuration data (87). Refresh the display list and try again; if this repeats, include the Monarch diagnostic log in the report.".to_string(),
        _ => format!("Windows rejected the requested display combination ({status}); select compatible resolution/refresh or Extend"),
    };
    ManagerError::Validation(message)
}

pub(super) fn log_rejected_plan(
    status: i32,
    paths: &[DISPLAYCONFIG_PATH_INFO],
    modes: &[DISPLAYCONFIG_MODE_INFO],
) {
    diagnostics::log(format!(
        "display_config:validation_failed:{status}:paths={}:modes={}",
        paths.len(),
        modes.len()
    ));
    for path in paths {
        diagnostics::log(format!("display_config:path:target={:?}:source={}:flags={:#x}:source_mode={}:target_mode={}:rotation={}:refresh={}/{}:scanline={}:scaling={}",
            path_target_key(path), path.sourceInfo.id, path.flags,
            unsafe {path.sourceInfo.Anonymous.modeInfoIdx}, unsafe {path.targetInfo.Anonymous.modeInfoIdx},
            path.targetInfo.rotation.0, path.targetInfo.refreshRate.Numerator, path.targetInfo.refreshRate.Denominator,
            path.targetInfo.scanLineOrdering.0, path.targetInfo.scaling.0));
    }
    for (index, mode) in modes.iter().enumerate() {
        if mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            let source = unsafe { mode.Anonymous.sourceMode };
            diagnostics::log(format!(
                "display_config:source:{index}:id={}:{}x{}:position={},{}:format={}",
                mode.id,
                source.width,
                source.height,
                source.position.x,
                source.position.y,
                source.pixelFormat.0
            ));
        } else if mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET {
            let signal = unsafe { mode.Anonymous.targetMode.targetVideoSignalInfo };
            diagnostics::log(format!(
                "display_config:target:{index}:id={}:{}x{}:refresh={}/{}:pixel_rate={}:scanline={}",
                mode.id,
                signal.activeSize.cx,
                signal.activeSize.cy,
                signal.vSyncFreq.Numerator,
                signal.vSyncFreq.Denominator,
                signal.pixelRate,
                signal.scanLineOrdering.0
            ));
        }
    }
}

pub(super) fn apply_preferences(
    desired: &Layout,
    refreshed: &TopologySnapshot,
) -> Result<(), ManagerError> {
    apply_preferences_inner(desired, refreshed, false)
}
pub(super) fn restore_preferences(
    desired: &Layout,
    refreshed: &TopologySnapshot,
) -> Result<(), ManagerError> {
    apply_preferences_inner(desired, refreshed, true)
}
fn apply_preferences_inner(
    desired: &Layout,
    refreshed: &TopologySnapshot,
    restoring: bool,
) -> Result<(), ManagerError> {
    let resolved = monarch::identity::resolve_layout(desired, &refreshed.layout)?;
    let mut failures = Vec::new();
    let mut apply_result = |result: Result<(), ManagerError>| -> Result<(), ManagerError> {
        match result {
            Ok(()) => Ok(()),
            Err(error) if restoring => {
                failures.push(error.to_string());
                Ok(())
            }
            Err(error) => Err(error),
        }
    };
    let mut scaled = HashSet::new();
    for output in resolved.outputs.iter().filter(|o| o.enabled) {
        let path = refreshed
            .raw
            .paths
            .iter()
            .find(|p| {
                path_target_key(p) == (output.display_id.adapter_luid, output.display_id.target_id)
            })
            .ok_or_else(|| {
                ManagerError::Backend(
                    "display disappeared before HDR/scaling could be applied".into(),
                )
            })?;
        if let Some(hdr) = output.hdr_enabled {
            apply_result(super::hdr::set(path, hdr))?;
        }
        let source = (
            luid_to_u64(
                path.sourceInfo.adapterId.HighPart,
                path.sourceInfo.adapterId.LowPart,
            ),
            path.sourceInfo.id,
        );
        if let Some(scale) = output.scale_percent {
            if scaled.insert(source) {
                apply_result(super::scaling::set(path, scale))?;
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ManagerError::RecoveryRequired(failures.join("; ")))
    }
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

pub(super) fn source_gdi_device_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
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
                    hdr_enabled: None,
                    scale_percent: None,
                    clone_group: None,
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

    fn active_modes() -> TopologySnapshot {
        active_modes_for(&[1, 2])
    }

    fn active_modes_for(targets: &[u32]) -> TopologySnapshot {
        use windows::Win32::Devices::Display::*;
        let mut snapshot = snapshot(targets);
        for (index, path) in snapshot.raw.paths.iter_mut().enumerate() {
            let output = &mut snapshot.layout.outputs[index];
            output.refresh_rate_mhz = 59_940;
            path.targetInfo.targetAvailable = BOOL(1);
            path.targetInfo.rotation = DISPLAYCONFIG_ROTATION_IDENTITY;
            path.targetInfo.scaling = DISPLAYCONFIG_SCALING_IDENTITY;
            path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
            path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
                Numerator: 60_000,
                Denominator: 1001,
            };
            path.sourceInfo.Anonymous.modeInfoIdx = snapshot.raw.modes.len() as u32;
            snapshot.raw.modes.push(DISPLAYCONFIG_MODE_INFO {
                infoType: DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
                id: path.sourceInfo.id,
                adapterId: path.sourceInfo.adapterId,
                Anonymous: DISPLAYCONFIG_MODE_INFO_0 {
                    sourceMode: DISPLAYCONFIG_SOURCE_MODE {
                        width: 1920,
                        height: 1080,
                        pixelFormat: DISPLAYCONFIG_PIXELFORMAT_32BPP,
                        position: windows::Win32::Foundation::POINTL {
                            x: output.position.x,
                            y: output.position.y,
                        },
                    },
                },
            });
            path.targetInfo.Anonymous.modeInfoIdx = snapshot.raw.modes.len() as u32;
            snapshot.raw.modes.push(DISPLAYCONFIG_MODE_INFO {
                infoType: DISPLAYCONFIG_MODE_INFO_TYPE_TARGET,
                id: path.targetInfo.id,
                adapterId: path.targetInfo.adapterId,
                Anonymous: DISPLAYCONFIG_MODE_INFO_0 {
                    targetMode: DISPLAYCONFIG_TARGET_MODE {
                        targetVideoSignalInfo: DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
                            pixelRate: 148_351_648,
                            vSyncFreq: path.targetInfo.refreshRate,
                            activeSize: DISPLAYCONFIG_2DREGION { cx: 1920, cy: 1080 },
                            totalSize: DISPLAYCONFIG_2DREGION { cx: 2200, cy: 1125 },
                            scanLineOrdering: DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE,
                            ..Default::default()
                        },
                    },
                },
            });
        }
        snapshot
    }

    #[test]
    fn moving_or_changing_primary_preserves_exact_active_target_timings() {
        let snapshot = active_modes();
        let mut desired = snapshot.layout.clone();
        desired.outputs[0].primary = false;
        desired.outputs[0].position = Position { x: 320, y: -1080 };
        desired.outputs[1].primary = true;
        desired.outputs[1].position = Position { x: 0, y: 0 };
        let (paths, modes) = plan_layout(&desired, &snapshot).unwrap();
        assert_eq!(modes.len(), 4);
        for path in paths {
            let output = desired
                .outputs
                .iter()
                .find(|o| o.display_id.target_id == path.targetInfo.id)
                .unwrap();
            let source = unsafe {
                modes[path.sourceInfo.Anonymous.modeInfoIdx as usize]
                    .Anonymous
                    .sourceMode
            };
            let target = unsafe {
                modes[path.targetInfo.Anonymous.modeInfoIdx as usize]
                    .Anonymous
                    .targetMode
                    .targetVideoSignalInfo
            };
            assert_eq!((source.width, source.height), (1920, 1080));
            assert_eq!(
                (source.position.x, source.position.y),
                (output.position.x, output.position.y)
            );
            assert_eq!(target.pixelRate, 148_351_648);
            assert_eq!((target.totalSize.cx, target.totalSize.cy), (2200, 1125));
            assert_eq!(
                (target.vSyncFreq.Numerator, target.vSyncFreq.Denominator),
                (60_000, 1001)
            );
            assert_eq!(
                (
                    path.targetInfo.refreshRate.Numerator,
                    path.targetInfo.refreshRate.Denominator
                ),
                (60_000, 1001)
            );
        }
    }

    #[test]
    fn changing_resolution_refresh_or_rotation_supplies_valid_scanline_ordering() {
        let snapshot = active_modes();
        for change in 0..3 {
            let mut desired = snapshot.layout.clone();
            match change {
                0 => {
                    desired.outputs[0].resolution = Resolution {
                        width: 1280,
                        height: 720,
                    }
                }
                1 => desired.outputs[0].refresh_rate_mhz = 60_000,
                _ => {
                    desired.outputs[0].rotation = Some(monarch::Rotation::Portrait);
                    desired.outputs[0].resolution = Resolution {
                        width: 1080,
                        height: 1920,
                    };
                }
            }
            let (paths, modes) = plan_layout(&desired, &snapshot).unwrap();
            let first = paths.iter().find(|p| p.targetInfo.id == 1).unwrap();
            let second = paths.iter().find(|p| p.targetInfo.id == 2).unwrap();
            assert_eq!(unsafe { first.targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
            assert_eq!(
                first.targetInfo.scanLineOrdering,
                DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE
            );
            assert_eq!(
                first.targetInfo.refreshRate.Numerator,
                desired.outputs[0].refresh_rate_mhz
            );
            assert_eq!(first.targetInfo.refreshRate.Denominator, 1000);
            let source = unsafe {
                modes[first.sourceInfo.Anonymous.modeInfoIdx as usize]
                    .Anonymous
                    .sourceMode
            };
            assert_eq!(
                (source.width, source.height),
                (
                    desired.outputs[0].resolution.width,
                    desired.outputs[0].resolution.height
                )
            );
            let target = unsafe {
                modes[second.targetInfo.Anonymous.modeInfoIdx as usize]
                    .Anonymous
                    .targetMode
                    .targetVideoSignalInfo
            };
            assert_eq!(target.pixelRate, 148_351_648);
        }
    }

    #[test]
    fn inactive_routes_never_reuse_another_sources_cached_mode() {
        let mut snapshot = active_modes();
        snapshot.layout.outputs[1].enabled = false;
        snapshot.raw.paths[1].flags = 0;
        let mut desired = snapshot.layout.clone();
        desired.outputs[1].enabled = true;
        let (paths, _) = plan_layout(&desired, &snapshot).unwrap();
        let second = paths.iter().find(|p| p.targetInfo.id == 2).unwrap();
        assert_eq!(unsafe { second.targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        assert_eq!(
            second.targetInfo.scanLineOrdering,
            DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE
        );
        assert_eq!(
            (
                second.targetInfo.refreshRate.Numerator,
                second.targetInfo.refreshRate.Denominator
            ),
            (59_940, 1000)
        );
    }

    #[test]
    fn automatic_mode_keeps_refresh_and_scanline_ordering_unspecified() {
        use windows::Win32::Devices::Display::DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED;
        let snapshot = active_modes();
        let mut desired = snapshot.layout.clone();
        desired.outputs[1].resolution = Resolution {
            width: 0,
            height: 0,
        };
        desired.outputs[1].refresh_rate_mhz = 0;
        let (paths, _) = plan_layout(&desired, &snapshot).unwrap();
        let second = paths.iter().find(|p| p.targetInfo.id == 2).unwrap();
        assert_eq!(unsafe { second.sourceInfo.Anonymous.modeInfoIdx }, u32::MAX);
        assert_eq!(unsafe { second.targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        assert_eq!(second.targetInfo.refreshRate.Numerator, 0);
        assert_eq!(second.targetInfo.refreshRate.Denominator, 0);
        assert_eq!(
            second.targetInfo.scanLineOrdering,
            DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED
        );
    }

    #[test]
    fn reattach_non_first_target_does_not_borrow_an_active_sources_mode() {
        let mut snapshot = active_modes_for(&[10, 30, 50]);
        let mut desired = snapshot.layout.clone();
        desired.outputs[1].resolution = Resolution {
            width: 1080,
            height: 1920,
        };
        desired.outputs[1].rotation = Some(monarch::Rotation::Portrait);
        desired.outputs[1].refresh_rate_mhz = 119_880;
        snapshot.layout.outputs[1].enabled = false;
        snapshot.layout.outputs[1].resolution = Resolution {
            width: 0,
            height: 0,
        };
        let mut inactive = snapshot.raw.paths.remove(1);
        inactive.flags = 0;
        // Inactive alternatives can alias active source indices. Only the
        // requested target's history may supply its mode after reconnection.
        for source in [2, 0, 1] {
            inactive.sourceInfo.id = source;
            inactive.sourceInfo.Anonymous.modeInfoIdx = source * 2;
            snapshot.raw.paths.push(inactive);
        }
        snapshot.raw.paths.reverse();
        let (paths, modes) = plan_layout(&desired, &snapshot).unwrap();
        assert_eq!(paths.len(), 3);
        assert_eq!(
            paths
                .iter()
                .map(|p| p.sourceInfo.id)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        let attached = paths.iter().find(|p| p.targetInfo.id == 30).unwrap();
        assert_eq!(attached.sourceInfo.id, 1);
        let source = unsafe {
            modes[attached.sourceInfo.Anonymous.modeInfoIdx as usize]
                .Anonymous
                .sourceMode
        };
        assert_eq!((source.width, source.height), (1080, 1920));
        assert_eq!((source.position.x, source.position.y), (1920, 0));
        assert_eq!(attached.targetInfo.rotation.0, 2);
        assert_eq!(
            (
                attached.targetInfo.refreshRate.Numerator,
                attached.targetInfo.refreshRate.Denominator
            ),
            (119_880, 1000)
        );
        assert_eq!(
            unsafe { attached.targetInfo.Anonymous.modeInfoIdx },
            u32::MAX
        );
        for path in paths.iter().filter(|p| p.targetInfo.id != 30) {
            assert_eq!(
                (
                    path.targetInfo.refreshRate.Numerator,
                    path.targetInfo.refreshRate.Denominator
                ),
                (60_000, 1001)
            );
            assert_ne!(unsafe { path.targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        }
    }

    #[test]
    fn joining_and_splitting_clones_supply_valid_timings_on_reassigned_sources() {
        let mut snapshot = active_modes_for(&[1, 2, 3]);
        // Make the second display's active mode different from the primary's.
        snapshot.layout.outputs[1].refresh_rate_mhz = 60_000;
        snapshot.raw.paths[1].targetInfo.refreshRate.Numerator = 60_000;
        snapshot.raw.paths[1].targetInfo.refreshRate.Denominator = 1000;
        snapshot.raw.modes[3]
            .Anonymous
            .targetMode
            .targetVideoSignalInfo
            .vSyncFreq = snapshot.raw.paths[1].targetInfo.refreshRate;
        let active = snapshot.raw.paths.clone();
        for path in active {
            for source in 0..3 {
                if source != path.sourceInfo.id {
                    let mut alternative = path;
                    alternative.flags = 0;
                    alternative.sourceInfo.id = source;
                    alternative.sourceInfo.Anonymous.modeInfoIdx = u32::MAX;
                    alternative.targetInfo.Anonymous.modeInfoIdx = u32::MAX;
                    snapshot.raw.paths.push(alternative);
                }
            }
        }
        let mut desired = snapshot.layout.clone();
        desired.outputs[0].clone_group = Some("pair".into());
        let id = desired.outputs[1].display_id.clone();
        desired.outputs[1] = desired.outputs[0].clone();
        desired.outputs[1].display_id = id;
        let (paths, modes) = plan_layout(&desired, &snapshot).unwrap();
        let first = paths.iter().find(|p| p.targetInfo.id == 1).unwrap();
        let second = paths.iter().find(|p| p.targetInfo.id == 2).unwrap();
        let third = paths.iter().find(|p| p.targetInfo.id == 3).unwrap();
        assert_eq!(first.sourceInfo.id, second.sourceInfo.id);
        assert_eq!(unsafe { first.sourceInfo.Anonymous.modeInfoIdx }, unsafe {
            second.sourceInfo.Anonymous.modeInfoIdx
        });
        assert_ne!(first.sourceInfo.id, third.sourceInfo.id);
        assert_eq!(unsafe { second.targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        assert_eq!(
            second.targetInfo.scanLineOrdering,
            DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE
        );
        assert_eq!(
            (
                second.targetInfo.refreshRate.Numerator,
                second.targetInfo.refreshRate.Denominator
            ),
            (59_940, 1000)
        );
        assert_ne!(unsafe { third.targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        assert_eq!(
            modes
                .iter()
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                .count(),
            2
        );

        // Observe the clone, then restore the original extended configuration.
        let mut cloned = snapshot.clone();
        cloned.layout = desired;
        cloned.raw.paths.iter_mut().for_each(|p| p.flags = 0);
        cloned.raw.paths.extend(paths);
        cloned.raw.modes = modes;
        let (paths, modes) = plan_layout(&snapshot.layout, &cloned).unwrap();
        assert_eq!(
            paths
                .iter()
                .map(|p| p.sourceInfo.id)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        let second = paths.iter().find(|p| p.targetInfo.id == 2).unwrap();
        assert_eq!(
            second.targetInfo.scanLineOrdering,
            DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE
        );
        assert_eq!(
            (
                second.targetInfo.refreshRate.Numerator,
                second.targetInfo.refreshRate.Denominator
            ),
            (60_000, 1000)
        );
        assert_eq!(
            modes
                .iter()
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                .count(),
            3
        );
    }

    #[test]
    fn unchanged_interlaced_target_keeps_its_observed_scanline_ordering() {
        use windows::Win32::Devices::Display::DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED;
        let mut snapshot = active_modes();
        snapshot.raw.paths[0].targetInfo.scanLineOrdering =
            DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED;
        snapshot.raw.modes[1]
            .Anonymous
            .targetMode
            .targetVideoSignalInfo
            .scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED;
        let (paths, modes) = plan_layout(&snapshot.layout, &snapshot).unwrap();
        let first = paths.iter().find(|p| p.targetInfo.id == 1).unwrap();
        assert_eq!(
            first.targetInfo.scanLineOrdering,
            DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED
        );
        assert_eq!(
            unsafe {
                modes[first.targetInfo.Anonymous.modeInfoIdx as usize]
                    .Anonymous
                    .targetMode
                    .targetVideoSignalInfo
                    .scanLineOrdering
            },
            DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED
        );
    }

    #[test]
    fn cloned_pair_and_extended_monitor_retain_target_timings_with_one_mode_per_source() {
        let mut snapshot = active_modes();
        snapshot.layout.outputs[0].clone_group = Some("pair".into());
        let mut clone = snapshot.layout.outputs[0].clone();
        clone.display_id.target_id = 3;
        clone.display_id.edid_hash = Some(3);
        snapshot.layout.outputs.push(clone);
        let mut path = snapshot.raw.paths[0];
        path.targetInfo.id = 3;
        path.targetInfo.Anonymous.modeInfoIdx = snapshot.raw.modes.len() as u32;
        let mut mode = snapshot.raw.modes[1];
        mode.id = 3;
        snapshot.raw.modes.push(mode);
        snapshot.raw.paths.push(path);
        let (paths, modes) = plan_layout(&snapshot.layout, &snapshot).unwrap();
        assert_eq!(
            modes
                .iter()
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                .count(),
            2
        );
        assert_eq!(
            modes
                .iter()
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET)
                .count(),
            3
        );
        let first = paths.iter().find(|p| p.targetInfo.id == 1).unwrap();
        let third = paths.iter().find(|p| p.targetInfo.id == 3).unwrap();
        assert_eq!(unsafe { first.sourceInfo.Anonymous.modeInfoIdx }, unsafe {
            third.sourceInfo.Anonymous.modeInfoIdx
        });
    }
}
