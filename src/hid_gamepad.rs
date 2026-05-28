//! Vendor-agnostic gamepad decoding for Winlogon (raw HID + HidP).
//!
//! Maps Xbox, DualShock 4, DualSense, and generic HID gamepads to the same
//! XInput-style button mask used by `gamepad.rs` / `vk_nav` (Y, A, D-pad, …).
//! `gamecontrollerdb.txt` (SDL format) seeds VID:PID → layout when present.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use windows::Win32::Devices::HumanInterfaceDevice::{
    HidP_GetUsages, HidP_GetUsagesEx, HidP_Input, HIDP_STATUS_SUCCESS, PHIDP_PREPARSED_DATA,
    USAGE_AND_PAGE,
};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT,
    XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER,
    XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y,
};
use windows::Win32::UI::Input::{
    GetRawInputDeviceInfoW, RIDI_DEVICEINFO, RIDI_DEVICENAME, RIDI_PREPARSEDDATA, RAWINPUT,
    RID_DEVICE_INFO, RIM_TYPEHID,
};

/// Buttons + normalized sticks (same convention as `XInputBackend::norm_thumb`).
#[derive(Clone, Copy, Debug, Default)]
pub struct PadSample {
    pub buttons: u16,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidProfile {
    /// Wired / wireless Xbox via XUSB byte layout (LE u16 @ 2–3).
    Xusb,
    /// Raw Input report is noisy/non-button data; keep bytes for diagnostics only.
    XusbButtonByte,
    /// DualShock 4 / DualSense USB report id 0x01.
    SonyUsb,
    /// DualShock 4 Bluetooth report id 0x01 (buttons @ 8–9).
    SonyBluetooth,
    /// Prefer `HidP_GetUsages` on button page 0x09 only.
    GenericHidP,
}

impl HidProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Xusb => "xusb",
            Self::XusbButtonByte => "xusb-raw",
            Self::SonyUsb => "sony-usb",
            Self::SonyBluetooth => "sony-bt",
            Self::GenericHidP => "hidp",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GcdbHint {
    vid: u16,
    pid: u16,
    profile: HidProfile,
}

static GCDB_HINTS: OnceLock<Vec<GcdbHint>> = OnceLock::new();

pub fn init_from_gcdb(path: &Path) -> usize {
    let hints = load_gcdb_hints(path);
    let n = hints.len();
    let _ = GCDB_HINTS.set(hints);
    n
}

fn gcdb_hints() -> &'static [GcdbHint] {
    GCDB_HINTS.get().map(|v| v.as_slice()).unwrap_or(&[])
}

/// Per-`hDevice` state cached on the secure poll thread.
pub struct DeviceState {
    pub handle_key: usize,
    pub vid: u16,
    pub pid: u16,
    pub profile: HidProfile,
    pub name: String,
    /// LE `wButtons` byte offset for [`HidProfile::Xusb`] (`255` = not calibrated).
    xusb_btn_offset: u8,
    preparsed_storage: Vec<usize>,
}

impl DeviceState {
    pub fn preparsed(&mut self) -> Option<PHIDP_PREPARSED_DATA> {
        if self.preparsed_storage.is_empty() {
            return None;
        }
        Some(PHIDP_PREPARSED_DATA(
            self.preparsed_storage.as_mut_ptr() as isize,
        ))
    }

    pub fn calibrate_xusb_from_report(&mut self, report: &[u8]) {
        if self.profile != HidProfile::Xusb {
            return;
        }
        self.xusb_btn_offset = calibrate_xusb_button_offset(report);
    }

    fn xusb_button_offset(&self, report: &[u8]) -> usize {
        if self.xusb_btn_offset != u8::MAX {
            return self.xusb_btn_offset as usize;
        }
        default_xusb_button_offset(report) as usize
    }
}

