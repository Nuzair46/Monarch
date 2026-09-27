//! Probe modern HDR-specific requests before the legacy advanced-color API.
use monarch::ManagerError;
use std::mem::size_of;
use windows::Win32::Devices::Display::*;
#[repr(C)]
#[derive(Default)]
struct ColorInfo2 {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    flags: u32,
    encoding: u32,
    bits: u32,
    active_mode: u32,
}
#[repr(C)]
struct ColorSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    enabled: u32,
}
pub struct Hdr {
    pub supported: bool,
    pub enabled: bool,
    modern: bool,
}
fn header(
    path: &DISPLAYCONFIG_PATH_INFO,
    kind: i32,
    size: usize,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: DISPLAYCONFIG_DEVICE_INFO_TYPE(kind),
        size: size as u32,
        adapterId: path.targetInfo.adapterId,
        id: path.targetInfo.id,
    }
}
pub fn query(path: &DISPLAYCONFIG_PATH_INFO) -> Option<Hdr> {
    let mut modern = ColorInfo2 {
        header: header(path, 15, size_of::<ColorInfo2>()),
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut modern.header) } == 0 {
        return Some(Hdr {
            supported: modern.flags & 16 != 0 && modern.flags & 8 == 0,
            enabled: modern.flags & 32 != 0,
            modern: true,
        });
    }
    let mut legacy = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
        header: header(path, 9, size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>()),
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut legacy.header) } != 0 {
        return None;
    }
    let flags = unsafe { legacy.Anonymous.value };
    Some(Hdr {
        supported: flags & 1 != 0 && flags & 8 == 0,
        enabled: flags & 2 != 0,
        modern: false,
    })
}
pub fn set(path: &DISPLAYCONFIG_PATH_INFO, enabled: bool) -> Result<(), ManagerError> {
    let info =
        query(path).ok_or_else(|| ManagerError::Backend("Windows cannot read HDR state".into()))?;
    if info.enabled == enabled {
        return Ok(());
    }
    if !info.supported {
        return Err(ManagerError::Validation(
            "HDR is unavailable in this display configuration".into(),
        ));
    }
    let request = ColorSet {
        header: header(
            path,
            if info.modern { 16 } else { 10 },
            size_of::<ColorSet>(),
        ),
        enabled: u32::from(enabled),
    };
    let status = unsafe { DisplayConfigSetDeviceInfo(&request.header) };
    if status != 0 {
        return Err(ManagerError::Backend(format!(
            "Windows rejected HDR change ({status}); choose a compatible mode/topology"
        )));
    }
    Ok(())
}
