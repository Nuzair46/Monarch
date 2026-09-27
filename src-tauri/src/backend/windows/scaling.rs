//! Compatibility boundary for undocumented per-source DPI requests (-3/-4).
//! https://github.com/lihas/windows-DPI-scaling-sample/tree/master/DPIHelper
//! Fail closed on unknown ranges/custom scaling. Never write registry settings.
use monarch::ManagerError;
use std::mem::size_of;
use windows::Win32::Devices::Display::*;

const PERCENTAGES: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];
#[repr(C)]
#[derive(Default)]
struct DpiGet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    minimum: i32,
    current: i32,
    maximum: i32,
}
#[repr(C)]
struct DpiSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    relative: i32,
}
pub struct Scaling {
    pub current: u32,
    pub supported: Vec<u32>,
    recommended_index: i32,
}
fn decode(minimum: i32, current: i32, maximum: i32) -> Option<Scaling> {
    if minimum > 0 || maximum < 0 || current < minimum || current > maximum {
        return None;
    }
    let recommended = minimum.checked_neg()?;
    let last = recommended.checked_add(maximum)? as usize;
    let index = recommended.checked_add(current)? as usize;
    if last >= PERCENTAGES.len() || index > last {
        return None;
    }
    Some(Scaling {
        current: PERCENTAGES[index],
        supported: PERCENTAGES[..=last].to_vec(),
        recommended_index: recommended,
    })
}
pub fn query(path: &DISPLAYCONFIG_PATH_INFO) -> Option<Scaling> {
    if path.flags & 1 == 0 {
        return None;
    }
    let mut request = DpiGet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_TYPE(-3),
            size: size_of::<DpiGet>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut request.header) } != 0 {
        return None;
    }
    decode(request.minimum, request.current, request.maximum)
}
pub fn set(path: &DISPLAYCONFIG_PATH_INFO, percent: u32) -> Result<(), ManagerError> {
    let info = query(path).ok_or_else(|| {
        ManagerError::Backend(
            "Windows cannot read per-monitor scaling; custom scaling is unsupported".into(),
        )
    })?;
    let index = info
        .supported
        .iter()
        .position(|p| *p == percent)
        .ok_or_else(|| {
            ManagerError::Validation(format!(
                "{percent}% is outside this source's supported scaling range"
            ))
        })?;
    if info.current == percent {
        return Ok(());
    }
    let request = DpiSet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_TYPE(-4),
            size: size_of::<DpiSet>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        relative: index as i32 - info.recommended_index,
    };
    let status = unsafe { DisplayConfigSetDeviceInfo(&request.header) };
    if status != 0 {
        return Err(ManagerError::Backend(format!(
            "Windows rejected {percent}% scaling ({status})"
        )));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_only_complete_supported_ranges() {
        let info = decode(-3, -1, 2).unwrap();
        assert_eq!(info.current, 150);
        assert_eq!(info.supported, vec![100, 125, 150, 175, 200, 225]);
        for values in [
            (1, 1, 2),
            (-20, 0, 0),
            (-3, -4, 2),
            (-3, 3, 2),
            (i32::MIN, 0, 0),
            (-1, 1234567, 1234567),
        ] {
            assert!(decode(values.0, values.1, values.2).is_none());
        }
        assert_eq!(size_of::<DpiGet>(), 32);
        assert_eq!(size_of::<DpiSet>(), 24);
    }
}
