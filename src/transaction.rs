//! One failure contract for native display transactions and their recovery journal.
use crate::ManagerError;
pub fn apply_with_recovery(
    apply: impl FnOnce() -> Result<(), ManagerError>,
    restore: impl FnOnce() -> Result<(), ManagerError>,
) -> Result<(), ManagerError> {
    match apply() {
        Ok(()) => Ok(()),
        Err(error) => match restore() {
            Ok(()) => Err(ManagerError::ApplyRestored(error.to_string())),
            Err(rollback) => Err(ManagerError::RecoveryRequired(format!(
                "{error}; restoring previous layout failed: {rollback}"
            ))),
        },
    }
}