pub fn open_device(hdevice: isize) -> Option<DeviceState> {
    let (vid, pid, name) = device_vid_pid_name(hdevice)?;
    let profile = profile_for_vid_pid(vid, pid, &name);
    let handle_key = hdevice as usize;
    let mut preparsed_storage = Vec::new();
    let mut bytes = 0u32;
    let handle = HANDLE(hdevice as _);
    unsafe {
        GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, None, &mut bytes);
    }
    if bytes > 0 {
        let words = (bytes as usize).div_ceil(size_of::<usize>());
        preparsed_storage.resize(words, 0);
        let got = unsafe {
            GetRawInputDeviceInfoW(
                handle,
                RIDI_PREPARSEDDATA,
                Some(preparsed_storage.as_mut_ptr().cast()),
                &mut bytes,
            )
        };
        if got == u32::MAX || got == 0 {
            preparsed_storage.clear();
        }
    }
    Some(DeviceState {
        handle_key,
        vid,
        pid,
        profile,
        name,
        xusb_btn_offset: u8::MAX,
        preparsed_storage,
    })
}

pub fn profile_for_vid_pid(vid: u16, pid: u16, name: &str) -> HidProfile {
    for hint in gcdb_hints() {
        if hint.vid == vid && hint.pid == pid {
            return hint.profile;
        }
    }
    let n = name.to_ascii_lowercase();
    if vid == 0x054c {
        if n.contains("wireless") || n.contains("bluetooth") {
            return HidProfile::SonyBluetooth;
        }
        return HidProfile::SonyUsb;
    }
    if vid == 0x045e && pid == 0x02ff {
        return HidProfile::XusbButtonByte;
    }
    if vid == 0x045e {
        return HidProfile::Xusb;
    }
    if n.contains("xbox") || n.contains("xinput") {
        return HidProfile::Xusb;
    }
    if n.contains("dualsense")
        || n.contains("dualshock")
        || n.contains("playstation")
        || n.contains("ps4")
        || n.contains("ps5")
    {
        if n.contains("bluetooth") || n.contains("wireless") {
            return HidProfile::SonyBluetooth;
        }
        return HidProfile::SonyUsb;
    }
    HidProfile::GenericHidP
}

pub fn decode_report_logged(device: &mut DeviceState, report: &[u8]) -> (PadSample, &'static str) {
    let preparsed = device.preparsed();
    let (buttons, source) = match device.profile {
        // Xbox XUSB: use bytes 2–3 only. HidP on Winlogon often reports every
        // button usage at once (e.g. 0xb200) on the first report after open.
        HidProfile::Xusb => {
            let off = device.xusb_button_offset(report);
            let b = parse_xusb_mask_at(report, off).unwrap_or(0);
            (b, if b != 0 { "xusb" } else { "" })
        }
        HidProfile::XusbButtonByte => {
            let _ = report;
            (0, "")
        }
        HidProfile::SonyUsb => {
            let b = parse_sony_usb_mask(report);
            (b, if b != 0 { "sony-usb" } else { "" })
        }
        HidProfile::SonyBluetooth => {
            let b = parse_sony_bt_mask(report);
            (b, if b != 0 { "sony-bt" } else { "" })
        }
        HidProfile::GenericHidP => {
            let mut b = hidp_button_mask(preparsed, report);
            let mut src = if b != 0 { "hidp" } else { "" };
            if b == 0 {
                b = bruteforce_button_mask(report).unwrap_or(0);
                if b != 0 {
                    src = "scan";
                }
            }
            (b, src)
        }
    };

    // Do not zero buttons when sticks move — Winlogon XInput often has live
    // thumb noise with btn=0x0000; that was blocking all HID face buttons.

    let (lx, ly, rx, ry) = sticks_for_profile(device.profile, report);
    (
        PadSample {
            buttons,
            lx,
            ly,
            rx,
            ry,
        },
        source,
    )
}

const XUSB_BUTTON_MASK: u16 = 0xF3FF;
const STICK_DEADZONE: i16 = 7849;

fn default_xusb_button_offset(report: &[u8]) -> u8 {
    // Microsoft XUSB HID: report id @0, `wButtons` LE @1–2.
    if !report.is_empty() && report[0] <= 0x0f {
        1
    } else {
        0
    }
}

