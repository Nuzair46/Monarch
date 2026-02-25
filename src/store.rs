use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{AppConfig, ManagerError};

pub trait ConfigStore {
    fn load(&self) -> Result<AppConfig, ManagerError>;
    fn save(&self, config: &AppConfig) -> Result<(), ManagerError>;
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
            return PathBuf::from(appdata)
                .join("Monarch")
                .join("config.json");
        }

        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg)
                .join("Monarch")
                .join("config.json");
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
        if !self.path.exists() {
            return Ok(AppConfig::default());
        }

        let bytes = fs::read(&self.path)?;
        let config = serde_json::from_slice(&bytes)?;
        Ok(config)
    }

    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_vec_pretty(config)?;
        fs::write(&self.path, body)?;
        Ok(())
    }
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
