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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardTheme {
    pub bg: Option<u32>,
    pub key: Option<u32>,
    pub accent: Option<u32>,
    pub text: Option<u32>,
    pub sel_text: Option<u32>,
    /// Key outline color (matches the webview VK border).
    pub border: Option<u32>,
}

/// Virtual-keyboard layout: docked along the screen edge, or floating near the caret
/// (emulating the warmUP webview keyboard). Pushed from the desktop `config.vkMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VkLayoutMode {
    #[default]
    Docked,
    Floating,
}

impl VkLayoutMode {
    pub fn as_str(self) -> &'static str {
        match self {
            VkLayoutMode::Docked => "docked",
            VkLayoutMode::Floating => "floating",
        }
    }
}

#[cfg(feature = "gamepad")]
pub fn parse_vk_layout_mode(raw: Option<&str>) -> VkLayoutMode {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("floating") => VkLayoutMode::Floating,
        _ => VkLayoutMode::Docked,
    }
}

#[cfg(feature = "gamepad")]
pub fn vk_layout_mode() -> VkLayoutMode {
    let raw = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (k, v) = line.split_once('=')?;
                (k.trim() == "vk_mode").then(|| v.trim().to_string())
            })
        });
    parse_vk_layout_mode(raw.as_deref())
}

#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug)]
pub struct GamepadSettings {
    pub userland_poll_mode: warmup_gamepad::PollMode,
    /// Standalone companion behavior: sleep the userland gamepad loop when another
    /// fullscreen game-like window is detected, even without warmUP IPC mode pushes.
    pub sleep_on_game: bool,
    pub cursor_deadzone: f32,
    pub cursor_speed: f32,
    pub cursor_accel: f32,
    pub scroll_deadzone: f32,
    pub scroll_speed: f32,
    pub scroll_accel: f32,
    /// Invert scroll direction (natural / reverse scrolling).
    pub natural_scroll: bool,
    /// Cursor movement smoothing (EMA factor), 0.0 (off) – 1.0 (max).
    pub cursor_smoothing: f32,
}

#[cfg(feature = "gamepad")]
impl Default for GamepadSettings {
    fn default() -> Self {
        Self {
            userland_poll_mode: warmup_gamepad::PollMode::Full,
            sleep_on_game: true,
            cursor_deadzone: 0.15,
            cursor_speed: 15.0,
            cursor_accel: 2.0,
            scroll_deadzone: 0.15,
            scroll_speed: 5.0,
            scroll_accel: 2.0,
            natural_scroll: false,
            cursor_smoothing: 0.0,
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

/// Winlogon "Press L3 to open keyboard" prompt overlay. User-facing, so default
/// ON in service mode; killed by `WARMUP_VK_PROMPT=0` or a disable sentinel file.
#[cfg(windows)]
pub fn prompt_overlay_enabled() -> bool {
    let off = std::env::var_os("WARMUP_VK_PROMPT").is_some_and(|v| v == "0")
        || std::path::Path::new(r"C:\ProgramData\WarmupVk\prompt.disabled").is_file();
    !off
}

#[cfg(not(windows))]
pub fn prompt_overlay_enabled() -> bool {
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
    if let Ok(raw) = std::env::var("WARMUP_VK_SLEEP_ON_GAME") {
        settings.sleep_on_game = parse_bool(&raw, settings.sleep_on_game);
    }

    settings
}

#[cfg(feature = "gamepad")]
pub fn keyboard_theme() -> KeyboardTheme {
    let mut theme = KeyboardTheme::default();
    if let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        apply_keyboard_theme_text(&mut theme, &text);
    }
    theme
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
            "sleep_on_game" | "game_sleep" | "sleep_when_game_active" => {
                settings.sleep_on_game = parse_bool(value, settings.sleep_on_game)
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
            "natural_scroll" => {
                settings.natural_scroll = parse_bool(value, settings.natural_scroll)
            }
            "cursor_smoothing" => {
                settings.cursor_smoothing = parse_unit_f32(value, settings.cursor_smoothing)
            }
            _ => {}
        }
    }
}

#[cfg(feature = "gamepad")]
fn apply_keyboard_theme_text(theme: &mut KeyboardTheme, text: &str) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(color) = parse_theme_color(value.trim()) else {
            continue;
        };
        match key.trim() {
            "keyboard_bg" | "keyboard_background" => theme.bg = Some(color),
            "keyboard_key" | "keyboard_key_bg" => theme.key = Some(color),
            "keyboard_accent" => theme.accent = Some(color),
            "keyboard_text" => theme.text = Some(color),
            "keyboard_sel_text" | "keyboard_selected_text" => theme.sel_text = Some(color),
            "keyboard_border" => theme.border = Some(color),
            _ => {}
        }
    }
}

