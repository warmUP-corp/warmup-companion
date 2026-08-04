//! User-configured desktop shortcuts for logical controller buttons.

use std::collections::{HashMap, HashSet};
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use std::process::Command;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
use windows::core::{w, PCWSTR};
#[cfg(windows)]
use windows::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::gamepad_backend::Button;

pub const MAPPABLE_BUTTONS: &[Button] = &[
    Button::Lt,
    Button::Lb,
    Button::Select,
    Button::L3,
    Button::Up,
    Button::Left,
    Button::Right,
    Button::Down,
    Button::Touchpad,
    Button::Rt,
    Button::Rb,
    Button::Start,
    Button::R3,
    Button::Y,
    Button::X,
    Button::B,
    Button::A,
    Button::Guide,
];

/// A directed two-button desktop-action key. The first button is held as a
/// modifier and the second button is the newly pressed trigger.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ControllerChord {
    pub hold: Button,
    pub press: Button,
}

impl ControllerChord {
    pub fn new(hold: Button, press: Button) -> Option<Self> {
        if hold == press || !MAPPABLE_BUTTONS.contains(&hold) || !MAPPABLE_BUTTONS.contains(&press)
        {
            return None;
        }
        Some(Self { hold, press })
    }

    pub fn setting_key(self) -> String {
        format!(
            "shortcut_{}+{}",
            self.hold.as_str().to_ascii_lowercase(),
            self.press.as_str().to_ascii_lowercase()
        )
    }
}

/// A Windows virtual key and its modifier state. Values are stored as
/// `Ctrl+Alt+VK_80` in `settings.ini`, which is stable across keyboard layouts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shortcut {
    pub key: u16,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Shortcut {
    pub fn new(key: u16, ctrl: bool, alt: bool, shift: bool, win: bool) -> Option<Self> {
        (key != 0 && key <= 0xff && !is_modifier_key(key)).then_some(Self {
            key,
            ctrl,
            alt,
            shift,
            win,
        })
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let mut key = None;
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        for part in raw
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "win" | "windows" => win = true,
                _ => {
                    if key.replace(parse_key(part)?).is_some() {
                        return None;
                    }
                }
            }
        }
        Self::new(key?, ctrl, alt, shift, win)
    }

    pub fn storage(self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(format!("VK_{}", self.key));
        parts.join("+")
    }

    pub fn display(self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(key_label(self.key));
        parts.join("+")
    }
}

/// The four desktop-only behaviors a controller input can own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopActionKind {
    Shortcut,
    Launch,
    Workspace,
    Command,
}

impl DesktopActionKind {
    pub const ALL: [Self; 4] = [Self::Shortcut, Self::Launch, Self::Workspace, Self::Command];

    pub fn label(self) -> &'static str {
        match self {
            Self::Shortcut => "Keys",
            Self::Launch => "App / site",
            Self::Workspace => "Workspace",
            Self::Command => "Command",
        }
    }
}

