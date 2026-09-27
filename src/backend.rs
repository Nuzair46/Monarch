use std::sync::{Arc, Mutex};

use crate::{DisplayInfo, DisplaySnapshot, Layout, ManagerError};

pub trait DisplayBackend {
    fn get_display_capabilities(
        &self,
    ) -> Result<Vec<crate::capabilities::DisplayCapabilities>, ManagerError> {
        Ok(Vec::new())
    }
    fn validate_layout(&self, _layout: &Layout) -> Result<(), ManagerError> {
        Ok(())
    }

    fn snapshot(&self) -> Result<DisplaySnapshot, ManagerError> {
        Ok(DisplaySnapshot {
            generation: 0,
            displays: self.list_displays()?,
            layout: self.get_layout()?,
        })
    }
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError>;
    fn get_layout(&self) -> Result<Layout, ManagerError>;
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError>;
    fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        Ok(None)
    }
    fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        Ok(())
    }
    /// Drop any cached display state so the next query rebuilds it from a fresh enumeration.
    /// Backends without a cache treat this as a no-op.
    fn invalidate_cache(&self) -> Result<(), ManagerError> {
        Ok(())
    }
    /// Refresh inventory or record diagnostics before rejecting unresolved outputs.
    /// This hook must not mutate the active topology.
    fn prepare_attach_targets(&self, _desired: &Layout) -> Result<(), ManagerError> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct MockBackend {
    state: Arc<Mutex<MockBackendState>>,
}

#[derive(Clone, Debug)]
struct MockBackendState {
    displays: Vec<DisplayInfo>,
    layout: Layout,
}

impl MockBackend {
    pub fn new(displays: Vec<DisplayInfo>, layout: Layout) -> Result<Self, ManagerError> {
        layout.ensure_valid()?;
        let mut state = MockBackendState { displays, layout };
        sync_displays_from_layout(&mut state);
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
        })
    }

    pub fn current_layout(&self) -> Result<Layout, ManagerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ManagerError::Backend("mock backend lock poisoned".to_string()))?;
        Ok(state.layout.clone())
    }
}

impl DisplayBackend for MockBackend {
    fn snapshot(&self) -> Result<DisplaySnapshot, ManagerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ManagerError::Backend("mock backend lock poisoned".into()))?;
        Ok(DisplaySnapshot {
            generation: 0,
            displays: state.displays.clone(),
            layout: state.layout.clone(),
        })
    }
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ManagerError::Backend("mock backend lock poisoned".to_string()))?;
        Ok(state.displays.clone())
    }

    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.current_layout()
    }

    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        layout.ensure_valid()?;

        let mut state = self
            .state
            .lock()
            .map_err(|_| ManagerError::Backend("mock backend lock poisoned".to_string()))?;
        state.layout = layout;
        sync_displays_from_layout(&mut state);
        Ok(())
    }

    fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        Ok(None)
    }

    fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        Ok(())
    }
}

fn sync_displays_from_layout(state: &mut MockBackendState) {
    for display in &mut state.displays {
        if let Some(output) = state
            .layout
            .outputs
            .iter()
            .find(|output| output.display_id == display.id)
        {
            display.is_active = output.enabled;
            display.is_primary = output.enabled && output.primary;
            display.resolution = output.resolution.clone();
            display.refresh_rate_mhz = output.refresh_rate_mhz;
        } else {
            display.is_active = false;
            display.is_primary = false;
        }
    }
}
