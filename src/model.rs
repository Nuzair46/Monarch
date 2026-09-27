use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::ManagerError;

pub const DEFAULT_PROFILE_SHORTCUT_BASE: &str = "Ctrl+Shift";
pub const DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE: &str = "Ctrl+Alt";

fn default_global_shortcuts_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DisplayId {
    pub adapter_luid: u64,
    pub target_id: u32,
    pub edid_hash: Option<u64>,
    #[serde(default)]
    pub identity: MonitorIdentity,
}

/// Evidence about a monitor, independent of its current GPU address.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct MonitorIdentity {
    pub device_path: Option<String>,
    pub edid_serial: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct DisplayEndpoint(pub u64, pub u32);

impl DisplayId {
    pub fn endpoint(&self) -> DisplayEndpoint {
        DisplayEndpoint(self.adapter_luid, self.target_id)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Landscape,
    Portrait,
    LandscapeFlipped,
    PortraitFlipped,
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
    /// None in legacy profiles means preserve the observed orientation.
    #[serde(default)]
    pub rotation: Option<Rotation>,
}

/// Legacy files encode an automatic mode as 0x0. Resolve that representation at
/// the planning boundary; an automatic mode must never reach SetDisplayConfig.
pub enum ModePreference<'a> {
    Automatic,
    Exact {
        position: &'a Position,
        resolution: &'a Resolution,
    },
}

impl OutputConfig {
    pub fn mode_preference(&self) -> ModePreference<'_> {
        if self.resolution.width == 0 && self.resolution.height == 0 {
            ModePreference::Automatic
        } else {
            ModePreference::Exact {
                position: &self.position,
                resolution: &self.resolution,
            }
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub outputs: Vec<OutputConfig>,
}

impl Layout {
    /// Profiles currently describe extended desktops. Reject unsupported cloned
    /// sources before journaling or mutating rather than guessing their routing.
    pub fn ensure_supported(&self) -> Result<(), ManagerError> {
        self.ensure_valid()?;
        let active: Vec<_> = self
            .outputs
            .iter()
            .filter(|o| o.enabled && o.resolution.width > 0)
            .collect();
        for (i, a) in active.iter().enumerate() {
            for b in active.iter().skip(i + 1) {
                let overlap = i64::from(a.position.x)
                    < i64::from(b.position.x) + i64::from(b.resolution.width)
                    && i64::from(b.position.x)
                        < i64::from(a.position.x) + i64::from(a.resolution.width)
                    && i64::from(a.position.y)
                        < i64::from(b.position.y) + i64::from(b.resolution.height)
                    && i64::from(b.position.y)
                        < i64::from(a.position.y) + i64::from(a.resolution.height);
                if overlap {
                    return Err(ManagerError::Validation("cloned or overlapping displays are not supported; select Extend in Windows Display Settings first".into()));
                }
            }
        }
        Ok(())
    }
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

        if self
            .outputs
            .iter()
            .filter(|o| o.enabled && o.primary)
            .count()
            > 1
        {
            return Err(ManagerError::Validation(
                "layout has multiple primary displays".into(),
            ));
        }
        let mut endpoints = HashSet::new();
        for output in &self.outputs {
            if !endpoints.insert(output.display_id.endpoint()) {
                return Err(ManagerError::Validation(
                    "layout contains a duplicate display target".into(),
                ));
            }
            if output.position.x.unsigned_abs() > 1_000_000
                || output.position.y.unsigned_abs() > 1_000_000
                || output.resolution.width > 65535
                || output.resolution.height > 65535
                || (output.resolution.width == 0) != (output.resolution.height == 0)
                || output.refresh_rate_mhz > 1_000_000
            {
                return Err(ManagerError::Validation(
                    "display geometry or refresh rate is out of range".into(),
                ));
            }
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub revert_timeout_secs: u64,
    pub start_with_windows: bool,
    pub startup_profile_name: Option<String>,
    #[serde(default = "default_global_shortcuts_enabled")]
    pub global_shortcuts_enabled: bool,
    pub profile_shortcut_base: Option<String>,
    pub display_toggle_shortcut_base: Option<String>,
    pub profile_shortcuts: BTreeMap<String, String>,
    pub display_toggle_shortcuts: BTreeMap<String, String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            revert_timeout_secs: 10,
            start_with_windows: false,
            startup_profile_name: None,
            global_shortcuts_enabled: true,
            profile_shortcut_base: Some(DEFAULT_PROFILE_SHORTCUT_BASE.to_string()),
            display_toggle_shortcut_base: Some(DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE.to_string()),
            profile_shortcuts: BTreeMap::new(),
            display_toggle_shortcuts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub profiles: Vec<Profile>,
    pub display_fingerprints: Vec<DisplayFingerprint>,
    pub last_known_good_layout: Option<Layout>,
    pub last_restorable_layout: Option<Layout>,
    pub settings: AppSettings,
    /// Written before mutation; removed only after confirmation or verified recovery.
    pub pending_recovery: Option<Layout>,
}

pub const CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug)]
pub struct DisplaySnapshot {
    pub generation: u64,
    pub displays: Vec<DisplayInfo>,
    pub layout: Layout,
}