/// A persisted desktop action. Existing key-chord mappings keep their original
/// `Ctrl+Alt+VK_80` format; launch and command mappings use explicit prefixes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControllerAction {
    Shortcut(Shortcut),
    Launch(String),
    Workspace(String),
    Command(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchableApp {
    pub name: String,
    pub target: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceWindowCandidate {
    pub id: isize,
    pub title: String,
    pub executable: String,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl ControllerAction {
    pub fn new(kind: DesktopActionKind, value: &str) -> Option<Self> {
        let value = value.trim();
        match kind {
            DesktopActionKind::Shortcut => Shortcut::parse(value).map(Self::Shortcut),
            DesktopActionKind::Launch if valid_action_text(value) => {
                Some(Self::Launch(value.to_string()))
            }
            DesktopActionKind::Workspace if valid_workspace_name(value) => {
                Some(Self::Workspace(value.to_string()))
            }
            DesktopActionKind::Command if valid_action_text(value) => {
                Some(Self::Command(value.to_string()))
            }
            _ => None,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if let Some((prefix, value)) = raw.split_once(':') {
            let kind = match prefix.trim().to_ascii_lowercase().as_str() {
                "launch" | "open" => DesktopActionKind::Launch,
                "workspace" | "scene" => DesktopActionKind::Workspace,
                "command" | "cmd" => DesktopActionKind::Command,
                _ => return Shortcut::parse(raw).map(Self::Shortcut),
            };
            return Self::new(kind, value);
        }
        Shortcut::parse(raw).map(Self::Shortcut)
    }

    pub fn kind(&self) -> DesktopActionKind {
        match self {
            Self::Shortcut(_) => DesktopActionKind::Shortcut,
            Self::Launch(_) => DesktopActionKind::Launch,
            Self::Workspace(_) => DesktopActionKind::Workspace,
            Self::Command(_) => DesktopActionKind::Command,
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Shortcut(_) => None,
            Self::Launch(value) | Self::Workspace(value) | Self::Command(value) => Some(value),
        }
    }

    pub fn storage(&self) -> String {
        match self {
            Self::Shortcut(shortcut) => shortcut.storage(),
            Self::Launch(value) => format!("launch:{value}"),
            Self::Workspace(value) => format!("workspace:{value}"),
            Self::Command(value) => format!("command:{value}"),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Shortcut(shortcut) => shortcut.display(),
            Self::Launch(value) => format!("Open · {value}"),
            Self::Workspace(value) => format!("Workspace · {value}"),
            Self::Command(value) => format!("Run · {value}"),
        }
    }
}

fn valid_action_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.contains(['\0', '\r', '\n'])
}

fn valid_workspace_name(value: &str) -> bool {
    !value.is_empty()
        && value == value.trim()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_'))
}

const MAX_WORKSPACE_WINDOWS: usize = 24;
const MAX_WORKSPACE_JSON_BYTES: usize = 64 * 1024;
const MAX_WINDOW_DIMENSION: i32 = 32_768;
const MAX_WINDOW_COORDINATE: i64 = 1_000_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceWindow {
    executable: String,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    maximized: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Workspace {
    windows: Vec<WorkspaceWindow>,
}

impl WorkspaceWindow {
    fn validate(&self) -> bool {
        valid_executable_path(&self.executable)
            && (1..=MAX_WINDOW_DIMENSION).contains(&self.width)
            && (1..=MAX_WINDOW_DIMENSION).contains(&self.height)
            && (-MAX_WINDOW_COORDINATE..=MAX_WINDOW_COORDINATE).contains(&(self.left as i64))
            && (-MAX_WINDOW_COORDINATE..=MAX_WINDOW_COORDINATE).contains(&(self.top as i64))
            && (self.left as i64 + self.width as i64).abs() <= MAX_WINDOW_COORDINATE
            && (self.top as i64 + self.height as i64).abs() <= MAX_WINDOW_COORDINATE
    }
}

impl Workspace {
    fn validate(&self) -> bool {
        !self.windows.is_empty()
            && self.windows.len() <= MAX_WORKSPACE_WINDOWS
            && self.windows.iter().all(WorkspaceWindow::validate)
    }
}

fn valid_executable_path(value: &str) -> bool {
    if value.is_empty() || value.chars().count() > 32_767 || value.chars().any(char::is_control) {
        return false;
    }
    let bytes = value.as_bytes();
    let windows_absolute = bytes.first().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.get(1) == Some(&b':')
        && bytes
            .get(2)
            .is_some_and(|&separator| matches!(separator, b'\\' | b'/'))
        || value.starts_with("\\\\");
    let absolute = windows_absolute || std::path::Path::new(value).is_absolute();
    let executable = value
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    absolute
        && !executable.is_empty()
        && (executable.ends_with(".exe") || executable.ends_with(".com"))
        && !value
            .chars()
            .any(|character| matches!(character, '<' | '>' | '"' | '|' | '?' | '*'))
}

#[derive(Default)]
struct ShortcutState {
    loaded: bool,
    mappings: HashMap<Button, ControllerAction>,
    chords: HashMap<ControllerChord, ControllerAction>,
}

static STATE: OnceLock<Mutex<ShortcutState>> = OnceLock::new();

fn state() -> &'static Mutex<ShortcutState> {
    STATE.get_or_init(|| Mutex::new(ShortcutState::default()))
}

pub fn setting_key(button: Button) -> String {
    format!("shortcut_{}", button.as_str().to_ascii_lowercase())
}

pub fn chord_setting_key(hold: Button, press: Button) -> Option<String> {
    ControllerChord::new(hold, press).map(ControllerChord::setting_key)
}

pub fn button_from_setting_name(name: &str) -> Option<Button> {
    match name.trim().to_ascii_uppercase().as_str() {
        "A" => Some(Button::A),
        "B" => Some(Button::B),
        "X" => Some(Button::X),
        "Y" => Some(Button::Y),
        "LB" => Some(Button::Lb),
        "RB" => Some(Button::Rb),
        "LT" => Some(Button::Lt),
        "RT" => Some(Button::Rt),
        "SELECT" => Some(Button::Select),
        "START" => Some(Button::Start),
        "L3" => Some(Button::L3),
        "R3" => Some(Button::R3),
        "UP" => Some(Button::Up),
        "DOWN" => Some(Button::Down),
        "LEFT" => Some(Button::Left),
        "RIGHT" => Some(Button::Right),
        "GUIDE" => Some(Button::Guide),
        "TOUCHPAD" => Some(Button::Touchpad),
        _ => None,
    }
}

enum ShortcutSettingKey {
    Button(Button),
    Chord(ControllerChord),
}

fn parse_setting_key(key: &str) -> Option<ShortcutSettingKey> {
    let suffix = key.strip_prefix("shortcut_")?;
    let mut buttons = suffix.split('+');
    let first = button_from_setting_name(buttons.next()?)?;
    let Some(second) = buttons.next() else {
        return Some(ShortcutSettingKey::Button(first));
    };
    if buttons.next().is_some() {
        return None;
    }
    Some(ShortcutSettingKey::Chord(ControllerChord::new(
        first,
        button_from_setting_name(second)?,
    )?))
}

pub fn is_valid_setting(key: &str, value: &str) -> bool {
    parse_setting_key(key).is_some()
        && (value.trim().is_empty() || ControllerAction::parse(value).is_some())
}

pub fn is_valid_workspace_setting(key: &str, value: &str) -> bool {
    let Some(name) = key.strip_prefix("workspace_") else {
        return false;
    };
    if name.is_empty()
        || name.len() > 48
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || value.len() > MAX_WORKSPACE_JSON_BYTES
    {
        return false;
    }
    serde_json::from_str::<Workspace>(value).is_ok_and(|workspace| workspace.validate())
}

pub fn reload() {
    if let Ok(mut state) = state().lock() {
        state.loaded = false;
        load(&mut state);
    }
}

pub fn mapping(button: Button) -> Option<ControllerAction> {
    let Ok(mut state) = state().lock() else {
        return None;
    };
    load(&mut state);
    state.mappings.get(&button).cloned()
}

pub fn is_mapped(button: Button) -> bool {
    mapping(button).is_some()
}

/// Returns whether a button is used as the held side of any configured chord.
/// Press-side buttons remain ordinary inputs until their hold modifier is down.
pub fn is_chord_hold_mapped(button: Button) -> bool {
    let Ok(mut state) = state().lock() else {
        return false;
    };
    load(&mut state);
    state.chords.keys().any(|chord| chord.hold == button)
}

pub fn chord_mapping(hold: Button, press: Button) -> Option<ControllerAction> {
    let chord = ControllerChord::new(hold, press)?;
    let Ok(mut state) = state().lock() else {
        return None;
    };
    load(&mut state);
    state.chords.get(&chord).cloned()
}

/// Resolve a directed two-button action against the currently held buttons.
/// The first configured pair in controller order wins when several holds match.
pub fn matching_chord(held: &[Button], pressed: Button) -> Option<ControllerChord> {
    let Ok(mut state) = state().lock() else {
        return None;
    };
    load(&mut state);
    MAPPABLE_BUTTONS
        .iter()
        .copied()
        .filter(|button| *button != pressed && held.contains(button))
        .find_map(|button| {
            let chord = ControllerChord::new(button, pressed)?;
            state.chords.contains_key(&chord).then_some(chord)
        })
}

pub fn set_mapping(button: Button, action: Option<ControllerAction>) -> Result<(), String> {
    let value = action
        .as_ref()
        .map(ControllerAction::storage)
        .unwrap_or_default();
    crate::config::set_gamepad_setting(&setting_key(button), &value)?;
    if let Ok(mut state) = state().lock() {
        load(&mut state);
        match action {
            Some(action) => {
                state.mappings.insert(button, action);
            }
            None => {
                state.mappings.remove(&button);
            }
        }
    }
    Ok(())
}

pub fn set_chord_mapping(
    hold: Button,
    press: Button,
    action: Option<ControllerAction>,
) -> Result<(), String> {
    let chord = ControllerChord::new(hold, press)
        .ok_or_else(|| "a chord needs two distinct mappable buttons".to_string())?;
    let value = action
        .as_ref()
        .map(ControllerAction::storage)
        .unwrap_or_default();
    crate::config::set_gamepad_setting(&chord.setting_key(), &value)?;
    if let Ok(mut state) = state().lock() {
        load(&mut state);
        match action {
            Some(action) => {
                state.chords.insert(chord, action);
            }
            None => {
                state.chords.remove(&chord);
            }
        }
    }
    Ok(())
}

/// Sends a configured shortcut and reports whether the button was mapped. The
/// caller consumes a mapped controller edge even if Windows rejects SendInput,
/// so an explicitly remapped button never falls through to its old action.
#[cfg(windows)]
pub fn trigger(button: Button) -> bool {
    let Some(action) = mapping(button) else {
        return false;
    };
    trigger_action(button.as_str(), action)
}

#[cfg(windows)]
pub fn trigger_chord(chord: ControllerChord) -> bool {
    let Some(action) = chord_mapping(chord.hold, chord.press) else {
        return false;
    };
    trigger_action(
        &format!("{}+{}", chord.hold.as_str(), chord.press.as_str()),
        action,
    )
}

#[cfg(windows)]
fn trigger_action(label: &str, action: ControllerAction) -> bool {
    let result = match action {
        ControllerAction::Shortcut(shortcut) => crate::vk_nav::inject_shortcut(
            shortcut.key,
            shortcut.ctrl,
            shortcut.alt,
            shortcut.shift,
            shortcut.win,
        )
        .then_some(())
        .ok_or_else(|| "SendInput rejected the shortcut".to_string()),
        ControllerAction::Launch(target) => launch_target(&target),
        ControllerAction::Workspace(name) => restore_workspace(&name),
        ControllerAction::Command(command) => run_command(&command),
    };
    if let Err(error) = result {
        crate::install::log_line(&format!("controller action {label} failed: {error}"));
    }
    true
}

pub fn capture_workspace(name: &str) -> Result<(), String> {
    if !valid_workspace_name(name.trim()) {
        return Err("Use a workspace name with letters, numbers, spaces, - or _".to_string());
    }
    capture_workspace_inner(name.trim())
}

pub fn launchable_apps() -> Vec<LaunchableApp> {
    #[cfg(windows)]
    {
        let mut apps = Vec::new();
        for root in start_menu_program_roots() {
            collect_launchable_apps(&root, &mut apps);
        }
        return sort_and_dedup_launchable_apps(apps);
    }

    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub fn workspace_window_candidates() -> Vec<WorkspaceWindowCandidate> {
    #[cfg(windows)]
    {
        return desktop_windows(false)
            .into_iter()
            .map(|window| WorkspaceWindowCandidate {
                id: window.hwnd.0 as isize,
                title: window.title,
                executable: window.entry.executable,
                left: window.entry.left,
                top: window.entry.top,
                width: window.entry.width,
                height: window.entry.height,
                maximized: window.entry.maximized,
            })
            .collect();
    }

    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub fn capture_workspace_windows(name: &str, window_ids: &[isize]) -> Result<(), String> {
    let name = name.trim();
    if !valid_workspace_name(name) {
        return Err("Use a workspace name with letters, numbers, spaces, - or _".to_string());
    }

    #[cfg(windows)]
    {
        let live = desktop_windows(false);
        let available_ids = live
            .iter()
            .map(|window| window.hwnd.0 as isize)
            .collect::<Vec<_>>();
        let selected = selected_window_indices(&available_ids, window_ids)?;
        let mut windows = Vec::with_capacity(selected.len());
        for index in selected {
            let entry = live[index].entry.clone();
            if !entry.validate() {
                return Err("A selected window has invalid geometry".to_string());
            }
            windows.push(entry);
        }
        let workspace = Workspace { windows };
        let value = serde_json::to_string(&workspace)
            .map_err(|error| format!("encode workspace: {error}"))?;
        if value.len() > MAX_WORKSPACE_JSON_BYTES {
            return Err("The selected workspace is too large".to_string());
        }
        return crate::config::set_gamepad_setting(&workspace_key(name), &value);
    }

    #[cfg(not(windows))]
    {
        let _ = window_ids;
        Err("Workspace capture is only available on Windows".to_string())
    }
}

fn workspace_key(name: &str) -> String {
    let slug = name
        .trim()
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' => (byte + 32) as char,
            b'a'..=b'z' | b'0'..=b'9' | b'_' => byte as char,
            b' ' | b'-' => '_',
            _ => '_',
        })
        .collect::<String>();
    format!("workspace_{slug}")
}

fn launchable_app_from_path(path: &Path) -> Option<LaunchableApp> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if !matches!(extension.as_str(), "lnk" | "url" | "appref-ms" | "exe") {
        return None;
    }
    let name = path.file_stem()?.to_string_lossy().trim().to_string();
    (!name.is_empty()).then(|| LaunchableApp {
        name,
        target: path.to_string_lossy().into_owned(),
    })
}

fn normalize_launchable_target(target: &str) -> String {
    target.replace('/', "\\").to_ascii_lowercase()
}

fn sort_and_dedup_launchable_apps(mut apps: Vec<LaunchableApp>) -> Vec<LaunchableApp> {
    apps.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| {
                left.target
                    .to_ascii_lowercase()
                    .cmp(&right.target.to_ascii_lowercase())
            })
    });
    let mut targets = HashSet::with_capacity(apps.len());
    apps.retain(|app| targets.insert(normalize_launchable_target(&app.target)));
    apps
}

