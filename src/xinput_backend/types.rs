//! Shared types and constants for XInput / secure poll.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Instant;

use windows::Win32::UI::Input::XboxController::XINPUT_STATE;

use crate::gamepad_backend::ButtonChange;
use crate::hid_gamepad::{self, PadSample};
use crate::xusb_ioctl::{XusbDevice, XusbReport};

pub(crate) const SLOTS: u32 = 4;
pub(crate) const ERROR_SUCCESS: u32 = 0;
pub(crate) const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
pub(crate) const ERROR_EMPTY: u32 = 4306;

pub(crate) type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;
pub(crate) type XInputGetKeystrokeFn =
    unsafe extern "system" fn(u32, u32, *mut XInputKeystroke) -> u32;

pub(crate) const XINPUT_KEYSTROKE_KEYDOWN: u16 = 0x0001;
pub(crate) const XINPUT_KEYSTROKE_KEYUP: u16 = 0x0002;

pub(crate) const VK_PAD_A: u16 = 0x5800;
pub(crate) const VK_PAD_B: u16 = 0x5801;
pub(crate) const VK_PAD_X: u16 = 0x5802;
pub(crate) const VK_PAD_Y: u16 = 0x5803;
pub(crate) const VK_PAD_LSHOULDER: u16 = 0x5804;
pub(crate) const VK_PAD_RSHOULDER: u16 = 0x5805;
pub(crate) const VK_PAD_DPAD_UP: u16 = 0x5810;
pub(crate) const VK_PAD_DPAD_DOWN: u16 = 0x5811;
pub(crate) const VK_PAD_DPAD_LEFT: u16 = 0x5812;
pub(crate) const VK_PAD_DPAD_RIGHT: u16 = 0x5813;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct XInputKeystroke {
    pub virtual_key: u16,
    pub unicode: u16,
    pub flags: u16,
    pub user_index: u8,
    pub hid_code: u8,
}

pub(crate) struct PollState {
    pub get_state: XInputGetStateFn,
    pub get_keystroke: Option<XInputGetKeystrokeFn>,
    pub tx: mpsc::Sender<SecureMsg>,
    pub prev_buttons: [u16; 4],
    pub active_slot: Option<u32>,
    pub connected_prev: [bool; 4],
    pub last_status: Instant,
    pub last_no_pad: Instant,
    pub last_probe_log: Instant,
    pub iter_count: u64,
    pub hid_devices: HashMap<usize, hid_gamepad::DeviceState>,
    /// Direct HID input-report readers (PlayStation / generic pads). Windowed
    /// Raw Input (`WM_INPUT`) is never delivered on the secure desktop, so these
    /// CreateFile/ReadFile handles are the only working source for vendor pads —
    /// the HID analogue of the `xusb` DeviceIoControl bypass for Xbox.
    pub hid_readers: Vec<crate::hid_reader::HidReader>,
    /// Throttle for re-enumerating HID readers (pad plugged in after spawn).
    pub last_hid_scan: Instant,
    pub last_hid: PadSample,
    pub hid_diag_count: u32,
    pub suppress_until_zero: bool,
    /// Diagnostics: count of XInputGetKeystroke events seen since spawn, and the
    /// most recent raw HID report. Surfaced in the 2s probe so the next deploy
    /// shows definitively whether Y arrives via keystroke or which HID byte moves.
    pub keystroke_events: u64,
    pub last_raw_report: Vec<u8>,
    /// Physical XUSB pads opened via direct DeviceIoControl. These bypass the
    /// XInput foreground focus gate that zeroes `get_state` on Winlogon.
    pub xusb: Vec<XusbDevice>,
    /// Most recent XUSB report (for the probe dump + offset verification).
    pub last_xusb: Option<XusbReport>,
    pub hid_active_prev: bool,
    /// Throttle for re-enumerating XUSB pads: `open_all` runs once at startup, but a
    /// pad plugged in later (or present only after the secure desktop appears) must
    /// be picked up, else we have 0 devices and fall back to the foreground-gated DLL.
    pub last_xusb_scan: Instant,
    /// Throttle for the non-fatal `XInputGetState error` log on the secure thread.
    pub last_slot_err_log: Instant,
    pub prev_trigger_left: bool,
    pub prev_trigger_right: bool,
}

pub(crate) enum SecureMsg {
    Ready(String),
    Slots([bool; 4]),
    Buttons { slot: u32, prev: u16, cur: u16 },
    Trigger(ButtonChange),
    HidActive(bool),
    Axes((f32, f32, f32, f32)),
    NoController,
    Error(String),
}
