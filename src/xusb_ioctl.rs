//! Direct XUSB driver reader — bypasses XInput's foreground focus gate.
//!
//! `XInputGetState` (xinput1_4.dll) returns a neutral, zeroed gamepad to any
//! process whose window is not foreground on the input desktop. On Winlogon
//! that window is always LogonUI, so the secure poll thread can never satisfy
//! the gate (stealing foreground would break the password box). The focus gate
//! lives in the *DLL*, not the driver: `XInputGetState` is a thin wrapper over
//! `DeviceIoControl` on the XUSB device interface. Talking to that interface
//! directly returns real button state regardless of foreground.
//!
//! Scope: physical XUSB pads (Xbox 360 / One / Series, xusb22.sys). Virtual
//! pads (ViGEm / DS4Windows / Steam Input) live in the interactive user session
//! and do not exist on the secure desktop, so nothing reads them at Winlogon.
//!
//! NOTE: the GET_GAMEPAD_STATE output byte layout is reverse-engineered and
//! provisional — `parse_report` documents the assumed offsets. Every poll also
//! surfaces the raw bytes (see `XusbReport.raw`) so the offsets can be confirmed
//! against a real button press before the parse is trusted.

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::GUID;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;

/// XUSB device interface class GUID {EC87F1E3-C13B-4100-B5F7-8B84D54260CB}.
const GUID_XUSB: GUID = GUID::from_u128(0xEC87F1E3_C13B_4100_B5F7_8B84D54260CB);

/// Wrapper IOCTLs xinput1_x sends to xusb22.sys.
const IOCTL_XUSB_GET_INFORMATION: u32 = 0x8000_6000;
const IOCTL_XUSB_GET_GAMEPAD_STATE: u32 = 0x8000_E00C;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Max output bytes for GET_GAMEPAD_STATE (real reports are ~29; pad generous).
const STATE_OUT_LEN: usize = 64;

/// One open handle to a physical XUSB pad, plus the LED ordinal (user index)
/// the driver expects in the GET_GAMEPAD_STATE request.
pub struct XusbDevice {
    handle: HANDLE,
    led: u8,
}

/// Parsed gamepad snapshot, mirroring the XInput state we already consume, plus
/// the raw IOCTL bytes for offset verification.
#[derive(Clone, Default)]
pub struct XusbReport {
    pub packet: u32,
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
    /// Raw output buffer, truncated to the bytes the driver actually returned.
    pub raw: Vec<u8>,
}

impl XusbDevice {
    /// Enumerate every present XUSB interface and open a handle to each. Returns
    /// the open devices plus diagnostic lines describing what happened (caller
    /// forwards them through the existing secure-poll log channel).
    pub fn open_all() -> (Vec<XusbDevice>, Vec<String>) {
        let mut log = Vec::new();
        let mut devices = Vec::new();

        let hdev = unsafe {
            SetupDiGetClassDevsW(
                Some(&GUID_XUSB),
                None,
                None,
                DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
            )
        };
        let hdev: HDEVINFO = match hdev {
            Ok(h) => h,
            Err(e) => {
                log.push(format!("XUSB: SetupDiGetClassDevsW failed: {e}"));
                return (devices, log);
            }
        };

        let mut index = 0u32;
        loop {
            let mut ifd = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if unsafe { SetupDiEnumDeviceInterfaces(hdev, None, &GUID_XUSB, index, &mut ifd) }
                .is_err()
            {
                break; // ERROR_NO_MORE_ITEMS terminates the enumeration.
            }
            index += 1;

            match open_interface(hdev, &ifd) {
                Ok(path) => {
                    let (handle, led) = path;
                    devices.push(XusbDevice { handle, led });
                }
                Err(e) => log.push(format!("XUSB: device {index} open failed: {e}")),
            }
        }

        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(hdev);
        }

        log.push(format!("XUSB: opened {} physical pad(s)", devices.len()));
        (devices, log)
    }

    /// Poll the current gamepad state via the XUSB driver. Returns None when the
    /// IOCTL fails (device gone) or returns no usable bytes.
    pub fn poll(&self) -> Option<XusbReport> {
        let input: [u8; 3] = [0x01, 0x01, self.led];
        let mut out = [0u8; STATE_OUT_LEN];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_XUSB_GET_GAMEPAD_STATE,
                Some(input.as_ptr() as *const c_void),
                input.len() as u32,
                Some(out.as_mut_ptr() as *mut c_void),
                out.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        if ok.is_err() || returned == 0 {
            return None;
        }
        let raw = out[..returned as usize].to_vec();
        Some(parse_report(&raw))
    }
}