fn calibrate_xusb_button_offset(report: &[u8]) -> u8 {
    let default = default_xusb_button_offset(report);
    let mut any_nonzero = false;
    for off in 0..=4usize {
        if off + 2 > report.len() {
            continue;
        }
        let raw = u16::from_le_bytes([report[off], report[off + 1]]);
        if raw & XUSB_BUTTON_MASK != 0 {
            any_nonzero = true;
            break;
        }
    }
    if !any_nonzero {
        return default;
    }

    let mut best_off = default;
    let mut best_score = i32::MIN;
    for off in 0..=4usize {
        if off + 2 > report.len() {
            continue;
        }
        let raw = u16::from_le_bytes([report[off], report[off + 1]]);
        let masked = raw & XUSB_BUTTON_MASK;
        let face = (masked & 0xF000).count_ones() as i32;
        let dpad_all = if masked & 0x000F == 0x000F { 50 } else { 0 };
        let layout_bonus = if off == default as usize { 2 } else { 0 };
        let score = layout_bonus - (masked.count_ones() as i32) - face * 10 - dpad_all;
        if score > best_score {
            best_score = score;
            best_off = off as u8;
        }
    }
    best_off
}

fn parse_xusb_mask_at(report: &[u8], off: usize) -> Option<u16> {
    if off + 2 > report.len() {
        return None;
    }
    plausible_mask(u16::from_le_bytes([report[off], report[off + 1]]))
}

fn bruteforce_button_mask(report: &[u8]) -> Option<u16> {
    for off in 0..report.len().saturating_sub(1) {
        if let Some(masked) = plausible_mask(u16::from_le_bytes([report[off], report[off + 1]])) {
            return Some(masked);
        }
    }
    None
}

fn plausible_mask(raw: u16) -> Option<u16> {
    let masked = raw & XUSB_BUTTON_MASK;
    if masked == 0 {
        return None;
    }
    // Ignore trigger-only bytes misread as LE 0x00?? at offset 2.
    if masked & 0xF000 == 0 && masked & 0x0300 == 0 && masked & 0x000F == 0 {
        return None;
    }
    // Idle/connect garbage often sets several face buttons at once (e.g. 0xb200).
    const FACE: u16 = 0xF000;
    if (masked & FACE).count_ones() > 1 {
        return None;
    }
    // Trigger bytes misread as LE often look like all dpad bits (e.g. 0x005f).
    if masked & 0x000F == 0x000F {
        return None;
    }
    Some(masked)
}

fn parse_sony_usb_mask(report: &[u8]) -> u16 {
    if report.len() < 7 {
        return 0;
    }
    if report[0] != 0x01 && report[0] != 0x00 {
        return 0;
    }
    sony_buttons_from_pair(report[5], report[6])
}

fn parse_sony_bt_mask(report: &[u8]) -> u16 {
    if report.len() < 10 {
        return 0;
    }
    if report[0] != 0x01 {
        return 0;
    }
    sony_buttons_from_pair(report[8], report[9])
}

fn sony_buttons_from_pair(b5: u8, b6: u8) -> u16 {
    let mut mask = ds4_dpad_hat(b5 & 0x0f);
    if b5 & 0x10 != 0 {
        mask |= XINPUT_GAMEPAD_X.0;
    }
    if b5 & 0x20 != 0 {
        mask |= XINPUT_GAMEPAD_A.0;
    }
    if b5 & 0x40 != 0 {
        mask |= XINPUT_GAMEPAD_B.0;
    }
    if b5 & 0x80 != 0 {
        mask |= XINPUT_GAMEPAD_Y.0;
    }
    if b6 & 0x01 != 0 {
        mask |= XINPUT_GAMEPAD_LEFT_SHOULDER.0;
    }
    if b6 & 0x02 != 0 {
        mask |= XINPUT_GAMEPAD_RIGHT_SHOULDER.0;
    }
    mask & XUSB_BUTTON_MASK
}

fn ds4_dpad_hat(hat: u8) -> u16 {
    match hat {
        0 => XINPUT_GAMEPAD_DPAD_UP.0,
        1 => XINPUT_GAMEPAD_DPAD_UP.0 | XINPUT_GAMEPAD_DPAD_RIGHT.0,
        2 => XINPUT_GAMEPAD_DPAD_RIGHT.0,
        3 => XINPUT_GAMEPAD_DPAD_DOWN.0 | XINPUT_GAMEPAD_DPAD_RIGHT.0,
        4 => XINPUT_GAMEPAD_DPAD_DOWN.0,
        5 => XINPUT_GAMEPAD_DPAD_DOWN.0 | XINPUT_GAMEPAD_DPAD_LEFT.0,
        6 => XINPUT_GAMEPAD_DPAD_LEFT.0,
        7 => XINPUT_GAMEPAD_DPAD_UP.0 | XINPUT_GAMEPAD_DPAD_LEFT.0,
        _ => 0,
    }
}

