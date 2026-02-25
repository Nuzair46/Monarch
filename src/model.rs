use serde::{Deserialize, Serialize};

use crate::ManagerError;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DisplayId {
    pub adapter_luid: u64,
    pub target_id: u32,
    pub edid_hash: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub friendly_name: String,
    pub is_active: bool,
    pub is_primary: bool,
    pub resolution: Resolution,
    pub refresh_rate_mhz: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutputConfig {
    pub display_id: DisplayId,
    pub enabled: bool,
    pub position: Position,
    pub resolution: Resolution,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub outputs: Vec<OutputConfig>,
}

impl Layout {
    pub fn enabled_output_count(&self) -> usize {
        self.outputs.iter().filter(|output| output.enabled).count()
    }

    pub fn ensure_valid(&self) -> Result<(), ManagerError> {
        if self.outputs.is_empty() {
            return Err(ManagerError::Validation(
                "layout cannot be empty".to_string(),
            ));
        }

        if self.enabled_output_count() == 0 {
            return Err(ManagerError::Validation(
                "layout must have at least one enabled display".to_string(),
            ));
        }

        Ok(())
    }

    pub fn find_output_index(&self, display_id: &DisplayId) -> Option<usize> {
        self.outputs
            .iter()
            .position(|output| &output.display_id == display_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub layout: Layout,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DisplayFingerprint {
    pub display_id: DisplayId,
    pub friendly_name: String,
    pub edid_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub revert_timeout_secs: u64,
    pub start_with_windows: bool,
    pub startup_profile_name: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            revert_timeout_secs: 10,
            start_with_windows: false,
            startup_profile_name: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub profiles: Vec<Profile>,
    pub display_fingerprints: Vec<DisplayFingerprint>,
    pub last_known_good_layout: Option<Layout>,
    pub last_restorable_layout: Option<Layout>,
    pub settings: AppSettings,
}
