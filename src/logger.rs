// Circular rolling file logger with runtime toggle
// Files are rotated when exceeding MAX_LOG_FILE_SIZE (2 MB), keeping at most MAX_BACKUPS (3).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

const MAX_LOG_FILE_SIZE: u64 = 2 * 1024 * 1024; // 2 MB
const MAX_BACKUPS: usize = 3;

static LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);
static LOG_MUTEX: Mutex<()> = Mutex::new(());
static CUSTOM_LOG_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Set or clear a custom log directory (used in isolated unit tests)
pub fn set_custom_log_dir(path: Option<PathBuf>) {
    let mut lock = CUSTOM_LOG_DIR.lock().unwrap_or_else(|e| e.into_inner());
    *lock = path;
}

/// Determine the log directory: ~/.config/circle-to-gemini/logs/ or custom test dir
pub fn log_dir() -> PathBuf {
    if let Ok(guard) = CUSTOM_LOG_DIR.lock() {
        if let Some(ref custom) = *guard {
            return custom.clone();
        }
    }
    crate::config::Config::config_path()
        .parent()
        .map(|p| p.join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

/// Determine the primary log file path: ~/.config/circle-to-gemini/logs/app.log
pub fn log_file_path() -> PathBuf {
    log_dir().join("app.log")
}

/// Initialize the logger with the configured state
pub fn init(enabled: bool) {
    LOGGING_ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        let dir = log_dir();
        fs::create_dir_all(&dir).ok();
        log("INFO", "logger", "=== Session started (logging enabled) ===");
    }
}

/// Dynamically toggle logging on or off at runtime
pub fn set_enabled(enabled: bool) {
    let was_enabled = LOGGING_ENABLED.swap(enabled, Ordering::SeqCst);
    if enabled && !was_enabled {
        let dir = log_dir();
        fs::create_dir_all(&dir).ok();
        log("INFO", "logger", "Logging turned ON by user");
    } else if !enabled && was_enabled {
        log("INFO", "logger", "Logging turned OFF by user");
    }
}

/// Check if logging is currently enabled
pub fn is_enabled() -> bool {
    LOGGING_ENABLED.load(Ordering::Relaxed)
}

/// Clear all log files in the log directory
pub fn clear_logs() {
    let _lock = LOG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = log_dir();
    let primary = log_file_path();
    fs::remove_file(&primary).ok();
    for i in 1..=MAX_BACKUPS {
        let backup = dir.join(format!("app.log.{}", i));
        fs::remove_file(&backup).ok();
    }
}

/// Civil calendar time formatter using standard epoch seconds without external dependencies
fn current_timestamp() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    let days = secs / 86400;
    let time = secs % 86400;
    let hours = time / 3600;
    let minutes = (time % 3600) / 60;
    let seconds = time % 60;

    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        y, m, d, hours, minutes, seconds, millis
    )
}

/// Rotate log files when the active file exceeds MAX_LOG_FILE_SIZE
fn rotate_logs_if_needed(primary: &PathBuf, dir: &PathBuf) {
    if let Ok(metadata) = fs::metadata(primary) {
        if metadata.len() >= MAX_LOG_FILE_SIZE {
            // Delete oldest backup
            let oldest = dir.join(format!("app.log.{}", MAX_BACKUPS));
            fs::remove_file(&oldest).ok();

            // Shift backups: app.log.2 -> app.log.3, app.log.1 -> app.log.2
            for i in (1..MAX_BACKUPS).rev() {
                let src = dir.join(format!("app.log.{}", i));
                let dst = dir.join(format!("app.log.{}", i + 1));
                if src.exists() {
                    fs::rename(&src, &dst).ok();
                }
            }

            // Move primary to app.log.1
            let first_backup = dir.join("app.log.1");
            fs::rename(primary, &first_backup).ok();
        }
    }
}

/// Write a structured log entry to the circular log file and stderr
pub fn log(level: &str, target: &str, message: &str) {
    if !is_enabled() {
        return;
    }

    let timestamp = current_timestamp();
    let entry = format!("[{}] [{}] [{}] {}\n", timestamp, level, target, message);

    let _lock = LOG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    let dir = log_dir();
    let primary = log_file_path();

    rotate_logs_if_needed(&primary, &dir);

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&primary) {
        file.write_all(entry.as_bytes()).ok();
    }
}

#[macro_export]
macro_rules! log_info {
    ($target:expr, $($arg:tt)+) => {
        if $crate::logger::is_enabled() {
            $crate::logger::log("INFO", $target, &format!($($arg)+));
        }
    };
}

#[macro_export]
macro_rules! log_warn {
    ($target:expr, $($arg:tt)+) => {
        if $crate::logger::is_enabled() {
            $crate::logger::log("WARN", $target, &format!($($arg)+));
        }
    };
}

#[macro_export]
macro_rules! log_error {
    ($target:expr, $($arg:tt)+) => {
        if $crate::logger::is_enabled() {
            $crate::logger::log("ERROR", $target, &format!($($arg)+));
        }
    };
}

#[macro_export]
macro_rules! log_debug {
    ($target:expr, $($arg:tt)+) => {
        if $crate::logger::is_enabled() {
            $crate::logger::log("DEBUG", $target, &format!($($arg)+));
        }
    };
}
