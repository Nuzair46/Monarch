pub mod backend;
pub mod error;
pub mod manager;
pub mod model;
pub mod store;

pub use backend::{DisplayBackend, MockBackend, Win32DisplayBackend};
pub use error::ManagerError;
pub use manager::MonarchDisplayManager;
pub use model::{
    AppConfig, AppSettings, DisplayFingerprint, DisplayId, DisplayInfo, Layout, OutputConfig,
    Position, Profile, Resolution,
};
pub use store::{ConfigStore, FileConfigStore, MemoryConfigStore};