fn stick_active_for_profile(profile: HidProfile, report: &[u8]) -> bool {
    match profile {
        HidProfile::Xusb | HidProfile::XusbButtonByte => xusb_stick_active(report),
        HidProfile::SonyUsb | HidProfile::SonyBluetooth => sony_stick_active(report),
        HidProfile::GenericHidP => false,
    }
}

fn xusb_stick_active(report: &[u8]) -> bool {
    for (lx, ly) in [(6usize, 8), (7, 9), (4, 6)] {
        if ly + 2 > report.len() {
            continue;
        }
        let lx_v = i16::from_le_bytes([report[lx], report[lx + 1]]);
        let ly_v = i16::from_le_bytes([report[ly], report[ly + 1]]);
        if lx_v.abs() > STICK_DEADZONE || ly_v.abs() > STICK_DEADZONE {
            return true;
        }
    }
    false
}

fn sony_stick_active(report: &[u8]) -> bool {
    if report.len() < 5 {
        return false;
    }
    let start = if report[0] == 0x01 && report.len() >= 10 {
        1
    } else {
        1
    };
    for i in start..start + 4 {
        if i >= report.len() {
            break;
        }
        if (report[i] as i16 - 128).abs() > 28 {
            return true;
        }
    }
    false
}

fn sticks_for_profile(profile: HidProfile, report: &[u8]) -> (f32, f32, f32, f32) {
    match profile {
        HidProfile::Xusb | HidProfile::XusbButtonByte => {
            if report.len() < 10 {
                return (0.0, 0.0, 0.0, 0.0);
            }
            let lx = i16::from_le_bytes([report[6], report[7]]);
            let ly = i16::from_le_bytes([report[8], report[9]]);
            let rx = i16::from_le_bytes([report[10], report[11]]);
            let ry = i16::from_le_bytes([report[12], report[13]]);
            (
                norm_thumb(lx, STICK_DEADZONE),
                norm_thumb(ly, STICK_DEADZONE),
                norm_thumb(rx, STICK_DEADZONE),
                norm_thumb(ry, STICK_DEADZONE),
            )
        }
        HidProfile::SonyUsb | HidProfile::SonyBluetooth => {
            if report.len() < 5 {
                return (0.0, 0.0, 0.0, 0.0);
            }
            let off = if matches!(profile, HidProfile::SonyBluetooth) && report.len() >= 12 {
                1
            } else {
                1
            };
            let lx = (report[off] as i16 - 128) as f32 / 128.0;
            let ly = (report[off + 1] as i16 - 128) as f32 / 128.0;
            let rx = (report[off + 2] as i16 - 128) as f32 / 128.0;
            let ry = (report[off + 3] as i16 - 128) as f32 / 128.0;
            (deadzone_f(lx), deadzone_f(-ly), deadzone_f(rx), deadzone_f(-ry))
        }
        HidProfile::GenericHidP => (0.0, 0.0, 0.0, 0.0),
    }
}

fn norm_thumb(value: i16, deadzone: i16) -> f32 {
    let v = value as f32;
    if v.abs() < deadzone as f32 {
        return 0.0;
    }
    (v / 32767.0).clamp(-1.0, 1.0)
}

