use std::sync::{Arc, Mutex};

use crate::{DisplayInfo, Layout, ManagerError};

pub trait DisplayBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError>;
    fn get_layout(&self) -> Result<Layout, ManagerError>;
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError>;
    fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        Ok(None)
    }
    fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
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

#[derive(Default, Debug, Clone)]
pub struct Win32DisplayBackend;

impl DisplayBackend for Win32DisplayBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        Err(ManagerError::Backend(
            "Win32 backend not implemented in this skeleton".to_string(),
        ))
    }

    fn get_layout(&self) -> Result<Layout, ManagerError> {
        Err(ManagerError::Backend(
            "Win32 backend not implemented in this skeleton".to_string(),
        ))
    }

    fn apply_layout(&self, _layout: Layout) -> Result<(), ManagerError> {
        Err(ManagerError::Backend(
            "Win32 backend not implemented in this skeleton".to_string(),
        ))
    }

    fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        Ok(None)
    }

    fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        Err(ManagerError::Backend(
            "Win32 backend not implemented in this skeleton".to_string(),
        ))
    }
}
