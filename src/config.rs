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
const RUNTIME_STATUS_FILE: &str = "runtime-status.ini";
#[cfg(all(windows, not(feature = "gamepad")))]
const PROMPT_USERLAND_DEBUG_FILE: &str = r"C:\ProgramData\WarmupVk\prompt-userland-debug.enabled";

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
    Docked,
    #[default]
    Floating,
}

#[cfg(feature = "gamepad")]
pub fn parse_vk_layout_mode(raw: Option<&str>) -> VkLayoutMode {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("docked") => VkLayoutMode::Docked,
        _ => VkLayoutMode::Floating,
    }
}

#[cfg(feature = "gamepad")]
pub(crate) fn raw_setting(key: &str) -> Option<String> {
    setting_value(&std::fs::read_to_string(settings_path()?).ok()?, key)
}

#[cfg(feature = "gamepad")]
fn setting_value(text: &str, key: &str) -> Option<String> {
    text.lines().rev().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

#[cfg(feature = "gamepad")]
pub fn vk_layout_mode() -> VkLayoutMode {
    parse_vk_layout_mode(raw_setting("vk_mode").as_deref())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VkStyle {
    #[default]
    Normal,
    Mono,
    Modern,
}

#[cfg(feature = "gamepad")]
pub fn parse_vk_style(raw: Option<&str>) -> VkStyle {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("mono" | "apple") => VkStyle::Mono,
        Some("modern" | "tv") => VkStyle::Modern,
        _ => VkStyle::Normal,
    }
}

#[cfg(feature = "gamepad")]
pub fn vk_style() -> VkStyle {
    parse_vk_style(raw_setting("vk_style").as_deref())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VkDisplay {
    #[default]
    Auto,
    Tv,
    Desk,
}

#[cfg(feature = "gamepad")]
pub fn parse_vk_display(raw: Option<&str>) -> VkDisplay {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("tv") => VkDisplay::Tv,
        Some("desk") => VkDisplay::Desk,
        _ => VkDisplay::Auto,
    }
}

#[cfg(feature = "gamepad")]
pub fn vk_display() -> VkDisplay {
    parse_vk_display(raw_setting("vk_display").as_deref())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunMode {
    #[default]
    Always,
    SignIn,
}

#[cfg(feature = "gamepad")]
pub fn parse_run_mode(raw: Option<&str>) -> RunMode {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("signin") => RunMode::SignIn,
        _ => RunMode::Always,
    }
}

#[cfg(feature = "gamepad")]
pub fn run_mode() -> RunMode {
    parse_run_mode(raw_setting("run_mode").as_deref())
}

#[cfg(feature = "gamepad")]
pub const COMPACT_BAR_SCALE: f32 = 0.8;

#[cfg(feature = "gamepad")]
pub fn vk_bar_scale() -> f32 {
    raw_setting("vk_bar_scale")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| (0.6..=1.2).contains(v))
        .unwrap_or(1.0)
}

#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug)]
pub struct GamepadSettings {
    pub userland_poll_mode: warmup_gamepad::PollMode,
    /// Standalone companion behavior: sleep the userland gamepad loop when another
    /// fullscreen game-like window is detected, even without warmUP IPC mode pushes.
    pub sleep_on_game: bool,
    /// Legacy standalone setting. It now selects Guide-only sleep so the PS button
    /// remains available instead of terminating the controller loop.
    pub auto_stop_on_game: bool,
    /// Show the userland prompt debug overlay.
    pub prompt_userland_debug: bool,
    pub signin_hints: bool,
    pub guide_launch: bool,
    pub update_check: bool,
    /// Offline voice typing. When false, R3, Ctrl+Alt+V and the mic key do nothing,
    /// the mic key on the keyboard shows disabled, and the speech engine is unloaded.
    pub voice_enabled: bool,
    pub vk_side_tips: bool,
    pub vk_sheet_opens: u32,
    /// Master switch for gamepad-driven mouse control ("Enable gamepad cursor" in
    /// warmUP, pushed as `config.enabled`). When false the sticks and the touchpad
    /// stop moving/scrolling the OS cursor and A/B stop emitting OS clicks — button
    /// edges still reach warmUP over the pipe, so D-pad/focus navigation keeps working.
    pub cursor_enabled: bool,
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
    pub touchpad_gestures: bool,
    pub touchpad_tap_click: bool,
}

