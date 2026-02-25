#[cfg(target_os = "windows")]
mod apply;
#[cfg(target_os = "windows")]
mod enumerate;
#[cfg(target_os = "windows")]
mod topology;
#[cfg(target_os = "windows")]
mod win32_types;

#[cfg(target_os = "windows")]
pub use topology::WindowsDisplayBackend;

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone)]
pub struct WindowsDisplayBackend;
