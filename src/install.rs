#[cfg(windows)]
#[path = "install_windows.rs"]
mod windows;

#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "linux")]
#[path = "install_linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::*;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn log_line(msg: &str) {
    let _ = log_line_inner(msg);
    eprintln!("> {msg}");
}

fn log_line_inner(msg: &str) -> std::io::Result<()> {
    let path = crate::paths::log_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    rotate_log_if_needed(&path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "[{}] {msg}", unix_timestamp())?;
    Ok(())
}

fn rotate_log_if_needed(path: &Path) -> std::io::Result<()> {
    if fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) < MAX_LOG_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    let _ = fs::remove_file(&rotated);
    fs::rename(path, rotated)?;
    Ok(())
}

fn unix_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
