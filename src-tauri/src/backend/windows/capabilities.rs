use super::{apply::source_gdi_device_name, win32_types::TopologySnapshot};
use monarch::{
    capabilities::{DisplayCapabilities, DisplayMode},
    ManagerError, Resolution, Rotation,
};
use std::{collections::HashMap, mem::size_of};
use windows::Win32::{
    Devices::Display::*,
    Graphics::Dxgi::{Common::*, *},
};

pub fn discover(snapshot: &TopologySnapshot) -> Vec<DisplayCapabilities> {
    let modes_by_source = dxgi_modes();
    snapshot.layout.outputs.iter().map(|output| {
        let path = snapshot.raw.paths.iter().find(|p| p.targetInfo.id == output.display_id.target_id && super::win32_types::luid_to_u64(p.targetInfo.adapterId.HighPart,p.targetInfo.adapterId.LowPart) == output.display_id.adapter_luid && (p.flags & 1 != 0 || !output.enabled));
        let mut modes = Vec::new();
        let mut hdr = None;
        let mut scaling = None;
        if let Some(path) = path {
            hdr = super::hdr::query(path);
            scaling = super::scaling::query(path);
            // A detached route can point at a source currently driving a different
            // monitor. A clone source also cannot describe each target's modes.
            if output.enabled && output.clone_group.is_none() {
                if let Some(name) = source_gdi_device_name(path) { modes = modes_by_source.get(&name).cloned().unwrap_or_default(); }
            }
            if output.enabled {
                let mut resolution = output.resolution.clone();
                if matches!(output.rotation, Some(Rotation::Portrait | Rotation::PortraitFlipped)) { std::mem::swap(&mut resolution.width,&mut resolution.height); }
                modes.push(DisplayMode { resolution, refresh_rate_mhz: output.refresh_rate_mhz });
            }
            // Target-specific preferred mode is safe even on detached/clone routes.
            let mut preferred = DISPLAYCONFIG_TARGET_PREFERRED_MODE { header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE, size: size_of::<DISPLAYCONFIG_TARGET_PREFERRED_MODE>() as u32,
                adapterId: path.targetInfo.adapterId, id: path.targetInfo.id,
            }, ..Default::default() };
            if unsafe { DisplayConfigGetDeviceInfo(&mut preferred.header) } == 0 {
                let frequency = preferred.targetMode.targetVideoSignalInfo.vSyncFreq;
                if preferred.width > 0 && preferred.height > 0 && frequency.Denominator > 0 {
                    modes.push(DisplayMode { resolution: Resolution { width: preferred.width, height: preferred.height }, refresh_rate_mhz: ((u64::from(frequency.Numerator)*1000)/u64::from(frequency.Denominator)) as u32 });
                }
            }
        }
        modes.sort_by_key(|m| (m.resolution.width,m.resolution.height,m.refresh_rate_mhz)); modes.dedup();
        let hdr_supported = hdr.as_ref().is_some_and(|h| h.supported);
        DisplayCapabilities {
            display_id: output.display_id.clone(),
            modes_unavailable_reason: (modes.is_empty() || !output.enabled || output.clone_group.is_some()).then(|| "Additional modes require this monitor to be extended and active. Saved preferences are retained.".into()), modes,
            hdr_supported, hdr_enabled: hdr.as_ref().map(|h| h.enabled),
            hdr_unavailable_reason: (!hdr_supported).then(|| "HDR is unsupported or unavailable in the current Windows configuration.".into()),
            scale_percent: scaling.as_ref().map(|s|s.current),
            scale_percentages: scaling.as_ref().map(|s|s.supported.clone()).unwrap_or_default(),
            scaling_unavailable_reason: scaling.is_none().then(|| "Windows cannot read this source's standard scaling range. Attach the display and turn off custom scaling in Windows.".into()),
        }
    }).collect()
}
pub fn current() -> Result<Vec<DisplayCapabilities>, ManagerError> {
    Ok(discover(&super::enumerate::query_connected_topology()?))
}

fn dxgi_modes() -> HashMap<String, Vec<DisplayMode>> {
    let mut result = HashMap::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return result;
        };
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            adapter_index += 1;
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                output_index += 1;
                let Ok(desc) = output.GetDesc() else {
                    continue;
                };
                let mut count = 0;
                if output
                    .GetDisplayModeList(
                        DXGI_FORMAT_R8G8B8A8_UNORM,
                        DXGI_ENUM_MODES(0),
                        &mut count,
                        None,
                    )
                    .is_err()
                    || count > 16384
                {
                    continue;
                }
                let mut modes = vec![DXGI_MODE_DESC::default(); count as usize];
                if output
                    .GetDisplayModeList(
                        DXGI_FORMAT_R8G8B8A8_UNORM,
                        DXGI_ENUM_MODES(0),
                        &mut count,
                        Some(modes.as_mut_ptr()),
                    )
                    .is_err()
                {
                    continue;
                }
                let name = String::from_utf16_lossy(
                    &desc.DeviceName[..desc
                        .DeviceName
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(desc.DeviceName.len())],
                );
                result.insert(
                    name,
                    modes
                        .into_iter()
                        .take(count as usize)
                        .filter(|m| m.RefreshRate.Denominator > 0)
                        .map(|m| DisplayMode {
                            resolution: Resolution {
                                width: m.Width,
                                height: m.Height,
                            },
                            refresh_rate_mhz: ((u64::from(m.RefreshRate.Numerator) * 1000)
                                / u64::from(m.RefreshRate.Denominator))
                                as u32,
                        })
                        .collect(),
                );
            }
        }
    }
    result
}