#[cfg(feature = "gamepad")]
impl Default for GamepadSettings {
    fn default() -> Self {
        Self {
            userland_poll_mode: warmup_gamepad::PollMode::Full,
            sleep_on_game: true,
            auto_stop_on_game: false,
            prompt_userland_debug: false,
            signin_hints: true,
            guide_launch: true,
            update_check: true,
            voice_enabled: true,
            vk_side_tips: true,
            vk_sheet_opens: 0,
            cursor_enabled: true,
            cursor_deadzone: 0.15,
            cursor_speed: 15.0,
            cursor_accel: 2.0,
            scroll_deadzone: 0.15,
            scroll_speed: 5.0,
            scroll_accel: 2.0,
            natural_scroll: false,
            cursor_smoothing: 0.0,
            touchpad_gestures: true,
            touchpad_tap_click: true,
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

#[cfg(all(windows, feature = "gamepad"))]
pub fn prompt_userland_debug() -> bool {
    std::env::var_os("WARMUP_PROMPT_USERLAND_DEBUG").is_some_and(|v| v != "0")
        || gamepad_settings().prompt_userland_debug
}

#[cfg(all(windows, not(feature = "gamepad")))]
pub fn prompt_userland_debug() -> bool {
    std::env::var_os("WARMUP_PROMPT_USERLAND_DEBUG").is_some_and(|v| v != "0")
        || std::path::Path::new(r"C:\ProgramData\WarmupVk\prompt-userland-debug.enabled").is_file()
}

#[cfg(not(windows))]
pub fn prompt_overlay_enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn prompt_userland_debug() -> bool {
    false
}

/// Enable or disable the userland prompt debug overlay.
#[cfg(all(windows, feature = "gamepad"))]
pub fn set_prompt_userland_debug(enabled: bool) -> Result<(), String> {
    set_gamepad_setting(
        "prompt_userland_debug",
        if enabled { "true" } else { "false" },
    )
}

#[cfg(all(windows, not(feature = "gamepad")))]
pub fn set_prompt_userland_debug(enabled: bool) -> Result<(), String> {
    let path = std::path::Path::new(PROMPT_USERLAND_DEBUG_FILE);
    if enabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create prompt debug dir: {e}"))?;
        }
        std::fs::write(path, "enabled\n").map_err(|e| format!("write prompt debug sentinel: {e}"))
    } else if path.is_file() {
        std::fs::remove_file(path).map_err(|e| format!("remove prompt debug sentinel: {e}"))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn set_prompt_userland_debug(_enabled: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "gamepad")]
pub fn runtime_status_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| {
        std::path::PathBuf::from(base)
            .join("WarmupVk")
            .join(RUNTIME_STATUS_FILE)
    })
}

#[cfg(feature = "gamepad")]
pub fn write_userland_poll_paused(paused: bool) -> Result<(), String> {
    let path = runtime_status_path()
        .ok_or_else(|| "LOCALAPPDATA is not set; cannot write runtime status".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create runtime status dir: {e}"))?;
    }
    let text = format!(
        "userland_poll_paused={}\n",
        if paused { "true" } else { "false" }
    );
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(feature = "gamepad")]
pub fn read_userland_poll_paused() -> bool {
    let Some(path) = runtime_status_path() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.lines()
        .find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == "userland_poll_paused").then(|| parse_bool(v, false))
        })
        .unwrap_or(false)
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
    if let Ok(raw) = std::env::var("WARMUP_VK_AUTO_STOP_ON_GAME") {
        settings.auto_stop_on_game = parse_bool(&raw, settings.auto_stop_on_game);
    }

    settings
}

