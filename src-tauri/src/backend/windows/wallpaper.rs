use super::win32_types::TopologySnapshot;
use windows::core::PCWSTR;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    DesktopWallpaper, IDesktopWallpaper, IShellItemArray, DESKTOP_SLIDESHOW_OPTIONS,
    DESKTOP_WALLPAPER_POSITION, DSS_ENABLED, DSS_SLIDESHOW,
};

struct ComApartment(bool);
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

enum Background {
    Static(Vec<(String, String)>),
    Slideshow(IShellItemArray, DESKTOP_SLIDESHOW_OPTIONS, u32),
}

/// All interfaces are dropped before the COM apartment, including construction failures.
pub(super) struct WallpaperState {
    background: Background,
    desktop: IDesktopWallpaper,
    enabled: bool,
    position: DESKTOP_WALLPAPER_POSITION,
    color: COLORREF,
    _apartment: ComApartment,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn wallpaper(desktop: &IDesktopWallpaper, monitor: &str) -> Option<String> {
    let monitor = wide(monitor);
    unsafe {
        let value = desktop.GetWallpaper(PCWSTR(monitor.as_ptr())).ok()?;
        let result = value.to_string().ok();
        CoTaskMemFree(Some(value.0.cast()));
        result
    }
}

impl WallpaperState {
    pub fn capture(snapshot: &TopologySnapshot) -> Option<Self> {
        unsafe {
            let apartment = ComApartment(CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok());
            let desktop: IDesktopWallpaper =
                CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).ok()?;
            let status = desktop.GetStatus().ok()?;
            let background = if status.0 & DSS_SLIDESHOW.0 != 0 {
                let items = desktop.GetSlideshow().ok()?;
                let mut options = DESKTOP_SLIDESHOW_OPTIONS::default();
                let mut tick = 0;
                desktop.GetSlideshowOptions(&mut options, &mut tick).ok()?;
                Background::Slideshow(items, options, tick)
            } else {
                let images = snapshot
                    .raw
                    .paths
                    .iter()
                    .filter(|p| p.flags & 1 != 0)
                    .filter_map(|path| {
                        let monitor = super::apply::target_monitor_device_path(path)?;
                        let image = wallpaper(&desktop, &monitor)?;
                        Some((monitor, image))
                    })
                    .collect();
                Background::Static(images)
            };
            Some(Self {
                background,
                enabled: status.0 & DSS_ENABLED.0 != 0,
                position: desktop.GetPosition().ok()?,
                color: desktop.GetBackgroundColor().ok()?,
                desktop,
                _apartment: apartment,
            })
        }
    }
}

impl Drop for WallpaperState {
    fn drop(&mut self) {
        unsafe {
            match &self.background {
                Background::Slideshow(items, options, tick) => {
                    // Leave a running slideshow alone: restarting it changes its current image.
                    if self
                        .desktop
                        .GetStatus()
                        .is_ok_and(|s| s.0 & DSS_SLIDESHOW.0 == 0)
                    {
                        let _ = self.desktop.SetSlideshowOptions(*options, *tick);
                        let _ = self.desktop.SetSlideshow(items);
                    }
                }
                Background::Static(images) => {
                    for (monitor, image) in images {
                        if wallpaper(&self.desktop, monitor).as_ref() != Some(image) {
                            let monitor = wide(monitor);
                            let image = wide(image);
                            let _ = self
                                .desktop
                                .SetWallpaper(PCWSTR(monitor.as_ptr()), PCWSTR(image.as_ptr()));
                        }
                    }
                }
            }
            if self.desktop.GetPosition().is_ok_and(|p| p != self.position) {
                let _ = self.desktop.SetPosition(self.position);
            }
            if self
                .desktop
                .GetBackgroundColor()
                .is_ok_and(|c| c != self.color)
            {
                let _ = self.desktop.SetBackgroundColor(self.color);
            }
            if self
                .desktop
                .GetStatus()
                .is_ok_and(|s| (s.0 & DSS_ENABLED.0 != 0) != self.enabled)
            {
                let _ = self.desktop.Enable(self.enabled);
            }
        }
    }
}