fn selected_window_indices(
    available_ids: &[isize],
    window_ids: &[isize],
) -> Result<Vec<usize>, String> {
    if window_ids.is_empty() {
        return Err("Select at least one window".to_string());
    }
    if window_ids.len() > MAX_WORKSPACE_WINDOWS {
        return Err(format!(
            "Select no more than {MAX_WORKSPACE_WINDOWS} windows"
        ));
    }
    let mut seen = HashSet::with_capacity(window_ids.len());
    let mut indices = Vec::with_capacity(window_ids.len());
    for &window_id in window_ids {
        if !seen.insert(window_id) {
            return Err("A window was selected more than once".to_string());
        }
        let index = available_ids
            .iter()
            .position(|&available_id| available_id == window_id)
            .ok_or_else(|| "A selected window is no longer available".to_string())?;
        indices.push(index);
    }
    Ok(indices)
}

#[cfg(windows)]
fn start_menu_program_roots() -> Vec<PathBuf> {
    ["APPDATA", "PROGRAMDATA"]
        .into_iter()
        .filter_map(|variable| {
            std::env::var_os(variable).map(|base| {
                PathBuf::from(base)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs")
            })
        })
        .collect()
}

#[cfg(windows)]
fn collect_launchable_apps(directory: &Path, apps: &mut Vec<LaunchableApp>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_launchable_apps(&path, apps);
        } else if file_type.is_file() {
            if let Some(app) = launchable_app_from_path(&path) {
                apps.push(app);
            }
        }
    }
}

