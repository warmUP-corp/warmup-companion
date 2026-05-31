//! Runtime config seam.
//!
//! The one place that reads `WARMUP_VK_SERVICE`. Every site that used to inspect
//! the env var inline now calls [`service_mode`], so "are we the boot/service
//! worker?" has a single definition. Feeds the `vk_gate` decision input.

/// `WARMUP_VK_SERVICE` is set (boot/service worker path). The ONE env read for
/// service mode.
pub fn service_mode() -> bool {
    std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0")
}

#[cfg(feature = "gamepad")]
const USERLAND_POLL_FILE: &str = "userland-poll.mode";
#[cfg(feature = "gamepad")]
const SETTINGS_FILE: &str = "settings.ini";

#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug)]
pub struct GamepadSettings {
    pub userland_poll_mode: warmup_gamepad::PollMode,
    pub cursor_deadzone: f32,
    pub cursor_speed: f32,
    pub cursor_accel: f32,
    pub scroll_deadzone: f32,
    pub scroll_speed: f32,
    pub scroll_accel: f32,
}

#[cfg(feature = "gamepad")]
impl Default for GamepadSettings {
    fn default() -> Self {
        Self {
            userland_poll_mode: warmup_gamepad::PollMode::Full,
            cursor_deadzone: 0.15,
            cursor_speed: 15.0,
            cursor_accel: 2.0,
            scroll_deadzone: 0.15,
            scroll_speed: 5.0,
            scroll_accel: 2.0,
        }
    }
}

/// Winlogon debug overlay / hotkeys. Enabled only by installer debug flag.
#[cfg(windows)]
pub fn debug_ui_enabled() -> bool {
    std::env::var_os("WARMUP_VK_DEBUG_UI").is_some_and(|v| v != "0")
        || std::path::Path::new(r"C:\ProgramData\WarmupVk\debug-ui.enabled").is_file()
}

#[cfg(not(windows))]
pub fn debug_ui_enabled() -> bool {
    false
}

#[cfg(feature = "gamepad")]
pub fn userland_gamepad_poll_mode() -> warmup_gamepad::PollMode {
    gamepad_settings().userland_poll_mode
}

#[cfg(feature = "gamepad")]
pub fn gamepad_settings() -> GamepadSettings {
    let mut settings = GamepadSettings::default();
    if let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        apply_gamepad_settings_text(&mut settings, &text);
    }

    let raw = std::env::var("WARMUP_VK_USERLAND_POLL_MODE")
        .ok()
        .or_else(|| userland_gamepad_poll_mode_path().and_then(|p| std::fs::read_to_string(p).ok()))
        .or_else(|| std::fs::read_to_string(r"C:\ProgramData\WarmupVk\userland-poll.mode").ok());
    if raw.is_some() {
        settings.userland_poll_mode = parse_userland_gamepad_poll_mode(raw.as_deref());
    }

    settings
}

#[cfg(feature = "gamepad")]
fn apply_gamepad_settings_text(settings: &mut GamepadSettings, text: &str) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "userland_poll" | "userland_poll_mode" | "poll_mode" => {
                settings.userland_poll_mode = parse_userland_gamepad_poll_mode(Some(value));
            }
            "cursor_deadzone" => {
                settings.cursor_deadzone = parse_unit_f32(value, settings.cursor_deadzone)
            }
            "cursor_speed" => {
                settings.cursor_speed = parse_positive_f32(value, settings.cursor_speed)
            }
            "cursor_accel" => {
                settings.cursor_accel = parse_positive_f32(value, settings.cursor_accel)
            }
            "scroll_deadzone" => {
                settings.scroll_deadzone = parse_unit_f32(value, settings.scroll_deadzone)
            }
            "scroll_speed" => {
                settings.scroll_speed = parse_positive_f32(value, settings.scroll_speed)
            }
            "scroll_accel" => {
                settings.scroll_accel = parse_positive_f32(value, settings.scroll_accel)
            }
            _ => {}
        }
    }
}

#[cfg(feature = "gamepad")]
fn parse_unit_f32(value: &str, fallback: f32) -> f32 {
    value
        .parse::<f32>()
        .ok()
        .filter(|v| (0.0..0.95).contains(v))
        .unwrap_or(fallback)
}

#[cfg(feature = "gamepad")]
fn parse_positive_f32(value: &str, fallback: f32) -> f32 {
    value
        .parse::<f32>()
        .ok()
        .filter(|v| *v > 0.0)
        .unwrap_or(fallback)
}

#[cfg(feature = "gamepad")]
pub fn parse_userland_gamepad_poll_mode(raw: Option<&str>) -> warmup_gamepad::PollMode {
    match raw.map(str::trim).map(str::to_ascii_lowercase) {
        Some(v) if v == "sleep" || v == "guide" || v == "guide-only" => {
            warmup_gamepad::PollMode::Sleep
        }
        _ => warmup_gamepad::PollMode::Full,
    }
}

#[cfg(feature = "gamepad")]
pub fn userland_gamepad_poll_mode_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| {
        std::path::PathBuf::from(base)
            .join("WarmupVk")
            .join(USERLAND_POLL_FILE)
    })
}

#[cfg(feature = "gamepad")]
pub fn set_userland_gamepad_poll_mode(mode: warmup_gamepad::PollMode) -> Result<(), String> {
    set_gamepad_setting(
        "userland_poll",
        match mode {
            warmup_gamepad::PollMode::Full => "full",
            warmup_gamepad::PollMode::Sleep => "sleep",
        },
    )
}

#[cfg(feature = "gamepad")]
pub fn settings_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| {
        std::path::PathBuf::from(base)
            .join("WarmupVk")
            .join(SETTINGS_FILE)
    })
}

#[cfg(feature = "gamepad")]
pub fn set_gamepad_setting(key: &str, value: &str) -> Result<(), String> {
    validate_gamepad_setting(key, value)?;
    let path = settings_path()
        .ok_or_else(|| "LOCALAPPDATA is not set; cannot write settings".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create settings dir: {e}"))?;
    }
    let mut entries = std::collections::BTreeMap::<String, String>::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            entries.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    entries.insert(key.to_string(), value.to_string());
    let text = entries
        .into_iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect::<String>();
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(feature = "gamepad")]
fn validate_gamepad_setting(key: &str, value: &str) -> Result<(), String> {
    match key {
        "userland_poll" | "userland_poll_mode" | "poll_mode" => {
            let v = value.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "full" | "sleep" | "guide" | "guide-only") {
                Ok(())
            } else {
                Err("poll mode must be full or sleep".to_string())
            }
        }
        "cursor_deadzone" | "scroll_deadzone" => value
            .parse::<f32>()
            .ok()
            .filter(|v| (0.0..0.95).contains(v))
            .map(|_| ())
            .ok_or_else(|| format!("{key} must be >= 0.0 and < 0.95")),
        "cursor_speed" | "cursor_accel" | "scroll_speed" | "scroll_accel" => value
            .parse::<f32>()
            .ok()
            .filter(|v| *v > 0.0)
            .map(|_| ())
            .ok_or_else(|| format!("{key} must be > 0.0")),
        _ => Err(format!("unknown setting: {key}")),
    }
}
