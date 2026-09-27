use super::audio_policy::PolicyConfig;
use monarch::{AudioDefaults, AudioDevice, AudioOutput, AudioSnapshot, ManagerError};
use std::time::{Duration, Instant};
use windows::core::{HRESULT, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, ERole, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, DEVICE_STATE, DEVICE_STATEMASK_ALL, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
    CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

struct Apartment(bool);
impl Apartment {
    fn new() -> windows::core::Result<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            result.ok()?;
            Ok(Self(true))
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

// Interfaces are local to the coordinator thread and released before COM teardown.
struct AudioSession {
    enumerator: IMMDeviceEnumerator,
    policy: PolicyConfig,
    _apartment: Apartment,
}

fn failure(operation: &str, error: windows::core::Error) -> ManagerError {
    crate::diagnostics::log(format!("audio:{operation}:failed:{error}"));
    ManagerError::Backend(format!(
        "Windows audio {operation} failed: {error}. Check Windows Sound settings and try again"
    ))
}

fn take_string(value: PWSTR) -> windows::core::Result<String> {
    unsafe {
        let result = value.to_string();
        CoTaskMemFree(Some(value.0.cast()));
        Ok(result?)
    }
}

fn endpoint_id(device: &IMMDevice) -> windows::core::Result<String> {
    take_string(unsafe { device.GetId()? })
}

fn endpoint_name(device: &IMMDevice) -> windows::core::Result<String> {
    unsafe {
        let properties = device.OpenPropertyStore(STGM_READ)?;
        let mut value = properties.GetValue(&PKEY_Device_FriendlyName)?;
        let result = PropVariantToStringAlloc(&value).and_then(take_string);
        let _ = PropVariantClear(&mut value);
        result
    }
}

impl AudioSession {
    fn new() -> Result<Self, ManagerError> {
        let apartment = Apartment::new().map_err(|e| failure("initialization", e))?;
        let enumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| failure("enumeration", e))?;
        let policy = PolicyConfig::new().map_err(|e| failure("output switching support", e))?;
        Ok(Self {
            enumerator,
            policy,
            _apartment: apartment,
        })
    }

    fn default_id(&self, role: ERole) -> windows::core::Result<Option<String>> {
        match unsafe { self.enumerator.GetDefaultAudioEndpoint(eRender, role) } {
            Ok(device) => endpoint_id(&device).map(Some),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn defaults(&self) -> Result<AudioDefaults, ManagerError> {
        Ok(AudioDefaults {
            console: self
                .default_id(eConsole)
                .map_err(|e| failure("default output query", e))?,
            multimedia: self
                .default_id(eMultimedia)
                .map_err(|e| failure("media output query", e))?,
            communications: self
                .default_id(eCommunications)
                .map_err(|e| failure("communications output query", e))?,
        })
    }

    fn devices(&self) -> Result<Vec<AudioDevice>, ManagerError> {
        let enumerate = || -> windows::core::Result<Vec<AudioDevice>> {
            unsafe {
                // Include inactive HDMI endpoints so profiles can select them before
                // enabling the associated display. Capture endpoints are excluded.
                let collection = self
                    .enumerator
                    .EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATEMASK_ALL))?;
                let mut devices = Vec::new();
                for index in 0..collection.GetCount()? {
                    let device = collection.Item(index)?;
                    let id = endpoint_id(&device)?;
                    let name = endpoint_name(&device)
                        .ok()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| id.clone());
                    devices.push(AudioDevice {
                        output: AudioOutput { id, name },
                        available: device.GetState()? == DEVICE_STATE_ACTIVE,
                    });
                }
                devices.sort_by(|a, b| {
                    a.output
                        .name
                        .cmp(&b.output.name)
                        .then(a.output.id.cmp(&b.output.id))
                });
                Ok(devices)
            }
        };
        enumerate().map_err(|e| failure("device list", e))
    }
}

pub fn snapshot() -> Result<AudioSnapshot, ManagerError> {
    let session = AudioSession::new()?;
    Ok(AudioSnapshot {
        devices: session.devices()?,
        defaults: session.defaults()?,
        unavailable_reason: None,
    })
}

pub fn set_defaults(defaults: &AudioDefaults) -> Result<(), ManagerError> {
    if !defaults.is_valid() {
        return Err(ManagerError::Validation("invalid audio endpoint ID".into()));
    }
    let roles = [
        (eConsole, &defaults.console),
        (eMultimedia, &defaults.multimedia),
        (eCommunications, &defaults.communications),
    ];
    if roles.iter().all(|(_, id)| id.is_none()) {
        return Ok(());
    }
    crate::diagnostics::log(format!("audio:defaults:requested:{defaults:?}"));
    let session = AudioSession::new()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let devices = session.devices()?;
        let missing = roles.iter().filter_map(|(_, id)| id.as_ref()).find(|id| {
            !devices
                .iter()
                .any(|device| &device.output.id == *id && device.available)
        });
        let Some(missing) = missing else {
            break;
        };
        if Instant::now() >= deadline {
            crate::diagnostics::log(format!("audio:endpoint:unavailable:{missing}"));
            let label = devices
                .iter()
                .find(|device| &device.output.id == missing)
                .map(|device| device.output.name.as_str())
                .unwrap_or(missing);
            return Err(ManagerError::Validation(format!("audio output '{label}' is unavailable. Turn on or reconnect the device, enable it in Windows Sound settings, or select another output for this profile")));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let observed = session.defaults()?;
    for (role, id) in roles {
        let Some(id) = id else {
            continue;
        };
        let current = match role {
            r if r == eConsole => &observed.console,
            r if r == eMultimedia => &observed.multimedia,
            _ => &observed.communications,
        };
        if current.as_ref() != Some(id) {
            session
                .policy
                .set_default(id, role)
                .map_err(|e| failure("output selection", e))?;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let observed = session.defaults()?;
        if defaults.matches(&observed) {
            crate::diagnostics::log("audio:defaults:verified");
            return Ok(());
        }
        if Instant::now() >= deadline {
            crate::diagnostics::log(format!("audio:defaults:verification-failed:{observed:?}"));
            return Err(ManagerError::Backend("Windows did not keep the selected audio output; check for another app changing the default device".into()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
