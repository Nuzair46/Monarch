use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ManagerError {
    Backend(String),
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
