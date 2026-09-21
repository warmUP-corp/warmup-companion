//! Raw HID registration and WM_INPUT polling on the secure poll thread.

use std::mem::size_of;
use std::sync::mpsc;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_RIGHT_SHOULDER,
    XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y,
};
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices, HRAWINPUT,
    RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_PAGEONLY,
    RIDI_DEVICEINFO, RID_DEVICE_INFO, RID_INPUT, RIM_TYPEHID,
};

use crate::hid_gamepad;

use super::service_log;
use super::types::{PollState, SecureMsg};

fn secure_hid_combo_mask(mask: u16) -> bool {
    const NON_DPAD: u16 = XINPUT_GAMEPAD_A.0
        | XINPUT_GAMEPAD_B.0
        | XINPUT_GAMEPAD_X.0
        | XINPUT_GAMEPAD_Y.0
        | XINPUT_GAMEPAD_LEFT_SHOULDER.0
        | XINPUT_GAMEPAD_RIGHT_SHOULDER.0;
    (mask & NON_DPAD).count_ones() > 1
}
pub(crate) fn register_raw_gamepad(hwnd: HWND, tx: &mpsc::Sender<SecureMsg>) {
    let devices = [
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x00,
            dwFlags: RIDEV_INPUTSINK | RIDEV_PAGEONLY,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x05,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x04,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x08,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
    ];
    match unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) } {
        Ok(()) => {
            let _ = tx.send(SecureMsg::Error(
                "HID: raw input sink registered (generic-desktop page + gamepad + joystick + multi-axis)".into(),
            ));
            log_raw_input_devices(tx);
        }
        Err(e) => {
            let _ = tx.send(SecureMsg::Error(format!("raw HID register failed: {e}")));
        }
    }
}

#[allow(dead_code)]
fn log_raw_input_devices(tx: &mpsc::Sender<SecureMsg>) {
    let mut count = 0u32;
    let item_size = size_of::<RAWINPUTDEVICELIST>() as u32;
    let first = unsafe { GetRawInputDeviceList(None, &mut count, item_size) };
    if first == u32::MAX || count == 0 {
        let _ = tx.send(SecureMsg::Error("HID: raw device list empty".into()));
        return;
    }

    let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
    let got = unsafe { GetRawInputDeviceList(Some(devices.as_mut_ptr()), &mut count, item_size) };
    if got == u32::MAX {
        let _ = tx.send(SecureMsg::Error("HID: raw device list failed".into()));
        return;
    }

    let mut logged = 0usize;
    for item in devices.into_iter().take(count as usize) {
        if item.dwType != RIM_TYPEHID {
            continue;
        }

        let mut info = RID_DEVICE_INFO {
            cbSize: size_of::<RID_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        let mut size = size_of::<RID_DEVICE_INFO>() as u32;
        let ok = unsafe {
            GetRawInputDeviceInfoW(
                item.hDevice,
                RIDI_DEVICEINFO,
                Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                &mut size,
            )
        };
        if ok == u32::MAX {
            continue;
        }

        let hid = unsafe { info.Anonymous.hid };
        let is_sony = hid.dwVendorId == 0x054c;
        if logged < 12 || is_sony {
            logged += 1;
            let _ = tx.send(SecureMsg::Error(format!(
                "HID: raw dev vid={:04x} pid={:04x} page=0x{:04x} usage=0x{:04x}",
                hid.dwVendorId, hid.dwProductId, hid.usUsagePage, hid.usUsage,
            )));
        }
    }
}

#[allow(dead_code)]
pub(crate) fn poll_raw_hid_input(state: &mut PollState, raw_handle: HRAWINPUT) {
    let mut size = 0u32;
    unsafe {
        GetRawInputData(
            raw_handle,
            RID_INPUT,
            None,
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
    }
    if size == 0 {
        return;
    }

    let words = (size as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    let capacity = words * size_of::<usize>();
    let read = unsafe {
        GetRawInputData(
            raw_handle,
            RID_INPUT,
            Some(storage.as_mut_ptr().cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if read == u32::MAX || read == 0 {
        return;
    }
    // `read` is the byte count GetRawInputData wrote; never trust it past the
    // allocation. process_raw_input clamps the HID payload to these bytes.
    let raw_bytes = (read as usize).min(capacity);
    if raw_bytes < size_of::<RAWINPUTHEADER>() {
        return;
    }

    let raw = unsafe { &*(storage.as_ptr() as *const RAWINPUT) };
    let Some((_key, sample, src, dev, report)) =
        hid_gamepad::process_raw_input(&mut state.hid_devices, raw, raw_bytes)
    else {
        return;
    };
    let raw_hex = report_hex(&report);
    state.last_raw_report = report.clone();
    if src == "open" {
        service_log(&format!("HID secure: {dev} raw={raw_hex}"));
        state.prev_buttons[0] = 0;
        state.hid_diag_count = 0;
        state.suppress_until_zero = false;
    }
    state.last_hid = sample;
    let cur = sample.buttons;
    let prev = state.prev_buttons[0];
    if prev != cur {
        if state.suppress_until_zero {
            if cur == 0 {
                state.suppress_until_zero = false;
                state.prev_buttons[0] = 0;
            }
            service_log(&format!(
                "HID secure: suppress combo tail 0x{prev:04x} -> 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
            ));
            return;
        }
        if secure_hid_combo_mask(prev) || secure_hid_combo_mask(cur) {
            state.suppress_until_zero = cur != 0;
            service_log(&format!(
                "HID secure: suppress noisy combo 0x{prev:04x} -> 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
            ));
            return;
        }
        state.prev_buttons[0] = cur;
        service_log(&format!(
            "HID secure: buttons 0x{prev:04x} -> 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
        ));
        let _ = state.tx.send(SecureMsg::Buttons { slot: 0, prev, cur });
    } else if cur != 0 && state.hid_diag_count < 4 {
        state.hid_diag_count = state.hid_diag_count.saturating_add(1);
        service_log(&format!(
            "HID secure: held 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
        ));
    }
}

pub(crate) fn report_hex(report: &[u8]) -> String {
    let mut out = String::new();
    for (i, b) in report.iter().take(16).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02x}"));
    }
    if report.len() > 16 {
        out.push_str(" ...");
    }
    out
}
