use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::ManagerError;

pub const DEFAULT_PROFILE_SHORTCUT_BASE: &str = "Ctrl+Shift";
pub const DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE: &str = "Ctrl+Alt";

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayId {
    pub adapter_luid: u64,
    pub target_id: u32,
    #[serde(deserialize_with = "Option::deserialize")]
    pub edid_hash: Option<u64>,
    pub identity: MonitorIdentity,
}

/// Evidence about a monitor, independent of its current GPU address.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorIdentity {
    #[serde(deserialize_with = "Option::deserialize")]
    pub device_path: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
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
#[serde(deny_unknown_fields)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub display_id: DisplayId,
    pub enabled: bool,
    pub position: Position,
    pub resolution: Resolution,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
    /// Unknown orientation is explicit for outputs without an observed mode.
    #[serde(deserialize_with = "Option::deserialize")]
    pub rotation: Option<Rotation>,
    #[serde(default)]
    pub hdr_enabled: Option<bool>,
    #[serde(default)]
    pub scale_percent: Option<u32>,
    /// Layout-local identity; never a Windows source ID.
    #[serde(default)]
    pub clone_group: Option<String>,
}

/// An output without an observed mode uses 0x0. Resolve that automatic preference
/// at the planning boundary before passing a source mode to SetDisplayConfig.
pub enum ModePreference<'a> {
    Automatic,
    Exact {
        position: &'a Position,
        resolution: &'a Resolution,
    },
}

impl OutputConfig {
    pub fn shares_source(&self, other: &Self) -> bool {
        self.enabled
            && other.enabled
            && self.clone_group.is_some()
            && self.clone_group == other.clone_group
    }

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
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub outputs: Vec<OutputConfig>,
}

impl Layout {
    pub fn normalize_clone_groups(&mut self) {
        let mut counts = BTreeMap::new();
        for o in self.outputs.iter().filter(|o| o.enabled) {
            if let Some(group) = &o.clone_group {
                *counts.entry(group.clone()).or_insert(0) += 1;
            }
        }
        for o in &mut self.outputs {
            if !o.enabled
                || o.clone_group
                    .as_ref()
                    .is_some_and(|g| counts.get(g).copied().unwrap_or(0) < 2)
            {
                o.clone_group = None;
            }
        }
    }

    /// Overlap is permitted only for explicitly duplicated sources.
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
                if overlap && !a.shares_source(b) {
                    return Err(ManagerError::Validation(
                        "extended displays overlap; move them apart or choose Duplicate of".into(),
                    ));
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

        let primaries: Vec<_> = self
            .outputs
            .iter()
            .filter(|o| o.enabled && o.primary)
            .collect();
        if primaries
            .iter()
            .skip(1)
            .any(|o| !o.shares_source(primaries[0]))
        {
            return Err(ManagerError::Validation(
                "layout has multiple primary displays".into(),
            ));
        }
        for (index, a) in self.outputs.iter().enumerate() {
            if a.clone_group
                .as_ref()
                .is_some_and(|g| g.trim().is_empty() || g.len() > 128)
                || a.scale_percent.is_some_and(|s| !(100..=500).contains(&s))
            {
                return Err(ManagerError::Validation(
                    "invalid clone group or scaling percentage".into(),
                ));
            }
            for b in self
                .outputs
                .iter()
                .skip(index + 1)
                .filter(|b| a.shares_source(b))
            {
                if a.position != b.position
                    || a.resolution != b.resolution
                    || a.scale_percent != b.scale_percent
                    || a.primary != b.primary
                {
                    return Err(ManagerError::Validation("duplicated displays must share position, resolution, scaling and primary status".into()));
                }
            }
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
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub layout: Layout,
    #[serde(default)]
    pub audio_output: Option<crate::AudioOutput>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayFingerprint {
    pub display_id: DisplayId,
    pub friendly_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    pub revert_timeout_secs: u64,
    pub start_with_windows: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub startup_profile_name: Option<String>,
    pub global_shortcuts_enabled: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub profile_shortcut_base: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
    pub profiles: Vec<Profile>,
    pub display_fingerprints: Vec<DisplayFingerprint>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub last_known_good_layout: Option<Layout>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub last_restorable_layout: Option<Layout>,
    pub settings: AppSettings,
    /// Written before mutation; removed only after confirmation or verified recovery.
    #[serde(deserialize_with = "Option::deserialize")]
    pub pending_recovery: Option<Layout>,
    #[serde(default)]
    pub pending_recovery_audio: Option<crate::AudioDefaults>,
    #[serde(default)]
    pub last_restorable_audio: Option<crate::AudioDefaults>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            profiles: Vec::new(),
            display_fingerprints: Vec::new(),
            last_known_good_layout: None,
            last_restorable_layout: None,
            settings: AppSettings::default(),
            pending_recovery: None,
            pending_recovery_audio: None,
            last_restorable_audio: None,
        }
    }
}

impl AppConfig {
    /// Supported means the current format and valid saved data, independent of
    /// which monitors happen to be connected when Monarch starts.
    pub fn is_supported(&self) -> bool {
        let mut names = HashSet::new();
        self.schema_version == CONFIG_SCHEMA_VERSION
            && (1..=60).contains(&self.settings.revert_timeout_secs)
            && self.profiles.iter().all(|profile| {
                !profile.name.trim().is_empty()
                    && names.insert(&profile.name)
                    && profile.layout.ensure_supported().is_ok()
                    && profile
                        .audio_output
                        .as_ref()
                        .is_none_or(crate::AudioOutput::is_valid)
            })
            && self
                .last_known_good_layout
                .iter()
                .chain(&self.last_restorable_layout)
                .chain(&self.pending_recovery)
                .all(|layout| layout.ensure_supported().is_ok())
            && self
                .pending_recovery_audio
                .iter()
                .chain(&self.last_restorable_audio)
                .all(crate::AudioDefaults::is_valid)
            && (self.pending_recovery_audio.is_none() || self.pending_recovery.is_some())
            && self
                .settings
                .display_toggle_shortcuts
                .keys()
                .all(|key| crate::identity::parse_display_key(key).is_ok())
    }
}

pub const CONFIG_SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Debug)]
pub struct DisplaySnapshot {
    pub generation: u64,
    pub displays: Vec<DisplayInfo>,
    pub layout: Layout,
}