/// Offline voice typing. Default on, so an install that never set this keeps
/// dictating. False shuts transcription off completely.
pub fn voice_enabled() -> bool {
    #[cfg(feature = "gamepad")]
    {
        gamepad_settings().voice_enabled
    }
    #[cfg(not(feature = "gamepad"))]
    {
        true
    }
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
    let has_cursor_enabled = text.lines().any(|line| {
        let line = line.trim();
        !line.starts_with('#')
            && line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "cursor_enabled")
    });
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
            "auto_stop_on_game" | "stop_on_game" | "stop_when_game_active" => {
                settings.auto_stop_on_game = parse_bool(value, settings.auto_stop_on_game)
            }
            "prompt_userland_debug" => {
                settings.prompt_userland_debug = parse_bool(value, settings.prompt_userland_debug)
            }
            "signin_hints" => settings.signin_hints = parse_bool(value, settings.signin_hints),
            "guide_launch" => settings.guide_launch = parse_bool(value, settings.guide_launch),
            "update_check" => settings.update_check = parse_bool(value, settings.update_check),
            "voice_enabled" => settings.voice_enabled = parse_bool(value, settings.voice_enabled),
            "vk_side_tips" => settings.vk_side_tips = parse_bool(value, settings.vk_side_tips),
            "vk_sheet_opens" => {
                settings.vk_sheet_opens = value.parse().unwrap_or(settings.vk_sheet_opens)
            }
            "cursor_enabled" => {
                settings.cursor_enabled = parse_bool(value, settings.cursor_enabled)
            }
            "gamepad_cursor" if !has_cursor_enabled => {
                settings.cursor_enabled = parse_bool(value, settings.cursor_enabled)
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
            "touchpad_gestures" => {
                settings.touchpad_gestures = parse_bool(value, settings.touchpad_gestures)
            }
            "touchpad_tap_click" => {
                settings.touchpad_tap_click = parse_bool(value, settings.touchpad_tap_click)
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
    // Fixed, machine-wide location so the session-0 SYSTEM service (which handles
    // the IPC config push AND renders the Winlogon prompts) always resolves the
    // SAME file. `%LOCALAPPDATA%` is unreliable under LocalSystem — often unset or
    // pointing at systemprofile — so theme/cursor pushes never met the reader.
    // The installer grants Users read/write on this one file (the dir stays locked
    // to SYSTEM+Admins), so the tray and desktop app can still read/edit it.
    #[cfg(windows)]
    {
        Some(std::path::PathBuf::from(r"C:\ProgramData\WarmupVk").join(SETTINGS_FILE))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("LOCALAPPDATA").map(|base| {
            std::path::PathBuf::from(base)
                .join("WarmupVk")
                .join(SETTINGS_FILE)
        })
    }
}

/// Commented `settings.ini` template. Each value line is commented and free of
/// inline comments, so uncommenting one stays parseable. Keep keys in sync with
/// `validate_gamepad_setting`.
#[cfg(feature = "gamepad")]
const SETTINGS_TEMPLATE: &str = r#"# Warmup Companion settings. One `key = value` per line; `#` lines are ignored.
# Uncomment a line, change the value, save, then re-open the keyboard.
# Cursor/scroll/theme are normally set in the warmUP desktop app and pushed over
# IPC; values set here apply until the next push.

# When the companion runs: always | signin. signin = only on the sign-in and lock
# screen; after you sign in or unlock the companion sleeps and wakes again on lock.
# run_mode = always

# Gamepad poll mode: full | sleep
# userland_poll = full

# Use Guide-only gamepad polling when a fullscreen game is foreground (true|false)
# sleep_on_game = true
# Legacy alias for sleep_on_game; never terminates the loop.
# auto_stop_on_game = false
# prompt_userland_debug = false

# Show the controller hints on the sign-in screen: "Connect controller", "Press ... for keyboard", "Connected" (true|false)
# signin_hints = true

# Guide (Xbox) / PS button opens warmUP when it is closed (true|false)
# guide_launch = true

# Check GitHub once a day for a newer Warmup Companion release (true|false)
# update_check = true

# Gamepad cursor master switch (true|false). False parks stick/touchpad cursor
# movement, scrolling and A/B OS clicks; the pad still navigates warmUP via d-pad.
# cursor_enabled = true

# Cursor: deadzone & smoothing are 0.0-0.95; speed & accel are > 0.0
# cursor_deadzone = 0.15
# cursor_speed = 15.0
# cursor_accel = 2.0
# cursor_smoothing = 0.0

# Scroll: deadzone 0.0-0.95; speed & accel > 0.0; natural_scroll true|false
# scroll_deadzone = 0.15
# scroll_speed = 5.0
# scroll_accel = 1.0
# natural_scroll = false

# Touchpad (DualSense/DS4) on the desktop. touchpad_gestures: two-finger scroll,
# two-finger tap = right click, pad click on the right third = right click, and
# pad click held + swipe: left/right = switch desktop, up = Task View, down = show
# desktop. touchpad_tap_click: tap = left click, tap then drag = drag. (true|false)
# touchpad_gestures = true
# touchpad_tap_click = true

# On-screen keyboard layout: docked | floating
# vk_mode = docked
# Keyboard size: 0.6 - 1.2 (0.8 = compact)
# vk_bar_scale = 1.0

# Keyboard theme colors, #RRGGBB
# keyboard_bg = #101010
# keyboard_key = #202020
# keyboard_accent = #4C7B99
# keyboard_text = #FFFFFF
# keyboard_sel_text = #FFFFFF
# keyboard_border = #333333

# Voice typing (offline whisper or Parakeet) is an opt-in install (-Speech).
# voice_enabled = false turns transcription off completely: R3, Ctrl+Alt+V and
# the mic key do nothing, the mic key on the keyboard shows disabled, and the
# speech engine unloads.
# voice_enabled = true
# Recognition language defaults to your Windows locale; override with the
# WARMUP_WHISPER_LANG environment variable. Mic = your Windows default input device.
# Dictation vocabulary: coding (built-in terminal/agent terms) | off | a comma list, e.g. coding, herdr, MyProject
# vocabulary = coding
"#;

/// Path to `settings.ini`, creating it with the documented template if missing
/// so the tray "Edit settings" item always opens a populated, self-describing file.
#[cfg(feature = "gamepad")]
pub fn ensure_settings_file() -> Option<std::path::PathBuf> {
    let path = settings_path()?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, SETTINGS_TEMPLATE);
    }
    Some(path)
}

