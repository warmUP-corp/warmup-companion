//! Gamepad-driven VK focus + key grid, locale + layer aware (Joyxoff parity).
//!
//! The visible keyboard mirrors `Joyxoff.exe`'s `JoyXboxVkWindow`:
//! a shared digit row (`vk_layouts::DIGITS`) plus the active locale's letter
//! page (`vk_layouts::LAYOUTS[..]`), with an on-keyboard `Shift` toggle and a
//! `?123`/`ABC` layer toggle that cycles the locale's symbol / extra pages.
//! Locale is chosen from the active keyboard layout exactly like Joyxoff's
//! `FUN_00429790` (ported in [`crate::win::vk_layouts::index_for_langid`]).

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(feature = "gamepad")]
use crate::gamepad_backend::Button;
use crate::win::vk_layouts;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE,
};

/// Keys per grid row (matches the shared 10-wide digit row).
const COLS: usize = 10;
/// Layer index of the shared symbol page (Joyxoff `dword[1]`); the only page with
/// no digit row and no Shift key.
const SYMBOL_LAYER: usize = 1;

#[derive(Clone)]
pub enum KeyAction {
    Char(char),
    Vk(VIRTUAL_KEY),
    /// Sticky upper-case toggle for the letter page (Joyxoff `Shift`/`Caps Lock`).
    Shift,
    /// Cycle letters -> symbols -> (locale extra) -> letters (Joyxoff `?123`/`ABC`).
    ToggleLayer,
}

#[derive(Clone)]
pub struct KeyCell {
    pub label: String,
    pub action: KeyAction,
}

