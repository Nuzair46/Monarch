pub mod audio;
pub mod backend;
pub mod capabilities;
pub mod error;
pub mod history;
pub mod identity;
pub mod manager;
pub mod model;
mod placement;
pub mod store;
pub mod transaction;
pub mod verification;
pub mod watchdog;

pub use audio::{AudioDefaults, AudioDevice, AudioOutput, AudioSnapshot};
pub use backend::{DisplayBackend, MockBackend};
pub use error::ManagerError;
pub use manager::MonarchDisplayManager;
pub use model::{
    AppConfig, AppSettings, DisplayEndpoint, DisplayFingerprint, DisplayId, DisplayInfo,
    DisplaySnapshot, Layout, MonitorIdentity, OutputConfig, Position, Profile, Resolution,
    Rotation, DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE, DEFAULT_PROFILE_SHORTCUT_BASE,
};
pub use store::{ConfigStore, FileConfigStore, MemoryConfigStore};