#[cfg(feature = "gamepad")]
const KEEP_ON_RESET: &[&str] = &[
    "vk_sheet_opens",
    "keyboard_bg",
    "keyboard_background",
    "keyboard_key",
    "keyboard_key_bg",
    "keyboard_accent",
    "keyboard_text",
    "keyboard_sel_text",
    "keyboard_selected_text",
    "keyboard_border",
];

#[cfg(feature = "gamepad")]
pub fn settings_backup_name(y: u16, mo: u16, d: u16, h: u16, mi: u16, sec: u16) -> String {
    format!("{SETTINGS_FILE}.bak-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{sec:02}")
}

#[cfg(feature = "gamepad")]
pub fn shipped_settings_text(current: &str) -> String {
    let mut out = SETTINGS_TEMPLATE.to_string();
    let mut kept = Vec::new();
    for key in KEEP_ON_RESET {
        if let Some(v) = setting_value(current, key) {
            kept.push(format!("{key}={v}"));
        }
    }
    let words = parse_vocabulary(Some(current)).words;
    if !words.is_empty() {
        let vocab = Vocabulary {
            coding: parse_vocabulary(None).coding,
            words,
        };
        kept.push(format!("vocabulary={}", format_vocabulary(&vocab)));
    }
    if !kept.is_empty() {
        out.push_str("\n# Kept across reset (keyboard theme, hint counter, custom words)\n");
        for line in kept {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

#[cfg(feature = "gamepad")]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub accepted: Vec<(String, String)>,
    pub rejected: Vec<String>,
}

#[cfg(feature = "gamepad")]
pub fn parse_settings_import(text: &str) -> ImportPlan {
    let mut plan = ImportPlan::default();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            plan.rejected.push(line.to_string());
            continue;
        };
        let (key, value) = (k.trim(), v.trim());
        if key == "vk_sheet_opens" || validate_gamepad_setting(key, value).is_err() {
            plan.rejected.push(line.to_string());
            continue;
        }
        plan.accepted.retain(|(existing, _)| existing != key);
        plan.accepted.push((key.to_string(), value.to_string()));
    }
    plan
}

#[cfg(all(feature = "gamepad", windows))]
fn backup_name_now() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    settings_backup_name(t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(all(feature = "gamepad", windows))]
pub fn backup_settings() -> Result<std::path::PathBuf, String> {
    let path = settings_path().ok_or_else(|| "settings path unavailable".to_string())?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let backup = path.with_file_name(backup_name_now());
    std::fs::write(&backup, &current).map_err(|e| format!("write {}: {e}", backup.display()))?;
    Ok(backup)
}

