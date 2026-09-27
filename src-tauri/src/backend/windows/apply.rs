#![cfg(target_os = "windows")]

use std::collections::HashMap;
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
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::ColorSystem::{
    GetDeviceGammaRamp, SetDeviceGammaRamp, WcsGetCalibrationManagementState,
    WcsSetCalibrationManagementState,
};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION};

use super::win32_types::{luid_to_u64, AttachablePath, TopologySnapshot};

const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 0x0000_0001;
/// `DISPLAYCONFIG_PATH_MODE_IDX_INVALID`: "no mode supplied, let Windows pick one".
const DISPLAYCONFIG_PATH_MODE_IDX_INVALID: u32 = 0xffff_ffff;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const GAMMA_RAMP_WORDS: usize = 3 * 256;
pub(super) type GammaRampKey = (u64, u32);
pub(super) type GammaRampWords = [u16; GAMMA_RAMP_WORDS];

pub fn apply_layout_against_snapshot(
    desired: &Layout,
    snapshot: &TopologySnapshot,
) -> Result<TopologySnapshot, ManagerError> {
    desired.ensure_valid()?;
    let saved_gamma_ramps = capture_active_gamma_ramps(snapshot);
    let saved_wallpapers = capture_active_wallpapers(snapshot);
    let saved_wallpaper_position = capture_wallpaper_position();

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
    best_effort_restore_wallpapers(&next_snapshot, &saved_wallpapers);
    best_effort_restore_wallpaper_position(saved_wallpaper_position);
    Ok(next_snapshot)
}

/// Add the candidate targets to the active topology, keeping known modes intact.
/// All candidates are submitted together because each apply replaces the topology.
pub(super) fn build_attach_paths(
    candidates: &[&AttachablePath],
    active_snapshot: &TopologySnapshot,
) -> Vec<DISPLAYCONFIG_PATH_INFO> {
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = active_snapshot
        .raw
        .paths
        .iter()
        .filter(|path| path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG != 0)
        .copied()
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }

    for candidate in candidates {
        let mut next = candidate.path;
        next.flags |= DISPLAYCONFIG_PATH_ACTIVE_FLAG;
        next.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        next.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        paths.push(next);
    }
    paths
}

/// Permit Windows to compute modes for newly attached targets.
fn attach_flags() -> windows::Win32::Devices::Display::SET_DISPLAY_CONFIG_FLAGS {
    SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES
}

/// Dry-run: SDC_VALIDATE changes nothing, so it is free to call and mandatory before applying —
/// this runs on desktops with no internal panel, where a bad apply leaves no rescue screen.
/// Returns the raw SetDisplayConfig status (0 = the configuration is accepted).
pub(super) fn validate_attach_paths(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    active_snapshot: &TopologySnapshot,
) -> i32 {
    if paths.is_empty() {
        return -1;
    }
    unsafe {
        SetDisplayConfig(
            Some(paths),
            Some(active_snapshot.raw.modes.as_slice()),
            SDC_VALIDATE | attach_flags(),
        )
    }
}

/// Apply a path array previously accepted by `validate_attach_paths`. Returns the raw status.
/// NOTE: a 0 here does NOT prove any display came back — SetDisplayConfig returns 0 for a no-op
/// on an unchanged active set. The caller must confirm against a fresh enumeration.
pub(super) fn apply_attach_paths(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    active_snapshot: &TopologySnapshot,
) -> i32 {
    let validation = validate_attach_paths(paths, active_snapshot);
    if validation != 0 {
        return validation;
    }
    unsafe {
        SetDisplayConfig(
            Some(paths),
            Some(active_snapshot.raw.modes.as_slice()),
            SDC_APPLY | attach_flags(),
        )
    }
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

fn capture_active_wallpapers(snapshot: &TopologySnapshot) -> HashMap<(u64, u32), String> {
    let Some(session) = create_desktop_wallpaper_session() else {
        return HashMap::new();
    };
    let mut wallpapers = HashMap::new();

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

        let Some(monitor_device_path) = target_monitor_device_path(path) else {
            continue;
        };
        let Some(wallpaper_path) =
            get_wallpaper_for_monitor(&session.desktop_wallpaper, &monitor_device_path)
        else {
            continue;
        };
        wallpapers.insert(key, wallpaper_path);
    }

    wallpapers
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

fn best_effort_restore_wallpapers(
    snapshot: &TopologySnapshot,
    wallpapers: &HashMap<(u64, u32), String>,
) {
    if wallpapers.is_empty() {
        return;
    }

    let Some(session) = create_desktop_wallpaper_session() else {
        return;
    };

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
        let Some(wallpaper_path) = wallpapers.get(&key) else {
            continue;
        };
        let Some(monitor_device_path) = target_monitor_device_path(path) else {
            continue;
        };

        let _ = set_wallpaper_for_monitor(
            &session.desktop_wallpaper,
            &monitor_device_path,
            wallpaper_path,
        );
    }
}

