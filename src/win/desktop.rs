//! `warmup_attach_named_desktop` / `warmup_attach_input_desktop` (Win32).
//!
//! Desktop handles stay open while attached — closing before `SetThreadDesktop` causes
//! `ERROR_BUSY` (0x800700AA). State is **per-thread** (VK UI thread attaches separately).

use std::cell::RefCell;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, GetThreadDesktop, GetUserObjectInformationW,
    OpenDesktopW, OpenInputDesktop, OpenWindowStationW, SetProcessWindowStation, SetThreadDesktop,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, HDESK, HWINSTA, UOI_NAME,
    USER_OBJECT_INFORMATION_INDEX,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

/// Same access mask as decompiled Joyxoff (`0x2000000`).
const DESKTOP_ALL: u32 = 0x200_0000;

struct DesktopGuard {
    station: HWINSTA,
    desktop: HDESK,
}

thread_local! {
    static ACTIVE: RefCell<Option<DesktopGuard>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn open_station() -> Result<HWINSTA, String> {
    let station = wide("WinSta0");
    OpenWindowStationW(PCWSTR(station.as_ptr()), false, DESKTOP_ALL)
        .map_err(|e| format!("OpenWindowStationW: {e}"))
}

unsafe fn apply_guard(guard: DesktopGuard) -> Result<(), String> {
    SetProcessWindowStation(guard.station)
        .map_err(|e| format!("SetProcessWindowStation: {e}"))?;
    SetThreadDesktop(guard.desktop)
        .map_err(|e| format!("SetThreadDesktop: {e}"))?;
    ACTIVE.with(|slot| {
        if let Some(old) = slot.borrow_mut().take() {
            let _ = CloseDesktop(old.desktop);
            let _ = CloseWindowStation(old.station);
        }
        *slot.borrow_mut() = Some(guard);
    });
    Ok(())
}

pub fn attach_named(name: &str) -> Result<(), String> {
    let desktop_name = match name {
        "default" | "winlogon" => name,
        other => return Err(format!("unknown desktop: {other}")),
    };
    unsafe {
        let station = open_station()?;
        let desktop = wide(desktop_name);
        let h_desktop = OpenDesktopW(
            PCWSTR(desktop.as_ptr()),
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ALL,
        )
        .map_err(|e| format!("OpenDesktopW({desktop_name}): {e}"))?;
        apply_guard(DesktopGuard {
            station,
            desktop: h_desktop,
        })
    }
}

pub fn attach_input() -> Result<(), String> {
    unsafe {
        let station = open_station()?;
        let h_desktop = OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(DESKTOP_ALL),
        )
        .map_err(|e| format!("OpenInputDesktop: {e}"))?;
        apply_guard(DesktopGuard {
            station,
            desktop: h_desktop,
        })
    }
}

/// Follow the interactive input desktop (sign-in, UAC, or user default).
/// Required for Session-0 service threads before SDL / SendInput / enigo.
pub fn sync_input_desktop() {
    let _ = attach_input();
}

pub fn current_desktop_name() -> Option<String> {
    unsafe {
        let h = GetThreadDesktop(GetCurrentThreadId()).ok()?;
        let mut buf = [0u16; 256];
        let mut needed = 0u32;
        GetUserObjectInformationW(
            HANDLE(h.0),
            USER_OBJECT_INFORMATION_INDEX(UOI_NAME.0 as i32),
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
        .ok()?;
        let len = (needed as usize / 2).min(buf.len()).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}