#[cfg(feature = "gamepad")]
pub fn export_settings_to(path: &std::path::Path) -> Result<(), String> {
    let src = settings_path().ok_or_else(|| "settings path unavailable".to_string())?;
    let text = std::fs::read_to_string(&src).unwrap_or_else(|_| SETTINGS_TEMPLATE.to_string());
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(all(feature = "gamepad", windows))]
pub fn reset_settings_to_shipped() -> Result<std::path::PathBuf, String> {
    let path = settings_path().ok_or_else(|| "settings path unavailable".to_string())?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let backup = backup_settings()?;
    let tmp = path.with_extension("ini.tmp");
    std::fs::write(&tmp, shipped_settings_text(&current))
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename to {}: {e}", path.display()))?;
    Ok(backup)
}

#[cfg(feature = "gamepad")]
pub fn set_gamepad_setting(key: &str, value: &str) -> Result<(), String> {
    validate_gamepad_setting(key, value)?;
    let path = settings_path()
        .ok_or_else(|| "LOCALAPPDATA is not set; cannot write settings".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create settings dir: {e}"))?;
    }
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let text = upsert_setting_line(&current, key, value);
    let tmp = path.with_extension("ini.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename to {}: {e}", path.display()))?;
    // Turning voice off must unload the engine now, not on the next R3 press.
    #[cfg(windows)]
    if key == "voice_enabled" {
        crate::win::speech_input::enforce_voice_enabled();
    }
    Ok(())
}

#[cfg(feature = "gamepad")]
fn upsert_setting_line(text: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 2);
    for line in text.lines() {
        let trimmed = line.trim_start();
        let matches = !trimmed.starts_with('#')
            && trimmed
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == key);
        if matches {
            found = true;
            out.push_str(&format!("{key}={value}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        out.push_str(&format!("{key}={value}\n"));
    }
    out
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
        "natural_scroll"
        | "touchpad_gestures"
        | "touchpad_tap_click"
        | "cursor_enabled"
        | "gamepad_cursor"
        | "sleep_on_game"
        | "game_sleep"
        | "sleep_when_game_active"
        | "auto_stop_on_game"
        | "stop_on_game"
        | "stop_when_game_active"
        | "prompt_userland_debug"
        | "signin_hints"
        | "guide_launch"
        | "update_check"
        | "voice_enabled"
        | "vk_side_tips" => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "false" | "1" | "0" | "yes" | "no" | "on" | "off" => Ok(()),
            _ => Err(format!("{key} must be a boolean")),
        },
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
        "led_color" => parse_theme_color(value)
            .map(|_| ())
            .ok_or_else(|| "led_color must be a #RRGGBB color".to_string()),
        "led_effect" => match value.trim().to_ascii_lowercase().as_str() {
            "solid" | "breathing" | "rainbow" | "gradient" | "off" => Ok(()),
            _ => Err("led_effect must be solid, breathing, rainbow, gradient or off".to_string()),
        },
        "vk_mode" => match value.trim().to_ascii_lowercase().as_str() {
            "docked" | "floating" => Ok(()),
            _ => Err("vk_mode must be docked or floating".to_string()),
        },
        "vk_sheet_opens" => value
            .trim()
            .parse::<u32>()
            .map(|_| ())
            .map_err(|_| "vk_sheet_opens must be a whole number".to_string()),
        "vk_style" => match value.trim().to_ascii_lowercase().as_str() {
            "normal" | "mono" | "refined" | "apple" | "modern" | "tv" => Ok(()),
            _ => Err("vk_style must be normal, mono, refined, apple, modern or tv".to_string()),
        },
        "vk_display" => match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "tv" | "desk" => Ok(()),
            _ => Err("vk_display must be auto, tv or desk".to_string()),
        },
        "vk_bar_scale" => value
            .parse::<f32>()
            .ok()
            .filter(|v| (0.6..=1.2).contains(v))
            .map(|_| ())
            .ok_or_else(|| "vk_bar_scale must be between 0.6 and 1.2".to_string()),
        "run_mode" => match value.trim().to_ascii_lowercase().as_str() {
            "always" | "signin" => Ok(()),
            _ => Err("run_mode must be always or signin".to_string()),
        },
        "vocabulary" => {
            if value.len() > 2000 {
                Err("vocabulary must be at most 2000 characters".to_string())
            } else if value.contains(['\n', '\r', '=']) {
                Err("vocabulary must not contain newlines or '='".to_string())
            } else {
                Ok(())
            }
        }
        _ => Err(format!("unknown setting: {key}")),
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Vocabulary {
    pub coding: bool,
    pub words: Vec<String>,
}

