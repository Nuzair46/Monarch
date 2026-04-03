use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 512 * 1024;

fn log_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn diagnostics_log_path() -> PathBuf {
    let config = monarch::FileConfigStore::default_config_path();
    config
        .parent()
        .map(|parent| parent.join("diagnostics.log"))
        .unwrap_or_else(|| PathBuf::from("diagnostics.log"))
}

fn diagnostics_log_backup_path() -> PathBuf {
    let mut path = diagnostics_log_path();
    path.set_extension("log.1");
    path
}

fn rotate_if_needed(path: &PathBuf) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() < MAX_LOG_BYTES {
        return;
    }

    let backup = diagnostics_log_backup_path();
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(path, backup);
}

pub fn log(message: impl AsRef<str>) {
    let _guard = match log_lock().lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let path = diagnostics_log_path();
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }

    rotate_if_needed(&path);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let line = format!("[{timestamp}] {}\n", message.as_ref());

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
}