#[cfg(feature = "gamepad")]
pub fn parse_theme_color(value: &str) -> Option<u32> {
    let v = value.trim();
    let hex = v
        .strip_prefix('#')
        .or_else(|| v.strip_prefix("0x"))
        .or_else(|| v.strip_prefix("0X"))
        .unwrap_or(v);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(hex, 16).ok()?;
    let r = (rgb >> 16) & 0xff;
    let g = (rgb >> 8) & 0xff;
    let b = rgb & 0xff;
    Some((b << 16) | (g << 8) | r)
}

#[cfg(feature = "gamepad")]
fn format_theme_color(colorref: u32) -> String {
    let r = colorref & 0xff;
    let g = (colorref >> 8) & 0xff;
    let b = (colorref >> 16) & 0xff;
    format!("#{r:02X}{g:02X}{b:02X}")
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
fn parse_bool(value: &str, fallback: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => fallback,
    }
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
pub fn set_keyboard_theme(theme: &KeyboardTheme) -> Result<(), String> {
    for (key, value) in [
        ("keyboard_bg", theme.bg),
        ("keyboard_key", theme.key),
        ("keyboard_accent", theme.accent),
        ("keyboard_text", theme.text),
        ("keyboard_sel_text", theme.sel_text),
        ("keyboard_border", theme.border),
    ] {
        if let Some(color) = value {
            set_gamepad_setting(key, &format_theme_color(color))?;
        }
    }
    Ok(())
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
        "cursor_deadzone" | "scroll_deadzone" | "cursor_smoothing" => value
            .parse::<f32>()
            .ok()
            .filter(|v| (0.0..0.95).contains(v))
            .map(|_| ())
            .ok_or_else(|| format!("{key} must be >= 0.0 and < 0.95")),
        "natural_scroll" => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "false" | "1" | "0" | "yes" | "no" | "on" | "off" => Ok(()),
            _ => Err("natural_scroll must be a boolean".to_string()),
        },
        "sleep_on_game" | "game_sleep" | "sleep_when_game_active" => {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" | "false" | "1" | "0" | "yes" | "no" | "on" | "off" => Ok(()),
                _ => Err("sleep_on_game must be a boolean".to_string()),
            }
        }
        "cursor_speed" | "cursor_accel" | "scroll_speed" | "scroll_accel" => value
            .parse::<f32>()
            .ok()
            .filter(|v| *v > 0.0)
            .map(|_| ())
            .ok_or_else(|| format!("{key} must be > 0.0")),
        "keyboard_bg"
        | "keyboard_background"
        | "keyboard_key"
        | "keyboard_key_bg"
        | "keyboard_accent"
        | "keyboard_text"
        | "keyboard_sel_text"
        | "keyboard_selected_text"
        | "keyboard_border" => parse_theme_color(value)
            .map(|_| ())
            .ok_or_else(|| format!("{key} must be a #RRGGBB color")),
        "vk_mode" => match value.trim().to_ascii_lowercase().as_str() {
            "docked" | "floating" => Ok(()),
            _ => Err("vk_mode must be docked or floating".to_string()),
        },
        _ => Err(format!("unknown setting: {key}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "gamepad")]
    #[test]
    fn theme_color_accepts_common_rgb_formats_as_colorref() {
        assert_eq!(parse_theme_color("#112233"), Some(0x00332211));
        assert_eq!(parse_theme_color("112233"), Some(0x00332211));
        assert_eq!(parse_theme_color("0x112233"), Some(0x00332211));
        assert_eq!(parse_theme_color("#123"), None);
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn keyboard_theme_text_accepts_aliases() {
        let mut theme = KeyboardTheme::default();
        apply_keyboard_theme_text(
            &mut theme,
            "
            keyboard_background=#010203
            keyboard_key_bg=#040506
            keyboard_accent=#070809
            keyboard_text=#0A0B0C
            keyboard_selected_text=#0D0E0F
            ",
        );
        assert_eq!(theme.bg, Some(0x00030201));
        assert_eq!(theme.key, Some(0x00060504));
        assert_eq!(theme.accent, Some(0x00090807));
        assert_eq!(theme.text, Some(0x000c0b0a));
        assert_eq!(theme.sel_text, Some(0x000f0e0d));
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn gamepad_settings_text_accepts_sleep_on_game_aliases() {
        let mut settings = GamepadSettings::default();
        apply_gamepad_settings_text(&mut settings, "sleep_on_game=false\n");
        assert!(!settings.sleep_on_game);

        apply_gamepad_settings_text(&mut settings, "game_sleep=on\n");
        assert!(settings.sleep_on_game);
    }
}
