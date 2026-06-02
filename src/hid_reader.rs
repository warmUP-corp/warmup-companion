//! Direct HID input-report reader — the secure-desktop bypass for vendor pads.
//!
//! On the Winlogon desktop our session-0 service window never holds foreground
//! (LogonUI owns it), and `WM_INPUT` is empirically never delivered there even
//! with `RIDEV_INPUTSINK`: the service log shows a DualSense enumerated (vid
//! 054c, gamepad usage 0x05) yet zero raw reports ever arrive. The XUSB path
//! (`xusb_ioctl`) already sidesteps this for Xbox pads by talking to xusb22.sys
//! directly; this module does the equivalent for PlayStation / generic HID pads
//! by opening the device interface and reading input reports with overlapped
//! I/O, polled from the same secure-poll timer tick. Decoding reuses the
//! `hid_gamepad` profile machinery (identical to the dead raw-input path), so a
//! DualShock/DualSense report parses the same way regardless of how the bytes
//! arrived.

use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::UI::Input::{
    GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RIDI_DEVICEINFO,
    RID_DEVICE_INFO, RIM_TYPEHID,
};

use crate::hid_gamepad::{self, DeviceState, PadSample};

const GENERIC_READ: u32 = 0x8000_0000;
/// Input reports top out at 78 bytes (DualSense Bluetooth 0x31); 128 is generous.
const HID_READ_BUF: usize = 128;

/// One opened HID gamepad we read input reports from directly, plus the decode
/// state (profile / preparsed data) shared with the raw-input parser.
pub struct HidReader {
    handle: HANDLE,
    event: HANDLE,
    /// Boxed so the pointer handed to `ReadFile` stays valid while the read is
    /// pending, even if the owning `Vec<HidReader>` reallocates.
    overlapped: Box<OVERLAPPED>,
    buf: Vec<u8>,
    /// A read is in flight (issued, not yet consumed).
    pending: bool,
    /// The handle errored (device unplugged); the caller drops us and rescans.
    dead: bool,
    device: DeviceState,
    last_report: Vec<u8>,
}

impl HidReader {
    /// Enumerate present HID gamepads/joysticks that are NOT Xbox/XUSB pads
    /// (those go through `xusb_ioctl`) and open a direct read handle to each.
    /// Returns the readers plus diagnostic lines for the secure-poll log.
    pub fn open_all() -> (Vec<HidReader>, Vec<String>) {
        let mut log = Vec::new();
        let mut readers = Vec::new();

        let item_size = size_of::<RAWINPUTDEVICELIST>() as u32;
        let mut count = 0u32;
        let first = unsafe { GetRawInputDeviceList(None, &mut count, item_size) };
        if first == u32::MAX || count == 0 {
            return (readers, log);
        }

        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let got = unsafe { GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, item_size) };
        if got == u32::MAX {
            log.push("HID read: device list failed".into());
            return (readers, log);
        }

        for item in list.into_iter().take(count as usize) {
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
            // Generic-desktop joystick (0x04) / gamepad (0x05) only.
            let is_pad = hid.usUsagePage == 0x01 && (hid.usUsage == 0x04 || hid.usUsage == 0x05);
            if !is_pad {
                continue;
            }
            // Xbox pads are read via the XUSB IOCTL bypass; don't double-drive them.
            if hid.dwVendorId as u16 == 0x045e {
                continue;
            }
            let Some(device) = hid_gamepad::open_device(item.hDevice.0 as isize) else {
                continue;
            };
            match HidReader::open(device) {
                Ok(reader) => {
                    log.push(format!("HID read: opened {}", reader.label()));
                    readers.push(reader);
                }
                Err(e) => log.push(format!(
                    "HID read: open failed vid={:04x} pid={:04x}: {e}",
                    hid.dwVendorId, hid.dwProductId
                )),
            }
        }

        log.push(format!("HID read: {} direct reader(s)", readers.len()));
        (readers, log)
    }

    fn open(device: DeviceState) -> Result<HidReader, String> {
        if device.name.is_empty() {
            return Err("empty device path".into());
        }
        let path: Vec<u16> = device
            .name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| format!("CreateFileW: {e}"))?;
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|e| format!("CreateEventW: {e}"))?;
        Ok(HidReader {
            handle,
            event,
            overlapped: Box::new(OVERLAPPED::default()),
            buf: vec![0u8; HID_READ_BUF],
            pending: false,
            dead: false,
            device,
            last_report: Vec::new(),
        })
    }

    /// Non-blocking poll: arm an overlapped read if none is in flight, then
    /// consume it if it has completed. Returns the decoded sample on a fresh
    /// report, `None` while a read is still pending or on a transient empty read.
    pub fn poll(&mut self) -> Option<PadSample> {
        if self.dead {
            return None;
        }
        if !self.pending && !self.arm() {
            return None;
        }
        let waited = unsafe { WaitForSingleObject(self.event, 0) };
        if waited != WAIT_OBJECT_0 {
            return None;
        }
        let mut transferred = 0u32;
        let ok = unsafe {
            GetOverlappedResult(self.handle, &*self.overlapped, &mut transferred, false)
        };
        self.pending = false;
        if ok.is_err() {
            self.dead = true;
            return None;
        }
        if transferred == 0 {
            return None;
        }
        let n = (transferred as usize).min(self.buf.len());
        self.last_report.clear();
        self.last_report.extend_from_slice(&self.buf[..n]);
        let (sample, _src) = hid_gamepad::decode_report_logged(&mut self.device, &self.last_report);
        Some(sample)
    }

    /// Issue an overlapped read into `buf`. Returns false (and marks us dead) on
    /// a hard error; ERROR_IO_PENDING is the normal "queued" path.
    fn arm(&mut self) -> bool {
        unsafe {
            let _ = ResetEvent(self.event);
        }
        self.overlapped.hEvent = self.event;
        let result = unsafe {
            ReadFile(self.handle, Some(&mut self.buf), None, Some(&mut *self.overlapped))
        };
        match result {
            Ok(()) => {
                self.pending = true;
                true
            }
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                self.pending = true;
                true
            }
            Err(_) => {
                self.dead = true;
                false
            }
        }
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    pub fn last_report(&self) -> &[u8] {
        &self.last_report
    }

    pub fn label(&self) -> String {
        format!(
            "{} {:04x}:{:04x} {}",
            self.device.profile.label(),
            self.device.vid,
            self.device.pid,
            self.device.name
        )
    }
}

impl Drop for HidReader {
    fn drop(&mut self) {
        unsafe {
            // Closing a handle with a pending overlapped read cancels it.
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.event);
        }
    }
}