impl KeyCell {
    fn ch(c: char) -> Self {
        KeyCell { label: c.to_string(), action: KeyAction::Char(c) }
    }
    fn vk(label: &str, vk: VIRTUAL_KEY) -> Self {
        KeyCell { label: label.to_string(), action: KeyAction::Vk(vk) }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyPos {
    pub row: usize,
    pub col: usize,
}

struct NavState {
    pos: KeyPos,
    /// Index into [`vk_layouts::LAYOUTS`] for the active keyboard locale.
    layout_idx: usize,
    /// Active page within the locale (0 = letters).
    layer: usize,
    /// Upper-case the letter page.
    shift: bool,
    /// Materialised grid for the current (layout, layer, shift).
    rows: Vec<Vec<KeyCell>>,
    #[cfg(feature = "gamepad")]
    hold_button: Option<Button>,
    hold_count: u32,
    hold_deadline: Option<Instant>,
}

static NAV: Mutex<NavState> = Mutex::new(NavState {
    pos: KeyPos { row: 0, col: 0 },
    layout_idx: 1, // en-US fallback
    layer: 0,
    shift: false,
    rows: Vec::new(),
    #[cfg(feature = "gamepad")]
    hold_button: None,
    hold_count: 0,
    hold_deadline: None,
});

#[cfg(feature = "gamepad")]
const HOLD_INITIAL: Duration = Duration::from_millis(350);
#[cfg(feature = "gamepad")]
const HOLD_REPEAT: Duration = Duration::from_millis(70);

/// LANGID of the active keyboard layout (low word of the HKL).
fn active_langid() -> u32 {
    let hkl = unsafe { GetKeyboardLayout(0) };
    (hkl.0 as usize as u32) & 0xffff
}

fn page_chars(idx: usize, layer: usize) -> Vec<char> {
    vk_layouts::LAYOUTS
        .get(idx)
        .and_then(|l| l.layers.get(layer))
        .map(|s| s.chars().collect())
        .unwrap_or_default()
}

fn layer_count(idx: usize) -> usize {
    vk_layouts::LAYOUTS.get(idx).map(|l| l.layers.len()).unwrap_or(1)
}

/// Materialise the grid for a (layout, layer, shift) tuple.
fn build_rows(idx: usize, layer: usize, shift: bool) -> Vec<Vec<KeyCell>> {
    let mut rows: Vec<Vec<KeyCell>> = Vec::new();

    // The symbol page (layer 1) is the only page Joyxoff builds without the shared
    // digit-row header and without a Shift key; every other page (letters, locale
    // extra, ABC) gets both.
    let is_symbols = layer == SYMBOL_LAYER;

    if !is_symbols {
        rows.push(vk_layouts::DIGITS.chars().map(KeyCell::ch).collect());
    }

    // Caps upper-cases the whole active page (Joyxoff applies LCMapStringW upper to
    // the page buffer when Caps Lock is on). Harmless on digits/symbols.
    let chars = page_chars(idx, layer);
    for chunk in chars.chunks(COLS) {
        rows.push(
            chunk
                .iter()
                .map(|&c| {
                    if shift {
                        // to_uppercase() may yield >1 char (e.g. ß -> SS); take the
                        // primary mapping, falling back to the original.
                        let u = c.to_uppercase().next().unwrap_or(c);
                        KeyCell { label: u.to_string(), action: KeyAction::Char(u) }
                    } else {
                        KeyCell::ch(c)
                    }
                })
                .collect(),
        );
    }

    // Special-key row (Joyxoff: Shift, Caps Lock, Backspace, Enter, Space bar). The
    // layer toggle cycles letters -> symbols -> (locale extra) -> ABC pages.
    let abc_start = vk_layouts::LAYOUTS.get(idx).map(|l| l.abc_start).unwrap_or(usize::MAX);
    let toggle_label = if layer == 0 {
        "?123"
    } else if layer + 1 >= abc_start && layer < abc_start {
        "ABC"
    } else if layer >= abc_start {
        "QWE"
    } else {
        "ABC"
    };
    let mut special = vec![
        KeyCell { label: "Shift".into(), action: KeyAction::Shift },
        KeyCell { label: toggle_label.into(), action: KeyAction::ToggleLayer },
        KeyCell::vk("Space", VK_SPACE),
        KeyCell::vk("Bksp", VK_BACK),
        KeyCell::vk("Enter", VK_RETURN),
    ];
    if is_symbols {
        special.remove(0);
    }
    rows.push(special);

    rows
}

fn rebuild(nav: &mut NavState) {
    nav.rows = build_rows(nav.layout_idx, nav.layer, nav.shift);
    clamp_pos(nav);
}

fn clamp_pos(nav: &mut NavState) {
    if nav.rows.is_empty() {
        nav.pos = KeyPos::default();
        return;
    }
    if nav.pos.row >= nav.rows.len() {
        nav.pos.row = nav.rows.len() - 1;
    }
    let cols = nav.rows[nav.pos.row].len();
    if cols == 0 {
        nav.pos.col = 0;
    } else if nav.pos.col >= cols {
        nav.pos.col = cols - 1;
    }
}

/// Reset focus and re-pick the locale layout from the active keyboard.
pub fn reset_selection() {
    if let Ok(mut nav) = NAV.lock() {
        nav.layout_idx = vk_layouts::index_for_langid(active_langid());
        nav.layer = 0;
        nav.shift = false;
        nav.pos = KeyPos { row: 0, col: 0 };
        #[cfg(feature = "gamepad")]
        {
            nav.hold_button = None;
        }
        nav.hold_count = 0;
        nav.hold_deadline = None;
        rebuild(&mut nav);
    }
}

pub fn selection() -> KeyPos {
    NAV.lock().map(|n| n.pos).unwrap_or_default()
}

/// Snapshot of the current grid for painting / hit-testing.
pub fn rows_snapshot() -> Vec<Vec<KeyCell>> {
    NAV.lock().map(|n| n.rows.clone()).unwrap_or_default()
}

pub fn selected_key() -> Option<KeyCell> {
    let nav = NAV.lock().ok()?;
    nav.rows.get(nav.pos.row)?.get(nav.pos.col).cloned()
}

#[cfg(feature = "gamepad")]
pub fn move_selection(dir: Button) -> bool {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return false,
    };
    if nav.rows.is_empty() {
        return false;
    }
    let mut pos = nav.pos;
    let changed = match dir {
        Button::Left => {
            if pos.col > 0 {
                pos.col -= 1;
                true
            } else {
                false
            }
        }
        Button::Right => {
            if pos.col + 1 < nav.rows[pos.row].len() {
                pos.col += 1;
                true
            } else {
                false
            }
        }
        Button::Up => {
            if pos.row > 0 {
                pos.row -= 1;
                pos.col = pos.col.min(nav.rows[pos.row].len().saturating_sub(1));
                true
            } else {
                false
            }
        }
        Button::Down => {
            if pos.row + 1 < nav.rows.len() {
                pos.row += 1;
                pos.col = pos.col.min(nav.rows[pos.row].len().saturating_sub(1));
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if changed {
        nav.pos = pos;
    }
    changed
}

/// D-pad with hold-to-repeat (call each frame).
#[cfg(feature = "gamepad")]
pub fn tick_dpad_hold(now: Instant) -> bool {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let Some(btn) = nav.hold_button else {
        return false;
    };
    let Some(deadline) = nav.hold_deadline else {
        return false;
    };
    if now < deadline {
        return false;
    }
    nav.hold_count += 1;
    nav.hold_deadline = Some(now + HOLD_REPEAT);
    drop(nav);
    move_selection(btn)
}

#[cfg(feature = "gamepad")]
pub fn dpad_pressed(dir: Button) {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return,
    };
    nav.hold_button = Some(dir);
    nav.hold_count = 0;
    nav.hold_deadline = Some(Instant::now() + HOLD_INITIAL);
    drop(nav);
    let _ = move_selection(dir);
}

#[cfg(feature = "gamepad")]
pub fn dpad_released(dir: Button) {
    let Ok(mut nav) = NAV.lock() else {
        return;
    };
    if nav.hold_button == Some(dir) {
        nav.hold_button = None;
        nav.hold_count = 0;
        nav.hold_deadline = None;
    }
}

pub fn activate_selection() {
    if let Some(key) = selected_key() {
        activate_key(&key);
    }
}

pub fn activate_key(key: &KeyCell) {
    match &key.action {
        KeyAction::Char(c) => send_unicode(&[*c as u16]),
        KeyAction::Vk(vk) => send_vk(*vk),
        KeyAction::Shift => {
            if let Ok(mut nav) = NAV.lock() {
                nav.shift = !nav.shift;
                rebuild(&mut nav);
            }
            request_ui_repaint();
        }
        KeyAction::ToggleLayer => {
            if let Ok(mut nav) = NAV.lock() {
                let count = layer_count(nav.layout_idx).max(1);
                nav.layer = (nav.layer + 1) % count;
                nav.shift = false;
                rebuild(&mut nav);
            }
            request_ui_repaint();
        }
    }
}

fn request_ui_repaint() {
    crate::win::vk_ui::request_repaint();
}

pub fn backspace() {
    send_vk(VK_BACK);
}

pub fn space() {
    send_vk(VK_SPACE);
}

pub fn enter() {
    send_vk(VK_RETURN);
}

pub fn cursor_left() {
    send_vk(VK_LEFT);
}

pub fn cursor_right() {
    send_vk(VK_RIGHT);
}

fn send_vk(vk: VIRTUAL_KEY) {
    // On Winlogon, re-target the credential password element before injecting so
    // CoreWindow nav drift can't divert the key (no-op off winlogon/loop thread).
    crate::win::logon_focus::focus_password_field();
    unsafe {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let _ = SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_unicode(units: &[u16]) {
    // Re-target the credential password element once before the char(s).
    crate::win::logon_focus::focus_password_field();
    for &unit in units {
        unsafe {
            let down = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let up = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let _ = SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32);
        }
    }
}
