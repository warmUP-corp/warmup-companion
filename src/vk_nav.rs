//! Gamepad-driven VK focus + full PC QWERTY grid (Joyxoff settings-style layout).

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(feature = "gamepad")]
use crate::gamepad_backend::Button;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_END,
    VK_RETURN, VK_SPACE, VK_TAB,
};

#[derive(Clone)]
pub enum KeyAction {
    Char(char),
    Vk(VIRTUAL_KEY),
    /// Shift: LT hold or on-screen shift keys (toggle).
    Shift,
    /// Caps lock toggle.
    CapsLock,
    /// Ctrl+V paste.
    Paste,
    /// Start background Windows speech recognition.
    VoiceInput,
    /// Dismiss the on-screen keyboard.
    CloseVk,
}

#[derive(Clone)]
pub struct KeyCell {
    pub label: String,
    /// Shifted symbol shown above the primary label (number row, etc.).
    pub sublabel: Option<String>,
    pub action: KeyAction,
    /// Width in key-units (1.0 = normal key).
    pub span: f32,
}

impl KeyCell {
    fn ch(c: char) -> Self {
        KeyCell {
            label: c.to_string(),
            sublabel: None,
            action: KeyAction::Char(c),
            span: 1.0,
        }
    }
    fn pair(base: char, shifted: char, shift: bool, caps: bool) -> Self {
        // Shift or caps alone → alternate symbol; both → base (same as letters).
        let alt = shift ^ caps;
        if alt {
            KeyCell {
                label: shifted.to_string(),
                sublabel: Some(base.to_string()),
                action: KeyAction::Char(shifted),
                span: 1.0,
            }
        } else {
            KeyCell {
                label: base.to_string(),
                sublabel: Some(shifted.to_string()),
                action: KeyAction::Char(base),
                span: 1.0,
            }
        }
    }
    fn alpha(c: char, shift: bool, caps: bool) -> Self {
        if shift ^ caps {
            let u = c.to_uppercase().next().unwrap_or(c);
            KeyCell {
                label: u.to_string(),
                sublabel: None,
                action: KeyAction::Char(u),
                span: 1.0,
            }
        } else {
            KeyCell::ch(c)
        }
    }
    fn named(label: &str, action: KeyAction, span: f32) -> Self {
        KeyCell {
            label: label.to_string(),
            sublabel: None,
            action,
            span,
        }
    }
    fn vk(label: &str, vk: VIRTUAL_KEY, span: f32) -> Self {
        KeyCell {
            label: label.to_string(),
            sublabel: None,
            action: KeyAction::Vk(vk),
            span,
        }
    }
    fn shift_key(span: f32) -> Self {
        KeyCell {
            label: "Shift".to_string(),
            sublabel: None,
            action: KeyAction::Shift,
            span,
        }
    }
}

