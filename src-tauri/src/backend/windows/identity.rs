use std::mem::size_of;
use windows::core::{w, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::System::Registry::*;

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn monitor_edid(device_path: &str) -> Option<[u8; 128]> {
    let path: Vec<u16> = device_path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let set = DeviceSet(SetupDiCreateDeviceInfoList(None, None).ok()?);
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        SetupDiOpenDeviceInterfaceW(set.0, PCWSTR(path.as_ptr()), 0, Some(&mut interface)).ok()?;
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SetupAPI documents that this size-query failure still fills DeviceInfoData.
        let result =
            SetupDiGetDeviceInterfaceDetailW(set.0, &interface, None, 0, None, Some(&mut device));
        if result.is_err_and(|e| e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()) {
            return None;
        }
        let key =
            Key(
                SetupDiOpenDevRegKey(set.0, &device, DICS_FLAG_GLOBAL.0, 0, DIREG_DEV, KEY_READ.0)
                    .ok()?,
            );
        let mut bytes = [0u8; 128];
        let count = bytes.len() as u32;
        // Read the full value first: registry EDIDs may include extension blocks.
        let mut required = 0;
        RegGetValueW(
            key.0,
            None,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut required),
        )
        .ok()
        .ok()?;
        if !(128..=32768).contains(&required) {
            return None;
        }
        let mut data = vec![0u8; required as usize];
        RegGetValueW(
            key.0,
            None,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut required),
        )
        .ok()
        .ok()?;
        bytes.copy_from_slice(data.get(..count as usize)?);
        Some(bytes)
    }
}

pub fn monitor_serial(path: &str) -> Option<String> {
    monarch::identity::edid_serial(&monitor_edid(path)?)
}
