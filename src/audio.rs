use serde::{Deserialize, Serialize};

/// Endpoint IDs are opaque Windows identifiers. Names are labels, never identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
}

impl AudioOutput {
    pub fn is_valid(&self) -> bool {
        valid_id(&self.id) && !self.name.trim().is_empty()
    }
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && !id.contains('\0')
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AudioDevice {
    #[serde(flatten)]
    pub output: AudioOutput,
    pub available: bool,
}

/// None means leave that role to Windows. Capture all three roles for recovery;
/// ordinary output selection changes console and multimedia only.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioDefaults {
    pub console: Option<String>,
    pub multimedia: Option<String>,
    pub communications: Option<String>,
}

impl AudioDefaults {
    pub fn playback(id: &str) -> Self {
        Self {
            console: Some(id.into()),
            multimedia: Some(id.into()),
            communications: None,
        }
    }

    pub fn is_valid(&self) -> bool {
        [&self.console, &self.multimedia, &self.communications]
            .into_iter()
            .flatten()
            .all(|id| valid_id(id))
    }

    pub fn matches(&self, observed: &Self) -> bool {
        [
            (&self.console, &observed.console),
            (&self.multimedia, &observed.multimedia),
            (&self.communications, &observed.communications),
        ]
        .into_iter()
        .all(|(wanted, actual)| wanted.is_none() || wanted == actual)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AudioSnapshot {
    pub devices: Vec<AudioDevice>,
    pub defaults: AudioDefaults,
    pub unavailable_reason: Option<String>,
}

impl AudioSnapshot {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            unavailable_reason: Some(reason.into()),
            ..Self::default()
        }
    }
}
