use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{AppConfig, ManagerError};

pub trait ConfigStore {
    fn load(&self) -> Result<AppConfig, ManagerError>;
    fn save(&self, config: &AppConfig) -> Result<(), ManagerError>;
    fn reset(&self) -> Result<(), ManagerError> {
        self.save(&AppConfig::default())
    }
}

#[derive(Clone, Debug)]
pub struct FileConfigStore {
    path: PathBuf,
}

impl FileConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn default_config_path() -> PathBuf {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("Monarch").join("config.json");
        }

        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("Monarch").join("config.json");
        }

        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("Monarch")
                .join("config.json");
        }

        PathBuf::from("config.json")
    }
}

impl Default for FileConfigStore {
    fn default() -> Self {
        Self::new(Self::default_config_path())
    }
}

impl ConfigStore for FileConfigStore {
    fn load(&self) -> Result<AppConfig, ManagerError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                if let Some(config) = serde_json::from_slice::<AppConfig>(&bytes)
                    .ok()
                    .filter(AppConfig::is_supported)
                {
                    return Ok(config);
                }
                if let Some(config) = migrate_v2(&bytes) {
                    atomic_write(&self.path.with_extension("json.v2.bak"), &bytes)?;
                    self.save(&config)?;
                    return Ok(config);
                }
                self.reset()?;
                Ok(AppConfig::default())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Removing the primary means a fresh start; never resurrect an old backup.
                self.reset()?;
                Ok(AppConfig::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn reset(&self) -> Result<(), ManagerError> {
        for path in [&self.path, &self.path.with_extension("json.bak")] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        let body = serde_json::to_vec_pretty(config)?;
        if let Ok(previous) = fs::read(&self.path) {
            // Backups only contain supported current-format configuration.
            if serde_json::from_slice::<AppConfig>(&previous)
                .is_ok_and(|config| config.is_supported())
            {
                atomic_write(&self.path.with_extension("json.bak"), &previous)?;
            }
        }
        atomic_write(&self.path, &body)?;
        Ok(())
    }
}

/// Replace a file without exposing a partial write to readers or the next process.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let (temp, mut file) = loop {
        let temp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            // A crashed process may have left a file under a subsequently reused PID.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xffff))
}

#[derive(Clone, Debug, Default)]
pub struct MemoryConfigStore {
    inner: Arc<Mutex<AppConfig>>,
}

impl MemoryConfigStore {
    pub fn new(config: AppConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(config)),
        }
    }

    pub fn snapshot(&self) -> Result<AppConfig, ManagerError> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| ManagerError::Backend("memory config store lock poisoned".to_string()))?;
        Ok(guard.clone())
    }
}

impl ConfigStore for MemoryConfigStore {
    fn load(&self) -> Result<AppConfig, ManagerError> {
        self.snapshot()
    }

    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ManagerError::Backend("memory config store lock poisoned".to_string()))?;
        *guard = config.clone();
        Ok(())
    }
}

/// Only a structurally valid v2 configuration is migrated. Serde still rejects
/// unknown fields and missing required old fields before compatible defaults apply.
fn migrate_v2(bytes: &[u8]) -> Option<AppConfig> {
    let mut config: AppConfig = serde_json::from_slice(bytes).ok()?;
    if config.schema_version != 2 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let settings = value.get("settings")?.as_object()?;
    if settings.contains_key("cursor_correction_enabled")
        || settings.contains_key("cursor_calibrations")
    {
        return None;
    }
    let layouts = value
        .get("profiles")?
        .as_array()?
        .iter()
        .filter_map(|p| p.get("layout"))
        .chain(
            [
                "last_known_good_layout",
                "last_restorable_layout",
                "pending_recovery",
            ]
            .into_iter()
            .filter_map(|key| value.get(key))
            .filter(|l| !l.is_null()),
        );
    for layout in layouts {
        for output in layout.get("outputs")?.as_array()? {
            let fields = output.as_object()?;
            if ["hdr_enabled", "scale_percent", "clone_group"]
                .iter()
                .any(|key| fields.contains_key(*key))
            {
                return None;
            }
        }
    }
    config.schema_version = crate::model::CONFIG_SCHEMA_VERSION;
    config.is_supported().then_some(config)
}
