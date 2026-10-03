use std::path::{Path, PathBuf};

pub const LOG_NAME: &str = "service.log";
const SETTINGS_FILE: &str = "settings.ini";
const RUNTIME_STATUS_FILE: &str = "runtime-status.ini";
const USERLAND_POLL_FILE: &str = "userland-poll.mode";

#[cfg(windows)]
pub const DATA_DIR: &str = r"C:\ProgramData\WarmupVk";
#[cfg(windows)]
pub const INSTALL_DIR: &str = r"C:\ProgramData\WarmupVk\bin";
#[cfg(windows)]
pub const DEV_EXE_PATH: &str = r"C:\ProgramData\WarmupVk\warmup-exe.path";
#[cfg(windows)]
pub const DEBUG_UI_FLAG: &str = r"C:\ProgramData\WarmupVk\debug-ui.enabled";

pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(DATA_DIR)
    }
    #[cfg(target_os = "linux")]
    {
        xdg_base("XDG_DATA_HOME", ".local/share").join("warmup-companion")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        local_app_warmup().unwrap_or_else(|| PathBuf::from("WarmupVk"))
    }
}

pub fn install_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(INSTALL_DIR)
    }
    #[cfg(not(windows))]
    {
        data_dir().join("bin")
    }
}

pub fn state_dir() -> PathBuf {
    #[cfg(windows)]
    {
        data_dir()
    }
    #[cfg(target_os = "linux")]
    {
        xdg_base("XDG_STATE_HOME", ".local/state").join("warmup-companion")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        data_dir()
    }
}

pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        data_dir()
    }
    #[cfg(target_os = "linux")]
    {
        xdg_base("XDG_CONFIG_HOME", ".config").join("warmup-companion")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        data_dir()
    }
}

pub fn settings_file() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(data_dir().join(SETTINGS_FILE))
    }
    #[cfg(target_os = "linux")]
    {
        Some(config_dir().join(SETTINGS_FILE))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        local_app_warmup().map(|dir| dir.join(SETTINGS_FILE))
    }
}

pub fn runtime_status_file() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        local_app_warmup().map(|dir| dir.join(RUNTIME_STATUS_FILE))
    }
    #[cfg(target_os = "linux")]
    {
        Some(state_dir().join(RUNTIME_STATUS_FILE))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        local_app_warmup().map(|dir| dir.join(RUNTIME_STATUS_FILE))
    }
}

pub fn userland_poll_mode_file() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        local_app_warmup().map(|dir| dir.join(USERLAND_POLL_FILE))
    }
    #[cfg(target_os = "linux")]
    {
        Some(state_dir().join(USERLAND_POLL_FILE))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        local_app_warmup().map(|dir| dir.join(USERLAND_POLL_FILE))
    }
}

pub fn program_data_userland_poll_file() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(data_dir().join(USERLAND_POLL_FILE))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn log_file() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        state_dir().join(LOG_NAME)
    }
    #[cfg(not(target_os = "linux"))]
    {
        data_dir().join(LOG_NAME)
    }
}

pub fn dev_exe_file() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(DEV_EXE_PATH)
    }
    #[cfg(not(windows))]
    {
        data_dir().join("warmup-exe.path")
    }
}

pub fn mapping_db_file() -> PathBuf {
    if let Ok(path) = std::env::var("WARMUP_GAMECONTROLLER_DB") {
        return PathBuf::from(path);
    }
    let installed = data_dir().join("gamecontrollerdb.txt");
    if installed.is_file() {
        return installed;
    }
    PathBuf::from("gamecontrollerdb.txt")
}

pub fn library_watch_file() -> PathBuf {
    data_dir().join("library-watch.json")
}

pub fn parental_guard_file() -> PathBuf {
    data_dir().join("parental-guard.json")
}

pub fn pending_play_sessions_file() -> PathBuf {
    data_dir().join("pending-play-sessions.json")
}

pub fn personal_dict_file() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|base| {
            PathBuf::from(base)
                .join("WarmupKeyboard")
                .join("personal.dict")
        })
    }
    #[cfg(not(windows))]
    {
        Some(data_dir().join("personal.dict"))
    }
}

#[cfg(target_os = "linux")]
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("warmup");
        }
    }
    let uid = nix::unistd::Uid::effective().as_raw();
    PathBuf::from(format!("/tmp/warmup-{uid}"))
}

#[cfg(target_os = "linux")]
pub fn socket_file() -> PathBuf {
    runtime_dir().join("input.sock")
}

#[cfg(target_os = "linux")]
pub fn socket_lock_file() -> PathBuf {
    runtime_dir().join("companion.lock")
}

#[cfg(target_os = "linux")]
pub fn systemd_user_unit_file() -> PathBuf {
    xdg_base("XDG_CONFIG_HOME", ".config")
        .join("systemd")
        .join("user")
        .join("warmup-companion.service")
}

#[cfg(target_os = "linux")]
fn xdg_base(var: &str, fallback_rel: &str) -> PathBuf {
    resolve_xdg(std::env::var_os(var).as_deref(), &home_dir(), fallback_rel)
}

pub fn resolve_xdg(explicit: Option<&std::ffi::OsStr>, home: &Path, fallback_rel: &str) -> PathBuf {
    if let Some(value) = explicit {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    home.join(fallback_rel)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn local_app_warmup() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("WarmupVk"))
}

#[cfg(test)]
mod tests {
    use super::resolve_xdg;
    use std::path::Path;

    #[test]
    fn xdg_explicit_value_wins_over_home_fallback() {
        let home = Path::new("/home/player");
        let resolved = resolve_xdg(Some("/custom/config".as_ref()), home, ".config");
        assert_eq!(resolved, Path::new("/custom/config"));
    }

    #[test]
    fn xdg_empty_value_uses_home_fallback() {
        let home = Path::new("/home/player");
        let resolved = resolve_xdg(Some("".as_ref()), home, ".config");
        assert_eq!(resolved, Path::new("/home/player/.config"));
    }
}