fn deadzone_f(v: f32) -> f32 {
    if v.abs() < 0.15 {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

fn hidp_button_mask(preparsed: Option<PHIDP_PREPARSED_DATA>, report: &[u8]) -> u16 {
    let Some(preparsed) = preparsed else {
        return 0;
    };
    let mut mask = 0u16;
    mask |= hidp_page_buttons(preparsed, report, 0x09);
    mask |= hidp_usages_ex(preparsed, report);
    mask & XUSB_BUTTON_MASK
}

fn hidp_page_buttons(preparsed: PHIDP_PREPARSED_DATA, report: &[u8], page: u16) -> u16 {
    let mut list = [0u16; 32];
    let mut len = list.len() as u32;
    let mut report_buf = report.to_vec();
    let status = unsafe {
        HidP_GetUsages(
            HidP_Input,
            page,
            0,
            list.as_mut_ptr(),
            &mut len,
            preparsed,
            &mut report_buf,
        )
    };
    if status != HIDP_STATUS_SUCCESS {
        return 0;
    }
    let mut mask = 0u16;
    for usage in list.iter().take(len as usize) {
        if let Some(bit) = hid_usage_to_mask(page, *usage) {
            mask |= bit;
        }
    }
    mask
}

fn hidp_usages_ex(preparsed: PHIDP_PREPARSED_DATA, report: &[u8]) -> u16 {
    let mut usages = [USAGE_AND_PAGE::default(); 64];
    let mut usage_len = usages.len() as u32;
    let mut report_buf = report.to_vec();
    let status = unsafe {
        HidP_GetUsagesEx(
            HidP_Input,
            0,
            usages.as_mut_ptr(),
            &mut usage_len,
            preparsed,
            &mut report_buf,
        )
    };
    if status != HIDP_STATUS_SUCCESS {
        return 0;
    }
    let mut mask = 0u16;
    for usage in usages.iter().take(usage_len as usize) {
        if let Some(bit) = hid_usage_to_mask(usage.UsagePage, usage.Usage) {
            mask |= bit;
        }
    }
    mask
}

fn hid_usage_to_mask(page: u16, usage: u16) -> Option<u16> {
    Some(match (page, usage) {
        (0x09, 1) => XINPUT_GAMEPAD_A.0,
        (0x09, 2) => XINPUT_GAMEPAD_B.0,
        (0x09, 3) => XINPUT_GAMEPAD_X.0,
        (0x09, 4) => XINPUT_GAMEPAD_Y.0,
        (0x09, 5) => XINPUT_GAMEPAD_LEFT_SHOULDER.0,
        (0x09, 6) => XINPUT_GAMEPAD_RIGHT_SHOULDER.0,
        (0x09, 12) => XINPUT_GAMEPAD_DPAD_UP.0,
        (0x09, 13) => XINPUT_GAMEPAD_DPAD_DOWN.0,
        (0x09, 14) => XINPUT_GAMEPAD_DPAD_LEFT.0,
        (0x09, 15) => XINPUT_GAMEPAD_DPAD_RIGHT.0,
        _ => return None,
    })
}

fn device_vid_pid_name(hdevice: isize) -> Option<(u16, u16, String)> {
    let handle = HANDLE(hdevice as _);
    let mut bytes = 0u32;
    unsafe {
        GetRawInputDeviceInfoW(handle, RIDI_DEVICEINFO, None, &mut bytes);
    }
    if bytes < size_of::<RID_DEVICE_INFO>() as u32 {
        return None;
    }
    let mut info = RID_DEVICE_INFO::default();
    let got = unsafe {
        GetRawInputDeviceInfoW(
            handle,
            RIDI_DEVICEINFO,
            Some((&mut info as *mut RID_DEVICE_INFO).cast()),
            &mut bytes,
        )
    };
    if got == u32::MAX || got == 0 {
        return None;
    }
    if info.dwType != RIM_TYPEHID {
        return None;
    }
    let hid = unsafe { info.Anonymous.hid };
    let vid = hid.dwVendorId as u16;
    let pid = hid.dwProductId as u16;
    let mut name_bytes = 0u32;
    unsafe {
        GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, None, &mut name_bytes);
    }
    let name = if name_bytes > 2 {
        let mut buf = vec![0u16; name_bytes as usize / 2 + 1];
        let got = unsafe {
            GetRawInputDeviceInfoW(
                handle,
                RIDI_DEVICENAME,
                Some(buf.as_mut_ptr().cast()),
                &mut name_bytes,
            )
        };
        if got != u32::MAX && got > 0 {
            let len = (got as usize / 2).min(buf.len()).saturating_sub(1);
            String::from_utf16_lossy(&buf[..len])
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    Some((vid, pid, name))
}

fn load_gcdb_hints(path: &Path) -> Vec<GcdbHint> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((guid, rest)) = line.split_once(',') else {
            continue;
        };
        let Some((name, _mapping)) = rest.split_once(',') else {
            continue;
        };
        let Some((vid, pid)) = vid_pid_from_gcdb_guid(guid.trim()) else {
            continue;
        };
        let profile = profile_from_gcdb_name(name.trim());
        out.push(GcdbHint { vid, pid, profile });
    }
    out
}

fn vid_pid_from_gcdb_guid(guid: &str) -> Option<(u16, u16)> {
    let guid = guid.trim();
    if guid.len() < 24 {
        return None;
    }
    let bus = u32::from_str_radix(&guid[0..8], 16).ok()?;
    if bus & 0x0f != 0x03 {
        return None;
    }
    let vid = u32::from_str_radix(&guid[8..16], 16).ok()? as u16;
    let pid = u32::from_str_radix(&guid[16..24], 16).ok()? as u16;
    if vid == 0 && pid == 0 {
        return None;
    }
    Some((vid, pid))
}

fn profile_from_gcdb_name(name: &str) -> HidProfile {
    let n = name.to_ascii_lowercase();
    if n.contains("dualsense") || n.contains("ps5") {
        return HidProfile::SonyUsb;
    }
    if n.contains("dualshock 3") || n.contains("ps3") {
        return HidProfile::SonyUsb;
    }
    if n.contains("dualshock") || n.contains("ps4") || n.contains("playstation") {
        if n.contains("bluetooth") || n.contains("wireless") {
            return HidProfile::SonyBluetooth;
        }
        return HidProfile::SonyUsb;
    }
    if n.contains("xbox") || n.contains("xinput") || n.contains("360") || n.contains("one") {
        return HidProfile::Xusb;
    }
    HidProfile::GenericHidP
}

pub fn process_raw_input(
    devices: &mut HashMap<usize, DeviceState>,
    raw: &RAWINPUT,
) -> Option<(usize, PadSample, &'static str, String, Vec<u8>)> {
    if raw.header.dwType != windows::Win32::UI::Input::RIM_TYPEHID.0 {
        return None;
    }
    let key = raw.header.hDevice.0 as usize;
    let mut just_opened = None;
    if !devices.contains_key(&key) {
        let device = open_device(raw.header.hDevice.0 as isize)?;
        just_opened = Some(format!(
            "opened {} {:04x}:{:04x} {}",
            device.profile.label(),
            device.vid,
            device.pid,
            device.name
        ));
        devices.insert(key, device);
    }
    let device = devices.get_mut(&key)?;
    let hid = unsafe { raw.data.hid };
    let report_size = hid.dwSizeHid as usize;
    let report_count = hid.dwCount as usize;
    if report_size == 0 || report_count == 0 {
        return None;
    }
    let raw_start =
        unsafe { std::slice::from_raw_parts(hid.bRawData.as_ptr(), report_size * report_count) };
    let mut last = PadSample::default();
    let mut last_src = "";
    let mut last_report = Vec::new();
    for (i, report) in raw_start.chunks(report_size).enumerate() {
        if just_opened.is_some() && i == 0 {
            device.calibrate_xusb_from_report(report);
        }
        let (sample, src) = decode_report_logged(device, report);
        last = sample;
        last_src = src;
        last_report.clear();
        last_report.extend_from_slice(report);
    }
    let dev = format!(
        "{} {:04x}:{:04x} {}",
        device.profile.label(),
        device.vid,
        device.pid,
        device.name
    );
    if let Some(open) = just_opened {
        let off_note = if device.profile == HidProfile::Xusb && device.xusb_btn_offset != u8::MAX {
            format!(" btn@{}", device.xusb_btn_offset)
        } else {
            String::new()
        };
        return Some((key, last, "open", format!("{open} | {dev}{off_note}"), last_report));
    }
    Some((key, last, last_src, dev, last_report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xusb_buttons_follow_report_id() {
        // Report id @0, wButtons @1–2, LT/RT @3–4 (misread as X @2–3 when LT=0x40).
        let report = [0x00, 0x00, 0x00, 0x40, 0x00, 0, 0, 0, 0, 0];
        assert_eq!(calibrate_xusb_button_offset(&report), 1);
        assert_eq!(parse_xusb_mask_at(&report, 1), None);
        assert_eq!(parse_xusb_mask_at(&report, 2), Some(0x4000));
    }

    #[test]
    fn rejects_all_dpad_mask() {
        assert!(plausible_mask(0x000f).is_none());
        assert!(plausible_mask(0x005f).is_none());
    }

}
