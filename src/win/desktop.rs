//! `warmup_attach_named_desktop` / `warmup_attach_input_desktop` (Win32).
//!
//! Desktop handles stay open while attached — closing before `SetThreadDesktop` causes
//! `ERROR_BUSY` (0x800700AA). State is **per-thread** (VK UI thread attaches separately).

use std::cell::RefCell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, GetThreadDesktop, GetUserObjectInformationW, OpenDesktopW,
    OpenInputDesktop, OpenWindowStationW, SetProcessWindowStation, SetThreadDesktop,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, HDESK, HWINSTA, UOI_NAME,
    USER_OBJECT_INFORMATION_INDEX,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, HMENU, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

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
    SetProcessWindowStation(guard.station).map_err(|e| format!("SetProcessWindowStation: {e}"))?;
    SetThreadDesktop(guard.desktop).map_err(|e| format!("SetThreadDesktop: {e}"))?;
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

pub fn input_desktop_name() -> Result<String, String> {
    unsafe {
        let station = open_station()?;
        let h_desktop = OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(DESKTOP_ALL),
        )
        .map_err(|e| {
            let _ = CloseWindowStation(station);
            format!("OpenInputDesktop: {e}")
        })?;
        let name = desktop_name(h_desktop).unwrap_or_else(|| "?".into());
        let _ = CloseDesktop(h_desktop);
        let _ = CloseWindowStation(station);
        Ok(name)
    }
}

pub fn current_desktop_name() -> Option<String> {
    unsafe {
        let h = GetThreadDesktop(GetCurrentThreadId()).ok()?;
        desktop_name(h)
    }
}

/// Create the worker-thread anchor window on whatever desktop the calling
/// thread is currently attached to. Joyxoff `+0xd9` creates `JoyXoffMWindow`
/// on the Winlogon desktop right after `SetThreadDesktop("winlogon")`; without
/// it, XInput / HID input directed at the input desktop never reaches the
/// process. The returned HWND must outlive the polling loop — leak it for the
/// process lifetime (worker exits when SCM stops the service).
pub unsafe fn create_main_anchor() -> Result<HWND, String> {
    unsafe extern "system" fn anchor_wndproc(
        hwnd: HWND,
        msg: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, w, l)
    }

    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let class = w!("WarmupWorkerAnchor");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(anchor_wndproc),
        hInstance: instance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    // RegisterClassW returns 0 if class already exists; tolerate the second worker.
    RegisterClassW(&wc);
    // Joyxoff `JoyXoffMWindow` ex_style 0x8080088 = TOPMOST|TOOLWINDOW|NOACTIVATE|LAYERED.
    CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        class,
        w!("Warmup Worker Anchor"),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        HMENU::default(),
        HINSTANCE(instance.0),
        None,
    )
    .map_err(|e| format!("CreateWindowExW(anchor): {e}"))
}

unsafe fn desktop_name(h: HDESK) -> Option<String> {
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