fn load_workspace(name: &str) -> Result<Workspace, String> {
    let key = workspace_key(name);
    let path =
        crate::config::settings_path().ok_or_else(|| "settings path is unavailable".to_string())?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let value = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(candidate, value)| (candidate.trim() == key).then(|| value.trim()))
        .ok_or_else(|| format!("workspace {name:?} was not found"))?;
    if !is_valid_workspace_setting(&key, value) {
        return Err(format!("workspace {name:?} is invalid"));
    }
    serde_json::from_str(value).map_err(|error| format!("read workspace {name:?}: {error}"))
}

#[cfg(not(windows))]
pub fn trigger(_button: Button) -> bool {
    false
}

#[cfg(not(windows))]
pub fn trigger_chord(_chord: ControllerChord) -> bool {
    false
}

fn load(state: &mut ShortcutState) {
    if state.loaded {
        return;
    }
    state.loaded = true;
    state.mappings.clear();
    state.chords.clear();
    let Some(text) =
        crate::config::settings_path().and_then(|path| std::fs::read_to_string(path).ok())
    else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(setting_key) = parse_setting_key(key.trim()) else {
            continue;
        };
        let Some(action) = ControllerAction::parse(value.trim()) else {
            continue;
        };
        match setting_key {
            ShortcutSettingKey::Button(button) => {
                state.mappings.insert(button, action);
            }
            ShortcutSettingKey::Chord(chord) => {
                state.chords.insert(chord, action);
            }
        }
    }
}

