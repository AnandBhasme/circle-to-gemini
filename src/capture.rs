// Screen capture via KDE Spectacle (Wayland-native)

use std::process::Command;
use tempfile::NamedTempFile;

/// Captures the current monitor's screen using Spectacle.
/// Returns the path to the temporary PNG file.
pub fn capture_current_monitor() -> Result<String, String> {
    let tmp = NamedTempFile::new()
        .map_err(|e| format!("Failed to create temp file: {e}"))?;
    let path = format!("{}.png", tmp.path().display());

    // Keep the tempfile handle alive so it doesn't get deleted
    // We use a separate path with .png extension
    let status = Command::new("spectacle")
        .args(["-m", "-b", "-n", "-o", &path])
        .status()
        .map_err(|e| format!("Failed to run spectacle: {e}"))?;

    if status.success() {
        Ok(path)
    } else {
        Err(format!("Spectacle exited with code: {:?}", status.code()))
    }
}

/// Captures all screens using Spectacle.
pub fn capture_full_desktop() -> Result<String, String> {
    let tmp = NamedTempFile::new()
        .map_err(|e| format!("Failed to create temp file: {e}"))?;
    let path = format!("{}.png", tmp.path().display());

    let status = Command::new("spectacle")
        .args(["-f", "-b", "-n", "-o", &path])
        .status()
        .map_err(|e| format!("Failed to run spectacle: {e}"))?;

    if status.success() {
        Ok(path)
    } else {
        Err(format!("Spectacle exited with code: {:?}", status.code()))
    }
}