pub fn parse_vocabulary_list(raw: &str) -> Vocabulary {
    let mut vocab = Vocabulary::default();
    for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "coding" => vocab.coding = true,
            "off" | "none" => {}
            _ if vocab.words.iter().any(|w| w.eq_ignore_ascii_case(token)) => {}
            _ => vocab.words.push(token.to_string()),
        }
    }
    vocab
}

pub fn parse_vocabulary(settings: Option<&str>) -> Vocabulary {
    let raw = settings.and_then(|text| {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == "vocabulary").then(|| v.trim())
        })
    });
    match raw {
        Some(raw) => parse_vocabulary_list(raw),
        None => Vocabulary {
            coding: true,
            words: Vec::new(),
        },
    }
}

pub fn format_vocabulary(vocab: &Vocabulary) -> String {
    let parts: Vec<&str> = vocab
        .coding
        .then_some("coding")
        .into_iter()
        .chain(vocab.words.iter().map(String::as_str))
        .collect();
    if parts.is_empty() {
        "off".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn vocabulary() -> Vocabulary {
    let text = settings_path().and_then(|p| std::fs::read_to_string(p).ok());
    parse_vocabulary(text.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "gamepad")]
    #[test]
    fn settings_import_keeps_only_valid_known_keys() {
        let plan = parse_settings_import(
            "\u{feff}# exported\n\ncursor_speed = 20\ncursor_deadzone=2.0\nbogus_key=1\n\
             no equals here\nvk_style=mono\nvk_sheet_opens=9\nkeyboard_accent=#FF0000\n\
             led_effect=breathing\ncursor_speed=25\n  # indented comment\n",
        );
        assert_eq!(
            plan.accepted,
            vec![
                ("vk_style".to_string(), "mono".to_string()),
                ("keyboard_accent".to_string(), "#FF0000".to_string()),
                ("led_effect".to_string(), "breathing".to_string()),
                ("cursor_speed".to_string(), "25".to_string()),
            ]
        );
        assert_eq!(
            plan.rejected,
            vec![
                "cursor_deadzone=2.0".to_string(),
                "bogus_key=1".to_string(),
                "no equals here".to_string(),
                "vk_sheet_opens=9".to_string(),
            ]
        );
        assert!(parse_settings_import("; ini comment\n;cursor_speed=9\n")
            .accepted
            .is_empty());
        assert!(parse_settings_import("# only comments\n\n")
            .accepted
            .is_empty());
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn touchpad_settings_default_on_and_validate() {
        let mut s = GamepadSettings::default();
        assert!(s.touchpad_gestures && s.touchpad_tap_click);
        apply_gamepad_settings_text(&mut s, "touchpad_gestures=false
touchpad_tap_click=off
");
        assert!(!s.touchpad_gestures && !s.touchpad_tap_click);
        assert!(validate_gamepad_setting("touchpad_gestures", "true").is_ok());
        assert!(validate_gamepad_setting("touchpad_tap_click", "maybe").is_err());
        assert!(SETTINGS_TEMPLATE.contains("# touchpad_gestures = true"));
        assert!(SETTINGS_TEMPLATE.contains("# touchpad_tap_click = true"));
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn backup_names_sort_by_time() {
        assert_eq!(
            settings_backup_name(2026, 10, 7, 23, 5, 9),
            "settings.ini.bak-20261007-230509"
        );
        assert!(
            settings_backup_name(2026, 1, 2, 3, 4, 5) < settings_backup_name(2026, 1, 2, 3, 4, 6)
        );
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn shipped_reset_drops_user_settings_but_keeps_required_state() {
        let current = "cursor_speed=33\nvk_style=modern\nled_color=#FF3B30\nrun_mode=signin\n\
                       vk_sheet_opens=3\nkeyboard_accent=#FF0000\nvocabulary=off, herdr, MyProject\n";
        let text = shipped_settings_text(current);
        assert!(text.starts_with(SETTINGS_TEMPLATE));
        for gone in ["cursor_speed", "vk_style", "led_color", "run_mode"] {
            assert_eq!(setting_value(&text, gone), None, "{gone}");
        }
        assert_eq!(setting_value(&text, "vk_sheet_opens").as_deref(), Some("3"));
        assert_eq!(
            setting_value(&text, "keyboard_accent").as_deref(),
            Some("#FF0000")
        );
        let vocab = parse_vocabulary(Some(&text));
        assert_eq!(vocab.coding, parse_vocabulary(None).coding);
        assert_eq!(
            vocab.words,
            vec!["herdr".to_string(), "MyProject".to_string()]
        );
        let mut settings = GamepadSettings::default();
        apply_gamepad_settings_text(&mut settings, &text);
        assert_eq!(
            settings.cursor_speed,
            GamepadSettings::default().cursor_speed
        );
        assert_eq!(
            parse_run_mode(setting_value(&text, "run_mode").as_deref()),
            RunMode::Always
        );
        let plain = shipped_settings_text("cursor_speed=33\n");
        assert_eq!(plain, SETTINGS_TEMPLATE);
    }

    #[test]
    fn vocabulary_round_trips_through_settings_value() {
        for raw in ["coding", "off", "herdr, MyProject", "coding, herdr, Jonas"] {
            let v = parse_vocabulary(Some(&format!("vocabulary = {raw}")));
            assert_eq!(format_vocabulary(&v), raw);
        }
        assert_eq!(format_vocabulary(&parse_vocabulary(None)), "coding");
        assert_eq!(
            format_vocabulary(&parse_vocabulary(Some("vocabulary = none"))),
            "off"
        );
        assert_eq!(
            format_vocabulary(&parse_vocabulary(Some("vocabulary ="))),
            "off"
        );
        assert_eq!(
            format_vocabulary(&parse_vocabulary(Some("vocabulary = herdr ,coding,HERDR"))),
            "coding, herdr"
        );
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn vocabulary_setting_rejects_line_breaks_and_equals() {
        assert!(validate_gamepad_setting("vocabulary", "coding, herdr").is_ok());
        assert!(validate_gamepad_setting("vocabulary", "off").is_ok());
        assert!(validate_gamepad_setting("vocabulary", "a\nb").is_err());
        assert!(validate_gamepad_setting("vocabulary", "a=b").is_err());
        assert!(validate_gamepad_setting("vocabulary", &"x".repeat(2001)).is_err());
    }

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
    fn vk_layout_defaults_to_floating() {
        assert_eq!(parse_vk_layout_mode(None), VkLayoutMode::Floating);
        assert_eq!(parse_vk_layout_mode(Some("docked")), VkLayoutMode::Docked);
        assert_eq!(
            parse_vk_layout_mode(Some(" Floating ")),
            VkLayoutMode::Floating
        );
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn setting_value_last_line_wins() {
        let text = "vk_style=mono\n# vk_style = normal\nvk_style = modern\n";
        assert_eq!(setting_value(text, "vk_style").as_deref(), Some("modern"));
        assert_eq!(setting_value(text, "vk_mode"), None);
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn upsert_setting_line_preserves_unrelated_lines() {
        let text = "# header\n# vk_mode = docked\nvk_mode = floating\n\nvoice_enabled=true\n";
        assert_eq!(
            upsert_setting_line(text, "vk_mode", "docked"),
            "# header\n# vk_mode = docked\nvk_mode=docked\n\nvoice_enabled=true\n"
        );
        assert_eq!(
            upsert_setting_line(text, "vk_style", "modern"),
            format!("{text}vk_style=modern\n")
        );
        assert_eq!(upsert_setting_line("", "a", "1"), "a=1\n");
        assert_eq!(upsert_setting_line("x=1", "x", "2"), "x=2\n");
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn vk_display_parses_and_validates() {
        assert_eq!(parse_vk_display(None), VkDisplay::Auto);
        assert_eq!(parse_vk_display(Some("auto")), VkDisplay::Auto);
        assert_eq!(parse_vk_display(Some(" TV ")), VkDisplay::Tv);
        assert_eq!(parse_vk_display(Some("Desk")), VkDisplay::Desk);
        assert_eq!(parse_vk_display(Some("bogus")), VkDisplay::Auto);
        assert!(validate_gamepad_setting("led_color", "#B6A0FF").is_ok());
        assert!(validate_gamepad_setting("led_color", "lavender").is_err());
        assert!(validate_gamepad_setting("led_effect", "Breathing").is_ok());
        assert!(validate_gamepad_setting("led_effect", "strobe").is_err());
        assert!(validate_gamepad_setting("vk_display", "tv").is_ok());
        assert!(validate_gamepad_setting("vk_display", "Desk").is_ok());
        assert!(validate_gamepad_setting("vk_display", "auto").is_ok());
        assert!(validate_gamepad_setting("vk_display", "big").is_err());
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn vk_style_defaults_to_normal() {
        assert_eq!(parse_vk_style(None), VkStyle::Normal);
        assert_eq!(parse_vk_style(Some("normal")), VkStyle::Normal);
        assert_eq!(parse_vk_style(Some(" Mono ")), VkStyle::Mono);
        assert_eq!(parse_vk_style(Some("bogus")), VkStyle::Normal);
        assert!(validate_gamepad_setting("vk_style", "mono").is_ok());
        assert_eq!(parse_vk_style(Some("apple")), VkStyle::Mono);
        assert!(validate_gamepad_setting("vk_style", "refined").is_ok());
        assert!(validate_gamepad_setting("vk_style", "Normal").is_ok());
        assert!(validate_gamepad_setting("vk_style", "ios").is_err());
        assert_eq!(parse_vk_style(Some("Modern")), VkStyle::Modern);
        assert!(validate_gamepad_setting("vk_style", "modern").is_ok());
        assert!(validate_gamepad_setting("vk_side_tips", "off").is_ok());
        assert!(validate_gamepad_setting("vk_sheet_opens", "3").is_ok());
        assert!(validate_gamepad_setting("vk_sheet_opens", "-1").is_err());
        let mut settings = GamepadSettings::default();
        assert_eq!(settings.vk_sheet_opens, 0);
        apply_gamepad_settings_text(&mut settings, "vk_sheet_opens=2\n");
        assert_eq!(settings.vk_sheet_opens, 2);
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn run_mode_defaults_to_always_and_validates() {
        assert_eq!(parse_run_mode(None), RunMode::Always);
        assert_eq!(parse_run_mode(Some(" SignIn ")), RunMode::SignIn);
        assert_eq!(parse_run_mode(Some("bogus")), RunMode::Always);
        assert!(validate_gamepad_setting("run_mode", "signin").is_ok());
        assert!(validate_gamepad_setting("run_mode", "always").is_ok());
        assert!(validate_gamepad_setting("run_mode", "never").is_err());
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn vk_bar_scale_validates_range() {
        // The tray Compact toggle writes this; out-of-range must be rejected so a
        // bad value can't shrink the bar to nothing or balloon it past the screen.
        assert!(validate_gamepad_setting("vk_bar_scale", "0.8").is_ok());
        assert!(validate_gamepad_setting("vk_bar_scale", "1.0").is_ok());
        assert!(validate_gamepad_setting("vk_bar_scale", "2.0").is_err());
        assert!(validate_gamepad_setting("vk_bar_scale", "abc").is_err());
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn gamepad_settings_text_accepts_sleep_on_game_aliases() {
        let mut settings = GamepadSettings::default();
        apply_gamepad_settings_text(&mut settings, "sleep_on_game=false\n");
        assert!(!settings.sleep_on_game);

        apply_gamepad_settings_text(&mut settings, "game_sleep=on\n");
        assert!(settings.sleep_on_game);

        apply_gamepad_settings_text(&mut settings, "auto_stop_on_game=true\n");
        assert!(settings.auto_stop_on_game);

        apply_gamepad_settings_text(&mut settings, "prompt_userland_debug=on\n");
        assert!(settings.prompt_userland_debug);
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn cursor_enabled_takes_precedence_over_its_legacy_alias() {
        let mut settings = GamepadSettings::default();
        apply_gamepad_settings_text(&mut settings, "gamepad_cursor=false\ncursor_enabled=true\n");
        assert!(settings.cursor_enabled);

        apply_gamepad_settings_text(&mut settings, "cursor_enabled=false\ngamepad_cursor=true\n");
        assert!(!settings.cursor_enabled);
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn voice_enabled_defaults_on_and_can_be_turned_off() {
        let mut settings = GamepadSettings::default();
        assert!(settings.voice_enabled);

        apply_gamepad_settings_text(&mut settings, "voice_enabled = false\n");
        assert!(!settings.voice_enabled);
        apply_gamepad_settings_text(&mut settings, "voice_enabled=off\n");
        assert!(!settings.voice_enabled);
        apply_gamepad_settings_text(&mut settings, "voice_enabled=on\n");
        assert!(settings.voice_enabled);

        assert!(validate_gamepad_setting("voice_enabled", "false").is_ok());
        assert!(validate_gamepad_setting("voice_enabled", "nope").is_err());
    }
}
