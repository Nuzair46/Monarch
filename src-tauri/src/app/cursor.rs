//! Cursor service lifecycle is driven by the display worker; callbacks never enumerate.
#[cfg(target_os = "windows")]
#[path = "cursor_windows.rs"]
mod platform;
#[cfg(target_os = "windows")]
pub use platform::{epoch, shutdown, suspend, sync};
#[cfg(not(target_os = "windows"))]
pub fn epoch() -> u64 {
    0
}
#[cfg(not(target_os = "windows"))]
pub fn suspend() {}
#[cfg(not(target_os = "windows"))]
pub fn shutdown() {}
#[cfg(not(target_os = "windows"))]
pub fn sync(_: &monarch::AppSettings, _: &monarch::Layout, _: u64) -> Result<(), String> {
    Ok(())
}