#[cfg(windows)]
fn launch_target(target: &str) -> Result<(), String> {
    launch_target_with_show(target, SW_SHOWNORMAL)
}

#[cfg(windows)]
fn launch_target_without_activation(target: &str) -> Result<(), String> {
    launch_target_with_show(
        target,
        windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
    )
}

#[cfg(windows)]
fn launch_target_with_show(
    target: &str,
    show_command: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD,
) -> Result<(), String> {
    let target = wide(target);
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            show_command,
        )
    };
    (result.0 as usize > 32)
        .then_some(())
        .ok_or_else(|| format!("ShellExecute failed ({})", result.0 as usize))
}

#[cfg(windows)]
fn run_command(command: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("cmd.exe")
        .args(["/d", "/s", "/c", command])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("run command: {error}"))
}

#[cfg(windows)]
#[derive(Clone)]
struct LiveWindow {
    hwnd: windows::Win32::Foundation::HWND,
    title: String,
    entry: WorkspaceWindow,
}

#[cfg(windows)]
fn desktop_windows(include_minimized: bool) -> Vec<LiveWindow> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        IsZoomed, GWL_EXSTYLE, GW_OWNER, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
    };

    struct Context {
        current_pid: u32,
        include_minimized: bool,
        windows: Vec<LiveWindow>,
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let context = &mut *(lparam.0 as *mut Context);
        if !IsWindowVisible(hwnd).as_bool()
            || (!context.include_minimized && IsIconic(hwnd).as_bool())
            || GetWindowTextLengthW(hwnd) <= 0
            || GetWindow(hwnd, GW_OWNER).is_ok()
            || (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0) != 0
        {
            return BOOL(1);
        }
        let mut title_buffer = [0u16; 512];
        let title_length = GetWindowTextW(hwnd, &mut title_buffer);
        if title_length <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&title_buffer[..title_length as usize])
            .trim()
            .to_string();
        if title.is_empty() {
            return BOOL(1);
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == context.current_pid {
            return BOOL(1);
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return BOOL(1);
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if !context.include_minimized && (width < 160 || height < 100) {
            return BOOL(1);
        }
        let Some(executable) = crate::win::native_keyboard::window_process_image(hwnd) else {
            return BOOL(1);
        };
        let executable = executable.trim().to_string();
        if !valid_executable_path(&executable) || is_system_surface(&executable) {
            return BOOL(1);
        }
        let maximized = IsZoomed(hwnd).as_bool();
        let rect = if maximized {
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            if GetWindowPlacement(hwnd, &mut placement).is_ok()
                && placement.rcNormalPosition.right > placement.rcNormalPosition.left
                && placement.rcNormalPosition.bottom > placement.rcNormalPosition.top
            {
                placement.rcNormalPosition
            } else {
                rect
            }
        } else {
            rect
        };
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let entry = WorkspaceWindow {
            executable: executable.clone(),
            left: rect.left,
            top: rect.top,
            width,
            height,
            maximized,
        };
        if !context.include_minimized && !entry.validate() {
            return BOOL(1);
        }
        context.windows.push(LiveWindow { hwnd, title, entry });
        BOOL(1)
    }

    let mut context = Context {
        current_pid: std::process::id(),
        include_minimized,
        windows: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut context as *mut _ as isize));
    }
    context.windows
}

