use monarch::{
    DisplayBackend, DisplayId, DisplayInfo, Layout, MockBackend, OutputConfig, Position, Resolution,
};

#[cfg(target_os = "windows")]
pub mod windows;

pub enum SystemDisplayBackend {
    #[cfg(target_os = "windows")]
    Windows(windows::WindowsDisplayBackend),
    Mock(MockBackend),
}

impl SystemDisplayBackend {
    pub fn new() -> Result<Self, monarch::ManagerError> {
        #[cfg(target_os = "windows")]
        {
            if std::env::var_os("MONARCH_FORCE_MOCK_BACKEND").is_some() {
                return Ok(Self::Mock(build_mock_backend()?));
            }
            return Ok(Self::Windows(windows::WindowsDisplayBackend::new()?));
        }

        #[allow(unreachable_code)]
        Ok(Self::Mock(build_mock_backend()?))
    }
}

impl DisplayBackend for SystemDisplayBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, monarch::ManagerError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(backend) => backend.list_displays(),
            Self::Mock(backend) => backend.list_displays(),
        }
    }

    fn get_layout(&self) -> Result<Layout, monarch::ManagerError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(backend) => backend.get_layout(),
            Self::Mock(backend) => backend.get_layout(),
        }
    }

    fn apply_layout(&self, layout: Layout) -> Result<(), monarch::ManagerError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(backend) => backend.apply_layout(layout),
            Self::Mock(backend) => backend.apply_layout(layout),
        }
    }

    fn color_state_signature(&self) -> Result<Option<String>, monarch::ManagerError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(backend) => backend.color_state_signature(),
            Self::Mock(backend) => backend.color_state_signature(),
        }
    }

    fn reapply_color_calibration(&self) -> Result<(), monarch::ManagerError> {
        match self {
            #[cfg(target_os = "windows")]
            Self::Windows(backend) => backend.reapply_color_calibration(),
            Self::Mock(backend) => backend.reapply_color_calibration(),
        }
    }
}

fn build_mock_backend() -> Result<MockBackend, monarch::ManagerError> {
    let displays = vec![
        DisplayInfo {
            id: DisplayId {
                adapter_luid: 1,
                target_id: 1,
                edid_hash: Some(1),
            },
            friendly_name: "Primary Panel (Mock)".to_string(),
            is_active: true,
            is_primary: true,
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            refresh_rate_mhz: 60_000,
        },
        DisplayInfo {
            id: DisplayId {
                adapter_luid: 1,
                target_id: 2,
                edid_hash: Some(2),
            },
            friendly_name: "Side Display (Mock)".to_string(),
            is_active: true,
            is_primary: false,
            resolution: Resolution {
                width: 2560,
                height: 1440,
            },
            refresh_rate_mhz: 144_000,
        },
        DisplayInfo {
            id: DisplayId {
                adapter_luid: 1,
                target_id: 3,
                edid_hash: Some(3),
            },
            friendly_name: "Portrait Display (Mock)".to_string(),
            is_active: false,
            is_primary: false,
            resolution: Resolution {
                width: 1080,
                height: 1920,
            },
            refresh_rate_mhz: 60_000,
        },
    ];

    let layout = Layout {
        outputs: vec![
            OutputConfig {
                display_id: displays[0].id.clone(),
                enabled: true,
                position: Position { x: 0, y: 0 },
                resolution: displays[0].resolution.clone(),
                refresh_rate_mhz: displays[0].refresh_rate_mhz,
                primary: true,
            },
            OutputConfig {
                display_id: displays[1].id.clone(),
                enabled: true,
                position: Position { x: 1920, y: 0 },
                resolution: displays[1].resolution.clone(),
                refresh_rate_mhz: displays[1].refresh_rate_mhz,
                primary: false,
            },
            OutputConfig {
                display_id: displays[2].id.clone(),
                enabled: false,
                position: Position { x: -1080, y: 0 },
                resolution: displays[2].resolution.clone(),
                refresh_rate_mhz: displays[2].refresh_rate_mhz,
                primary: false,
            },
        ],
    };

    MockBackend::new(displays, layout)
}