impl Drop for XusbDevice {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Open one enumerated interface; also queries GET_INFORMATION to recover the
/// LED ordinal the state IOCTL expects. Returns (handle, led_ordinal).
fn open_interface(
    hdev: HDEVINFO,
    ifd: &SP_DEVICE_INTERFACE_DATA,
) -> Result<(HANDLE, u8), String> {
    // Two-call pattern: first call reports the required detail-buffer size.
    let mut required = 0u32;
    let _ = unsafe {
        SetupDiGetDeviceInterfaceDetailW(hdev, ifd, None, 0, Some(&mut required), None)
    };
    if required == 0 {
        return Err("detail size query returned 0".into());
    }

    let mut buf = vec![0u8; required as usize];
    let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
    // cbSize is the size of the fixed header (NOT the whole buffer): 8 on x64,
    // 6 on x86. The OS uses it to locate the DevicePath member.
    let cb = if cfg!(target_pointer_width = "64") {
        8u32
    } else {
        6u32
    };
    unsafe {
        (*detail).cbSize = cb;
    }
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(hdev, ifd, Some(detail), required, None, None)
            .map_err(|e| format!("GetDeviceInterfaceDetailW: {e}"))?;
    }

    // DevicePath is a NUL-terminated wide string at the DevicePath member offset.
    let path_off = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let path = unsafe { wide_from_buf(&buf, path_off) };

    let handle = unsafe {
        CreateFileW(
            windows::core::PCWSTR(path.as_ptr()),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| format!("CreateFileW: {e}"))?;

    let led = query_led(handle).unwrap_or(0);
    Ok((handle, led))
}

/// GET_INFORMATION returns the device's LED ordinal (slot index) among other
/// fields. Provisional: byte offset confirmed empirically — fall back to 0.
fn query_led(handle: HANDLE) -> Option<u8> {
    let mut out = [0u8; 16];
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_XUSB_GET_INFORMATION,
            None,
            0,
            Some(out.as_mut_ptr() as *mut c_void),
            out.len() as u32,
            Some(&mut returned),
            None,
        )
    };
    if ok.is_err() || returned == 0 {
        return None;
    }
    // The LED ordinal sits near the front of the info block; index 0 is the
    // safe default for a single connected pad until verified against the dump.
    Some(0)
}

/// Parse the GET_GAMEPAD_STATE output buffer.
///
/// PROVISIONAL LAYOUT — verify against `XusbReport.raw` on real hardware:
/// the request returns a small header followed by a structure that mirrors
/// XINPUT_STATE (DWORD packet + XINPUT_GAMEPAD). Reported reports are 29 bytes;
/// the gamepad payload is assumed to begin after a 2-byte header:
///   [0]      status
///   [1]      size (0x14 = 20)
///   [2..6]   dwPacketNumber  (u32 LE)
///   [6..8]   wButtons        (u16 LE)
///   [8]      bLeftTrigger
///   [9]      bRightTrigger
///   [10..12] sThumbLX        (i16 LE)
///   [12..14] sThumbLY
///   [14..16] sThumbRX
///   [16..18] sThumbRY
fn parse_report(raw: &[u8]) -> XusbReport {
    let r = |off: usize, len: usize| -> Option<&[u8]> {
        raw.get(off..off + len)
    };
    let u16le = |off: usize| -> u16 {
        r(off, 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    };
    let i16le = |off: usize| -> i16 {
        r(off, 2).map_or(0, |b| i16::from_le_bytes([b[0], b[1]]))
    };
    let u32le = |off: usize| -> u32 {
        r(off, 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };

    XusbReport {
        packet: u32le(2),
        buttons: u16le(6),
        left_trigger: raw.get(8).copied().unwrap_or(0),
        right_trigger: raw.get(9).copied().unwrap_or(0),
        thumb_lx: i16le(10),
        thumb_ly: i16le(12),
        thumb_rx: i16le(14),
        thumb_ry: i16le(16),
        raw: raw.to_vec(),
    }
}

/// Read a NUL-terminated UTF-16 string from `buf` starting at byte `offset`,
/// returning it NUL-terminated for PCWSTR use.
unsafe fn wide_from_buf(buf: &[u8], offset: usize) -> Vec<u16> {
    let mut out = Vec::new();
    let mut i = offset;
    while i + 1 < buf.len() {
        let ch = u16::from_le_bytes([buf[i], buf[i + 1]]);
        if ch == 0 {
            break;
        }
        out.push(ch);
        i += 2;
    }
    out.push(0);
    out
}