#[cfg(windows)]
fn is_system_surface(executable: &str) -> bool {
    let image = executable
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        image.as_str(),
        "applicationframehost.exe"
            | "ctfmon.exe"
            | "dwm.exe"
            | "lockapp.exe"
            | "runtimebroker.exe"
            | "searchapp.exe"
            | "searchhost.exe"
            | "shellexperiencehost.exe"
            | "startmenuexperiencehost.exe"
            | "tabtip.exe"
            | "textinputhost.exe"
            | "warmup-companion.exe"
            | "warmup.exe"
            | "widgets.exe"
    )
}

#[cfg(windows)]
fn capture_workspace_inner(name: &str) -> Result<(), String> {
    let workspace = Workspace {
        windows: desktop_windows(false)
            .into_iter()
            .map(|window| window.entry)
            .take(MAX_WORKSPACE_WINDOWS)
            .collect(),
    };
    if !workspace.validate() {
        return Err("No visible app windows were found to capture".to_string());
    }
    let value =
        serde_json::to_string(&workspace).map_err(|error| format!("encode workspace: {error}"))?;
    crate::config::set_gamepad_setting(&workspace_key(name), &value)
}

#[cfg(windows)]
fn missing_workspace_executables(workspace: &Workspace) -> Vec<String> {
    let live = desktop_windows(true);
    let mut desired = HashMap::<String, (String, usize)>::new();
    for target in &workspace.windows {
        let key = target.executable.to_ascii_lowercase();
        let entry = desired
            .entry(key)
            .or_insert_with(|| (target.executable.clone(), 0));
        entry.1 += 1;
    }
    let mut live_counts = HashMap::<String, usize>::new();
    for window in live {
        *live_counts
            .entry(window.entry.executable.to_ascii_lowercase())
            .or_default() += 1;
    }
    desired
        .into_iter()
        .filter_map(|(key, (executable, wanted))| {
            let present = live_counts.get(&key).copied().unwrap_or_default();
            (present < wanted).then(|| format!("{executable} ({} missing)", wanted - present))
        })
        .collect()
}

#[cfg(not(windows))]
fn capture_workspace_inner(_name: &str) -> Result<(), String> {
    Err("Workspace capture is only available on Windows".to_string())
}

#[cfg(windows)]
fn restore_workspace(name: &str) -> Result<(), String> {
    use std::collections::HashSet;
    use std::time::Duration;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindowAsync, SWP_NOACTIVATE, SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE,
    };

    let workspace = load_workspace(name)?;
    let workspace_name = name.to_string();
    std::thread::Builder::new()
        .name(format!("warmup-workspace-{}", workspace_key(name)))
        .spawn(move || {
            let mut launched = HashMap::<String, usize>::new();
            const MAX_ATTEMPTS: usize = 20;
            const RETRY_DELAY: Duration = Duration::from_millis(250);
            for _ in 0..MAX_ATTEMPTS {
                let live = desktop_windows(true);
                let mut used = HashSet::new();
                let mut missing = HashMap::<String, (String, usize)>::new();
                for target in &workspace.windows {
                    let found = live.iter().find(|window| {
                        !used.contains(&(window.hwnd.0 as usize))
                            && window
                                .entry
                                .executable
                                .eq_ignore_ascii_case(&target.executable)
                    });
                    if let Some(window) = found {
                        used.insert(window.hwnd.0 as usize);
                        unsafe {
                            let _ = ShowWindowAsync(window.hwnd, SW_RESTORE);
                            let _ = SetWindowPos(
                                window.hwnd,
                                HWND::default(),
                                target.left,
                                target.top,
                                target.width,
                                target.height,
                                SWP_NOACTIVATE | SWP_NOZORDER,
                            );
                            if target.maximized {
                                let _ = ShowWindowAsync(window.hwnd, SW_MAXIMIZE);
                            }
                        }
                    } else {
                        let key = target.executable.to_ascii_lowercase();
                        let entry = missing
                            .entry(key)
                            .or_insert_with(|| (target.executable.clone(), 0));
                        entry.1 += 1;
                    }
                }
                for (key, (executable, count)) in &missing {
                    let requested = launched.get(key).copied().unwrap_or_default();
                    if requested >= *count {
                        continue;
                    }
                    launched.insert(key.clone(), requested + 1);
                    if let Err(error) = launch_target_without_activation(executable) {
                        crate::install::log_line(&format!(
                            "workspace {workspace_name}: launch {executable} failed: {error}"
                        ));
                    }
                }
                if missing.is_empty() {
                    return;
                }
                std::thread::sleep(RETRY_DELAY);
            }
            let missing = missing_workspace_executables(&workspace);
            if missing.is_empty() {
                return;
            }
            crate::install::log_line(&format!(
                "workspace {workspace_name} restore timed out after {}ms waiting for: {}",
                MAX_ATTEMPTS as u64 * RETRY_DELAY.as_millis() as u64,
                missing.join(", ")
            ));
        })
        .map(|_| ())
        .map_err(|error| format!("start workspace restore: {error}"))
}