#[derive(Clone)]
pub struct KeyRow {
    pub keys: Vec<KeyCell>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyPos {
    pub row: usize,
    pub col: usize,
}

struct NavState {
    pos: KeyPos,
    shift: bool,
    caps: bool,
    voice_input: bool,
    rows: Vec<KeyRow>,
    #[cfg(feature = "gamepad")]
    hold_button: Option<Button>,
    hold_count: u32,
    hold_deadline: Option<Instant>,
}

static NAV: Mutex<NavState> = Mutex::new(NavState {
    pos: KeyPos { row: 0, col: 0 },
    shift: false,
    caps: false,
    voice_input: false,
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

/// Every row spans this many key-units so the block is one rectangle.
const GRID_UNITS: f32 = 15.0;

/// Five-row US QWERTY matching the Joyxoff settings keyboard screenshot.
fn build_pc_layout(shift: bool, caps: bool) -> Vec<KeyRow> {
    let a = |c: char| KeyCell::alpha(c, shift, caps);
    let p = |base: char, shifted: char| KeyCell::pair(base, shifted, shift, caps);
    vec![
        KeyRow {
            keys: vec![
                p('`', '~'),
                p('1', '!'),
                p('2', '@'),
                p('3', '#'),
                p('4', '$'),
                p('5', '%'),
                p('6', '^'),
                p('7', '&'),
                p('8', '*'),
                p('9', '('),
                p('0', ')'),
                p('-', '_'),
                p('=', '+'),
                KeyCell::vk("Backspace", VK_BACK, 2.0),
            ],
        },
        KeyRow {
            keys: vec![
                KeyCell::vk("Tab", VK_TAB, 1.5),
                a('q'),
                a('w'),
                a('e'),
                a('r'),
                a('t'),
                a('y'),
                a('u'),
                a('i'),
                a('o'),
                a('p'),
                p('[', '{'),
                p(']', '}'),
                p('\\', '|'),
            ],
        },
        KeyRow {
            keys: vec![
                KeyCell::named("Caps", KeyAction::CapsLock, 1.75),
                a('a'),
                a('s'),
                a('d'),
                a('f'),
                a('g'),
                a('h'),
                a('j'),
                a('k'),
                a('l'),
                p(';', ':'),
                p('\'', '"'),
                KeyCell::vk("Enter", VK_RETURN, 2.25),
            ],
        },
        KeyRow {
            keys: vec![
                KeyCell::shift_key(2.25),
                a('z'),
                a('x'),
                a('c'),
                a('v'),
                a('b'),
                a('n'),
                a('m'),
                p(',', '<'),
                p('.', '>'),
                p('/', '?'),
                KeyCell::shift_key(2.75),
            ],
        },
        KeyRow {
            keys: vec![
                KeyCell::vk("", VK_SPACE, GRID_UNITS - 1.5 - 1.5 - 1.5),
                KeyCell {
                    label: "Mic".to_string(),
                    sublabel: Some("WIP".to_string()),
                    action: KeyAction::VoiceInput,
                    span: 1.5,
                },
                KeyCell::named("Paste", KeyAction::Paste, 1.5),
                KeyCell::named("", KeyAction::CloseVk, 1.5),
            ],
        },
    ]
}

fn rebuild(nav: &mut NavState) {
    nav.rows = build_pc_layout(nav.shift, nav.caps);
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
    let cols = nav.rows[nav.pos.row].keys.len();
    if cols == 0 {
        nav.pos.col = 0;
    } else if nav.pos.col >= cols {
        nav.pos.col = cols - 1;
    }
}

fn caps_lock_on() -> bool {
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 }
}

/// Reset focus when the keyboard opens.
pub fn reset_selection() {
    if let Ok(mut nav) = NAV.lock() {
        nav.shift = false;
        nav.caps = caps_lock_on();
        nav.pos = KeyPos { row: 2, col: 4 };
        #[cfg(feature = "gamepad")]
        {
            nav.hold_button = None;
        }
        nav.hold_count = 0;
        nav.hold_deadline = None;
        rebuild(&mut nav);
    }
    crate::vk_predict::reset();
}

pub fn selection() -> KeyPos {
    NAV.lock().map(|n| n.pos).unwrap_or_default()
}

pub fn rows_snapshot() -> Vec<KeyRow> {
    NAV.lock().map(|n| n.rows.clone()).unwrap_or_default()
}

pub fn selected_key() -> Option<KeyCell> {
    let nav = NAV.lock().ok()?;
    nav.rows.get(nav.pos.row)?.keys.get(nav.pos.col).cloned()
}

pub fn voice_input_active() -> bool {
    NAV.lock().map(|n| n.voice_input).unwrap_or(false)
}

pub fn set_voice_input_active(active: bool) {
    if let Ok(mut nav) = NAV.lock() {
        nav.voice_input = active;
    }
    request_ui_repaint();
}

fn voice_input_wip_disabled() -> bool {
    true
}

pub fn modifier_state() -> (bool, bool) {
    NAV.lock().map(|n| (n.shift, n.caps)).unwrap_or_default()
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
            // Wrap around the row: left from the first key lands on the last.
            let cols = nav.rows[pos.row].keys.len();
            if cols > 0 {
                pos.col = if pos.col > 0 { pos.col - 1 } else { cols - 1 };
                true
            } else {
                false
            }
        }
        Button::Right => {
            // Wrap around the row: right from the last key lands on the first.
            let cols = nav.rows[pos.row].keys.len();
            if cols > 0 {
                pos.col = if pos.col + 1 < cols { pos.col + 1 } else { 0 };
                true
            } else {
                false
            }
        }
        Button::Up => {
            if pos.row > 0 {
                pos.row -= 1;
                pos.col = pos.col.min(nav.rows[pos.row].keys.len().saturating_sub(1));
                true
            } else {
                false
            }
        }
        Button::Down => {
            if pos.row + 1 < nav.rows.len() {
                pos.row += 1;
                pos.col = pos.col.min(nav.rows[pos.row].keys.len().saturating_sub(1));
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
    let moved = move_selection(btn);
    refocus_after_nav_move();
    moved
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
    refocus_after_nav_move();
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

pub fn set_shift(on: bool) {
    let mut changed = false;
    if let Ok(mut nav) = NAV.lock() {
        if nav.shift != on {
            nav.shift = on;
            rebuild(&mut nav);
            changed = true;
        }
    }
    if changed || !on {
        request_ui_repaint();
    }
}

pub fn toggle_shift() {
    if let Ok(mut nav) = NAV.lock() {
        nav.shift = !nav.shift;
        rebuild(&mut nav);
    }
    request_ui_repaint();
}

pub fn toggle_caps() {
    if let Ok(mut nav) = NAV.lock() {
        nav.caps = !nav.caps;
        rebuild(&mut nav);
    }
    inject_vk(VK_CAPITAL);
    request_ui_repaint();
}

/// Layer cycle — no-op on the PC layout (kept for gamepad LB wiring).
pub fn next_layer() {
    request_ui_repaint();
}

pub fn activate_key(key: &KeyCell) {
    match &key.action {
        KeyAction::Char(c) => {
            send_unicode(&[*c as u16]);
            crate::vk_predict::on_char(*c);
        }
        KeyAction::Vk(vk) => {
            notify_vk_key(*vk);
            inject_vk(*vk);
        }
        KeyAction::Shift => toggle_shift(),
        KeyAction::CapsLock => toggle_caps(),
        KeyAction::Paste => send_paste(),
        KeyAction::VoiceInput => start_voice_input(),
        KeyAction::CloseVk => crate::win::vk_ui::request_hide(),
    }
}

/// Inject one character without updating prediction state (candidate commit path).
pub fn send_char_direct(c: char) {
    send_unicode(&[c as u16]);
}

pub fn send_text_direct(text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    send_unicode(&units);
}

fn notify_vk_key(vk: VIRTUAL_KEY) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN, VK_SPACE};
    if vk == VK_BACK {
        crate::vk_predict::on_backspace();
    } else if vk == VK_SPACE {
        crate::vk_predict::on_space();
    } else if vk == VK_RETURN {
        crate::vk_predict::on_boundary();
    } else {
        crate::vk_predict::on_boundary();
    }
}

fn request_ui_repaint() {
    crate::win::vk_ui::request_repaint();
}

pub fn backspace() {
    crate::vk_predict::on_backspace();
    inject_vk(VK_BACK);
}

/// `SendInput` only — no prediction side effects (candidate commit backspaces).
pub fn inject_backspace() {
    inject_vk(VK_BACK);
}

pub fn space() {
    crate::vk_predict::on_space();
    inject_vk(VK_SPACE);
}

pub fn enter() {
    crate::vk_predict::on_boundary();
    inject_vk(VK_RETURN);
}

fn send_paste() {
    let collapse = focus_for_inject();
    let v = VIRTUAL_KEY(b'V' as u16);
    let mut batch: Vec<INPUT> = Vec::with_capacity(6);
    // Caret-to-end first, as standalone End presses, so the select-on-focus
    // selection collapses before Ctrl is held (a held Ctrl would make it Ctrl+End).
    push_collapse(&mut batch, collapse);
    batch.push(vk_event(VK_CONTROL, false));
    batch.push(vk_event(v, false));
    batch.push(vk_event(v, true));
    batch.push(vk_event(VK_CONTROL, true));
    unsafe {
        let _ = SendInput(&batch, std::mem::size_of::<INPUT>() as i32);
    }
    suppress_native_keyboard_after_winlogon_inject(collapse);
}

pub fn start_voice_input() {
    if voice_input_wip_disabled() {
        crate::install::log_line("vk voice input ignored: WIP");
        set_voice_input_active(false);
        return;
    }

    if crate::win::logon_focus::is_active() {
        crate::install::log_line("vk voice input ignored on Winlogon");
        return;
    }

    if crate::win::speech_input::is_active() && voice_input_active() {
        crate::win::speech_input::stop();
        set_voice_input_active(false);
        return;
    }
    if crate::win::speech_input::is_active() {
        crate::win::speech_input::stop();
    }

    set_voice_input_active(true);
    if let Err(e) = crate::win::speech_input::start() {
        set_voice_input_active(false);
        crate::install::log_line(&format!("speech input start failed: {e}"));
    }
}

/// Focus the credential field for an inject. Returns true on Winlogon, where the
/// edit selects its entire contents on focus — the caller must then lead its
/// `SendInput` batch with a caret-to-end (`VK_END`) so the collapse and the key
/// land in the *same* injection. Two separate sends let the target re-select
/// (or re-process focus) between them, so every key after the first overwrites
/// the selection. False off Winlogon, where normal caret rules apply.
fn focus_for_inject() -> bool {
    crate::win::logon_focus::focus_password_field()
}

#[cfg(feature = "gamepad")]
fn refocus_after_nav_move() {
    let _ = crate::win::logon_focus::focus_password_field();
}

/// Build a virtual-key down (or up) `INPUT`.
fn vk_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Build a Unicode-scancode down (or up) `INPUT`.
fn unicode_event(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Push a caret-to-end (`VK_END`) down+up onto a Winlogon inject batch so the
/// select-on-focus selection collapses in the same injection as the key. No-op
/// off Winlogon.
fn push_collapse(batch: &mut Vec<INPUT>, on_winlogon: bool) {
    if on_winlogon {
        batch.push(vk_event(VK_END, false));
        batch.push(vk_event(VK_END, true));
    }
}

fn inject_vk(vk: VIRTUAL_KEY) {
    let collapse = focus_for_inject();
    let mut batch: Vec<INPUT> = Vec::with_capacity(4);
    push_collapse(&mut batch, collapse);
    batch.push(vk_event(vk, false));
    batch.push(vk_event(vk, true));
    unsafe {
        let _ = SendInput(&batch, std::mem::size_of::<INPUT>() as i32);
    }
    suppress_native_keyboard_after_winlogon_inject(collapse);
}

fn send_unicode(units: &[u16]) {
    let collapse = focus_for_inject();
    let mut batch: Vec<INPUT> = Vec::with_capacity(units.len() * 2 + 2);
    push_collapse(&mut batch, collapse);
    for &unit in units {
        batch.push(unicode_event(unit, false));
        batch.push(unicode_event(unit, true));
    }
    let sent = unsafe { SendInput(&batch, std::mem::size_of::<INPUT>() as i32) };
    suppress_native_keyboard_after_winlogon_inject(collapse);
    // Userland-typing diagnostic: when off Winlogon, SendInput should land in the
    // foreground app. Log the event count actually inserted + the loop thread's
    // desktop + foreground window so a misrouted inject (wrong desktop /
    // not-foreground / blocked) is visible in the log.
    #[cfg(feature = "gamepad")]
    if !crate::win::logon_focus::is_active() {
        let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        let desk = crate::win::current_desktop_name().unwrap_or_default();
        crate::install::log_line(&format!(
            "vk inject(userland): units={} SendInput->{sent} desktop={desk} fg=0x{:x}",
            units.len(),
            fg.0 as usize
        ));
    }
}

fn suppress_native_keyboard_after_winlogon_inject(on_winlogon: bool) {
    if on_winlogon {
        crate::win::native_keyboard::suppress_for(Duration::from_millis(300));
    }
}

#[allow(dead_code)]
fn active_langid() -> u32 {
    let hkl = unsafe { GetKeyboardLayout(0) };
    (hkl.0 as usize as u32) & 0xffff
}
