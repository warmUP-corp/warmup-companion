//! Gamepad-driven VK focus + key grid (warmUP-style).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY, VK_BACK, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE,
};

#[derive(Clone, Copy)]
pub struct KeyCell {
    pub label: &'static str,
    pub ch: char,
    pub vk: Option<VIRTUAL_KEY>,
}

pub const ROWS: &[&[KeyCell]] = &[
    &[
        KeyCell { label: "1", ch: '1', vk: None },
        KeyCell { label: "2", ch: '2', vk: None },
        KeyCell { label: "3", ch: '3', vk: None },
        KeyCell { label: "4", ch: '4', vk: None },
        KeyCell { label: "5", ch: '5', vk: None },
        KeyCell { label: "6", ch: '6', vk: None },
        KeyCell { label: "7", ch: '7', vk: None },
        KeyCell { label: "8", ch: '8', vk: None },
        KeyCell { label: "9", ch: '9', vk: None },
        KeyCell { label: "0", ch: '0', vk: None },
    ],
    &[
        KeyCell { label: "Q", ch: 'q', vk: None },
        KeyCell { label: "W", ch: 'w', vk: None },
        KeyCell { label: "E", ch: 'e', vk: None },
        KeyCell { label: "R", ch: 'r', vk: None },
        KeyCell { label: "T", ch: 't', vk: None },
        KeyCell { label: "Y", ch: 'y', vk: None },
        KeyCell { label: "U", ch: 'u', vk: None },
        KeyCell { label: "I", ch: 'i', vk: None },
        KeyCell { label: "O", ch: 'o', vk: None },
        KeyCell { label: "P", ch: 'p', vk: None },
    ],
    &[
        KeyCell { label: "A", ch: 'a', vk: None },
        KeyCell { label: "S", ch: 's', vk: None },
        KeyCell { label: "D", ch: 'd', vk: None },
        KeyCell { label: "F", ch: 'f', vk: None },
        KeyCell { label: "G", ch: 'g', vk: None },
        KeyCell { label: "H", ch: 'h', vk: None },
        KeyCell { label: "J", ch: 'j', vk: None },
        KeyCell { label: "K", ch: 'k', vk: None },
        KeyCell { label: "L", ch: 'l', vk: None },
    ],
    &[
        KeyCell { label: "Z", ch: 'z', vk: None },
        KeyCell { label: "X", ch: 'x', vk: None },
        KeyCell { label: "C", ch: 'c', vk: None },
        KeyCell { label: "V", ch: 'v', vk: None },
        KeyCell { label: "B", ch: 'b', vk: None },
        KeyCell { label: "N", ch: 'n', vk: None },
        KeyCell { label: "M", ch: 'm', vk: None },
        KeyCell {
            label: "Bksp",
            ch: '\0',
            vk: Some(VK_BACK),
        },
    ],
    &[
        KeyCell {
            label: "Space",
            ch: '\0',
            vk: Some(VK_SPACE),
        },
        KeyCell {
            label: "Enter",
            ch: '\0',
            vk: Some(VK_RETURN),
        },
    ],
];

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyPos {
    pub row: usize,
    pub col: usize,
}

struct NavState {
    pos: KeyPos,
    hold_button: Option<&'static str>,
    hold_count: u32,
    hold_deadline: Option<Instant>,
}

static NAV: Mutex<NavState> = Mutex::new(NavState {
    pos: KeyPos { row: 0, col: 0 },
    hold_button: None,
    hold_count: 0,
    hold_deadline: None,
});

const HOLD_INITIAL: Duration = Duration::from_millis(350);
const HOLD_REPEAT: Duration = Duration::from_millis(70);

pub fn reset_selection() {
    if let Ok(mut nav) = NAV.lock() {
        nav.pos = KeyPos { row: 0, col: 0 };
        nav.hold_button = None;
        nav.hold_count = 0;
        nav.hold_deadline = None;
    }
}

pub fn selection() -> KeyPos {
    NAV.lock().map(|n| n.pos).unwrap_or_default()
}

pub fn selected_key() -> KeyCell {
    let pos = selection();
    ROWS[pos.row][pos.col]
}

pub fn move_selection(dir: &str) -> bool {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let row_count = ROWS.len();
    let mut pos = nav.pos;
    let changed = match dir {
        "LEFT" => {
            if pos.col > 0 {
                pos.col -= 1;
                true
            } else {
                false
            }
        }
        "RIGHT" => {
            if pos.col + 1 < ROWS[pos.row].len() {
                pos.col += 1;
                true
            } else {
                false
            }
        }
        "UP" => {
            if pos.row > 0 {
                pos.row -= 1;
                pos.col = pos.col.min(ROWS[pos.row].len().saturating_sub(1));
                true
            } else {
                false
            }
        }
        "DOWN" => {
            if pos.row + 1 < row_count {
                pos.row += 1;
                pos.col = pos.col.min(ROWS[pos.row].len().saturating_sub(1));
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

pub fn dpad_pressed(dir: &'static str) {
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

pub fn dpad_released(dir: &str) {
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
    send_key(selected_key());
}

pub fn activate_key(key: KeyCell) {
    send_key(key);
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

fn send_key(key: KeyCell) {
    #[cfg(windows)]
    crate::win::vk_ui::focus_text_target();
    if let Some(vk) = key.vk {
        send_vk(vk);
        #[cfg(windows)]
        crate::win::vk_ui::refocus_vk();
        return;
    }
    if key.ch != '\0' {
        send_unicode(&[key.ch as u16]);
    }
    #[cfg(windows)]
    crate::win::vk_ui::refocus_vk();
}

fn send_vk(vk: VIRTUAL_KEY) {
    #[cfg(windows)]
    crate::win::vk_ui::focus_text_target();
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
    #[cfg(windows)]
    crate::win::vk_ui::refocus_vk();
}

fn send_unicode(units: &[u16]) {
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
