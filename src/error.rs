use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ManagerError {
    Backend(String),
    ApplyRestored(String),
    /// Live enumeration could not identify these requested displays.
    DisplaysUnavailable(Vec<crate::DisplayId>),
    /// The same failure, with the previous desktop successfully restored.
    DisplaysUnavailableRestored(Vec<crate::DisplayId>),
    RecoveryRequired(String),
    Validation(String),
    NotFound(String),
    ConfirmationPending,
    NoPendingConfirmation,
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl Display for ManagerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
            Self::ApplyRestored(msg) => write!(f, "{msg}. The previous settings were restored."),
            Self::DisplaysUnavailable(ids) | Self::DisplaysUnavailableRestored(ids) => {
                write!(f, "validation error: display identity is missing or changed for targets {:?}; they may be in standby or not connected. Turn them on or reconnect them and refresh the inventory before applying", ids.iter().map(|id| id.target_id).collect::<Vec<_>>())?;
                if matches!(self, Self::DisplaysUnavailableRestored(_)) {
                    write!(f, ". The previous settings were restored.")?;
                }
                Ok(())
            }
            Self::RecoveryRequired(msg) => write!(f, "{msg}. Recovery is pending."),
            Self::Validation(msg) => write!(f, "validation error: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::ConfirmationPending => {
                write!(f, "a layout change is pending confirmation")
            }
            Self::NoPendingConfirmation => write!(f, "no pending confirmation"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
        }
    }
}

impl std::error::Error for ManagerError {}

impl From<std::io::Error> for ManagerError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ManagerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