#[cfg(not(windows))]
fn restore_workspace(_name: &str) -> Result<(), String> {
    Err("Workspace restore is only available on Windows".to_string())
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn parse_key(raw: &str) -> Option<u16> {
    let key = raw.trim().to_ascii_uppercase();
    if let Some(value) = key.strip_prefix("VK_") {
        return value.parse::<u16>().ok();
    }
    match key.as_str() {
        "ENTER" => Some(0x0d),
        "ESC" | "ESCAPE" => Some(0x1b),
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "BACKSPACE" => Some(0x08),
        "DELETE" => Some(0x2e),
        _ if key.len() == 1 => Some(key.as_bytes()[0] as u16),
        _ => None,
    }
}

fn is_modifier_key(key: u16) -> bool {
    matches!(key, 0x10 | 0x11 | 0x12 | 0x5b | 0x5c)
}

fn key_label(key: u16) -> String {
    match key {
        0x08 => "Backspace".to_string(),
        0x09 => "Tab".to_string(),
        0x0d => "Enter".to_string(),
        0x1b => "Esc".to_string(),
        0x20 => "Space".to_string(),
        0x2e => "Delete".to_string(),
        0x70..=0x87 => format!("F{}", key - 0x6f),
        0x30..=0x39 | 0x41..=0x5a => (key as u8 as char).to_string(),
        _ => format!("VK {key:#04X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_storage_round_trips() {
        let shortcut = Shortcut::new(0x50, true, true, false, false).unwrap();
        assert_eq!(shortcut.storage(), "Ctrl+Alt+VK_80");
        assert_eq!(Shortcut::parse(&shortcut.storage()), Some(shortcut));
        assert_eq!(shortcut.display(), "Ctrl+Alt+P");
    }

    #[test]
    fn setting_validation_rejects_unknown_buttons_and_modifier_only_keys() {
        assert!(is_valid_setting("shortcut_a", "Ctrl+VK_80"));
        assert!(is_valid_setting(
            "shortcut_a",
            r"launch:C:\Games\example.exe"
        ));
        assert!(is_valid_setting("shortcut_a", "command:echo warmup"));
        assert!(is_valid_setting("shortcut_a", "workspace:Coding"));
        assert!(is_valid_setting("shortcut_a", ""));
        assert!(!is_valid_setting("shortcut_unknown", "Ctrl+VK_80"));
        assert!(!is_valid_setting("shortcut_a", "Ctrl+VK_17"));
    }

    #[test]
    fn two_button_setting_keys_preserve_hold_then_press_order() {
        let chord = ControllerChord::new(Button::Lb, Button::A).unwrap();
        assert_eq!(chord.setting_key(), "shortcut_lb+a");
        assert_eq!(
            chord_setting_key(Button::Lb, Button::A),
            Some("shortcut_lb+a".into())
        );
        assert_ne!(ControllerChord::new(Button::A, Button::Lb), Some(chord));
        assert!(is_valid_setting(
            "shortcut_lb+a",
            "launch:https://example.com"
        ));
        assert!(is_valid_setting("shortcut_a+lb", "workspace:Coding"));
        assert!(!is_valid_setting("shortcut_a+a", "Ctrl+VK_80"));
        assert!(!is_valid_setting("shortcut_a+lb+x", "Ctrl+VK_80"));
    }

    #[test]
    fn generic_chords_accept_other_buttons_and_actions() {
        let chord = ControllerChord::new(Button::X, Button::Y).unwrap();
        assert_eq!(chord.setting_key(), "shortcut_x+y");
        assert!(is_valid_setting("shortcut_x+y", "command:echo generic"));
        assert_eq!(
            ControllerAction::parse("command:echo generic"),
            Some(ControllerAction::Command("echo generic".into()))
        );
        assert!(ControllerChord::new(Button::X, Button::X).is_none());
        assert!(!is_valid_setting(
            "shortcut_x+unknown",
            "command:echo generic"
        ));
    }

    #[test]
    fn every_distinct_mappable_pair_preserves_direction() {
        for hold in MAPPABLE_BUTTONS.iter().copied() {
            for press in MAPPABLE_BUTTONS.iter().copied() {
                if hold == press {
                    continue;
                }
                let chord = ControllerChord::new(hold, press).unwrap();
                assert_eq!(chord.hold, hold);
                assert_eq!(chord.press, press);
                for value in [
                    "Ctrl+VK_80",
                    "launch:https://example.com",
                    "workspace:Coding",
                    "command:echo pair",
                ] {
                    assert!(is_valid_setting(&chord.setting_key(), value));
                }
            }
        }
    }

    #[test]
    fn launch_and_command_actions_round_trip_without_control_characters() {
        let launch =
            ControllerAction::new(DesktopActionKind::Launch, r"C:\Games\example.exe").unwrap();
        assert_eq!(launch.storage(), r"launch:C:\Games\example.exe");
        assert_eq!(ControllerAction::parse(&launch.storage()), Some(launch));

        let command = ControllerAction::new(DesktopActionKind::Command, "echo warmup").unwrap();
        assert_eq!(command.storage(), "command:echo warmup");
        assert_eq!(ControllerAction::parse(&command.storage()), Some(command));
        assert!(ControllerAction::new(DesktopActionKind::Command, "echo\nnope").is_none());
    }

    #[test]
    fn workspace_actions_and_captured_layouts_validate() {
        let action = ControllerAction::new(DesktopActionKind::Workspace, "Coding").unwrap();
        assert_eq!(action.storage(), "workspace:Coding");
        assert_eq!(ControllerAction::parse(&action.storage()), Some(action));
        assert_eq!(workspace_key("Coding Setup"), "workspace_coding_setup");

        let workspace = Workspace {
            windows: vec![WorkspaceWindow {
                executable: r"C:\Program Files\Editor\editor.exe".to_string(),
                left: 0,
                top: 0,
                width: 1280,
                height: 1080,
                maximized: false,
            }],
        };
        let json = serde_json::to_string(&workspace).unwrap();
        assert!(is_valid_workspace_setting("workspace_coding", &json));
        assert!(!is_valid_workspace_setting(
            "workspace_coding",
            r#"{"windows":[]}"#
        ));
    }

    #[test]
    fn workspace_settings_reject_untrusted_shapes_and_geometry() {
        let valid = r#"{"windows":[{"executable":"C:\\Apps\\Editor.exe","left":0,"top":0,"width":1280,"height":720,"maximized":false}]}"#;
        assert!(is_valid_workspace_setting("workspace_coding", valid));
        assert!(!is_valid_workspace_setting("workspace_Coding", valid));
        assert!(!is_valid_workspace_setting(
            "workspace_coding",
            r#"{"windows":[{"executable":"editor.exe","left":0,"top":0,"width":1280,"height":720,"maximized":false}]}"#
        ));
        assert!(!is_valid_workspace_setting(
            "workspace_coding",
            r#"{"windows":[{"executable":"C:\\Apps\\Editor.exe","left":0,"top":0,"width":0,"height":720,"maximized":false}]}"#
        ));
        assert!(!is_valid_workspace_setting(
            "workspace_coding",
            r#"{"windows":[{"executable":"C:\\Apps\\Editor.exe","left":0,"top":0,"width":1280,"height":720,"maximized":false,"extra":true}]}"#
        ));
    }

    #[test]
    fn inventory_helpers_filter_sort_and_deduplicate() {
        let apps = [
            "C:/Apps/Zeta.EXE",
            "C:/Apps/readme.txt",
            "C:/Apps/Editor.LnK",
            "C:/Apps/Docs.url",
            "C:/Apps/Remote.AppRef-Ms",
            "C:/Apps/editor.exe",
            "c:/apps/EDITOR.EXE",
        ]
        .into_iter()
        .filter_map(|path| launchable_app_from_path(Path::new(path)))
        .collect();
        let apps = sort_and_dedup_launchable_apps(apps);

        assert_eq!(
            apps,
            vec![
                LaunchableApp {
                    name: "Docs".to_string(),
                    target: "C:/Apps/Docs.url".to_string(),
                },
                LaunchableApp {
                    name: "editor".to_string(),
                    target: "C:/Apps/editor.exe".to_string(),
                },
                LaunchableApp {
                    name: "Editor".to_string(),
                    target: "C:/Apps/Editor.LnK".to_string(),
                },
                LaunchableApp {
                    name: "Remote".to_string(),
                    target: "C:/Apps/Remote.AppRef-Ms".to_string(),
                },
                LaunchableApp {
                    name: "Zeta".to_string(),
                    target: "C:/Apps/Zeta.EXE".to_string(),
                },
            ]
        );
    }

    #[test]
    fn selected_window_ids_validate_selection_shape() {
        let available = [11, 22, 33];
        assert_eq!(
            selected_window_indices(&available, &[33, 11]),
            Ok(vec![2, 0])
        );
        assert!(selected_window_indices(&available, &[]).is_err());
        assert!(selected_window_indices(&available, &[11, 11]).is_err());
        assert!(selected_window_indices(&available, &[44]).is_err());
        assert!(selected_window_indices(&available, &vec![11; MAX_WORKSPACE_WINDOWS + 1]).is_err());
    }
}