fn capture_wallpaper_position() -> Option<DESKTOP_WALLPAPER_POSITION> {
    let session = create_desktop_wallpaper_session()?;
    unsafe { session.desktop_wallpaper.GetPosition().ok() }
}

fn best_effort_restore_wallpaper_position(position: Option<DESKTOP_WALLPAPER_POSITION>) {
    let Some(position) = position else {
        return;
    };
    let Some(session) = create_desktop_wallpaper_session() else {
        return;
    };
    let _ = unsafe { session.desktop_wallpaper.SetPosition(position) };
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
    if output.resolution.width == 0 || output.resolution.height == 0 {
        // Geometry sentinel (a seeded, never-yet-active display): writing 0x0 into the source
        // mode would make SetDisplayConfig fail with 87 or stack the display on the primary.
        // Leave the snapshot's real source mode untouched and let Windows place it.
        return;
    }

    let mode_index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
    let Some(mode) = modes.get_mut(mode_index) else {
        return;
    };
    if mode.infoType.0 != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE.0 {
        return;
    }

    unsafe {
        let source = &mut mode.Anonymous.sourceMode;
        source.position.x = output.position.x;
        source.position.y = output.position.y;
        source.width = output.resolution.width;
        source.height = output.resolution.height;
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
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        };

        let status = DisplayConfigGetDeviceInfo(&mut source.header);
        if status != 0 {
            return None;
        }

        Some(wide_array_to_string(&source.viewGdiDeviceName))
    }
}

fn target_monitor_device_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    unsafe {
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        target.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
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
        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
        info.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
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

struct DesktopWallpaperSession {
    desktop_wallpaper: IDesktopWallpaper,
    should_uninitialize: bool,
}

impl Drop for DesktopWallpaperSession {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

fn create_desktop_wallpaper_session() -> Option<DesktopWallpaperSession> {
    let mut should_uninitialize = false;
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() {
            should_uninitialize = true;
        }

        let desktop_wallpaper: IDesktopWallpaper =
            CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).ok()?;
        Some(DesktopWallpaperSession {
            desktop_wallpaper,
            should_uninitialize,
        })
    }
}

fn get_wallpaper_for_monitor(
    desktop_wallpaper: &IDesktopWallpaper,
    monitor_device_path: &str,
) -> Option<String> {
    let monitor_wide = to_wide_null(monitor_device_path);
    let wallpaper = unsafe {
        desktop_wallpaper
            .GetWallpaper(PCWSTR(monitor_wide.as_ptr()))
            .ok()?
    };

    let wallpaper_path = unsafe { wallpaper.to_string().ok() };
    unsafe {
        CoTaskMemFree(Some(wallpaper.0.cast()));
    }
    wallpaper_path
}

fn set_wallpaper_for_monitor(
    desktop_wallpaper: &IDesktopWallpaper,
    monitor_device_path: &str,
    wallpaper_path: &str,
) -> bool {
    let monitor_wide = to_wide_null(monitor_device_path);
    let wallpaper_wide = to_wide_null(wallpaper_path);
    unsafe {
        desktop_wallpaper
            .SetWallpaper(
                PCWSTR(monitor_wide.as_ptr()),
                PCWSTR(wallpaper_wide.as_ptr()),
            )
            .is_ok()
    }
}
