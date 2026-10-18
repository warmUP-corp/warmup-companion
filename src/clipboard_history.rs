use std::sync::{Mutex, Once};
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

pub const MAX_ITEMS: usize = 25;
const MAX_CHARS: usize = 100_000;
const CF_UNICODETEXT: u32 = 13;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;

static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn remember(list: &mut Vec<String>, text: &str) -> bool {
    if text.trim().is_empty() || text.chars().count() > MAX_CHARS {
        return false;
    }
    list.retain(|t| t != text);
    list.insert(0, text.to_string());
    list.truncate(MAX_ITEMS);
    true
}

pub fn is_private(excluded: bool, can_include: Option<u32>, can_upload: Option<u32>) -> bool {
    excluded || can_include == Some(0) || can_upload == Some(0)
}

pub fn items() -> Vec<String> {
    HISTORY.lock().map(|h| h.clone()).unwrap_or_default()
}

#[cfg(test)]
pub fn push_for_test(text: &str) {
    if let Ok(mut h) = HISTORY.lock() {
        remember(&mut h, text);
    }
}

pub fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("warmup-clipboard".into())
            .spawn(|| unsafe { run() });
    });
}

unsafe fn run() {
    let Ok(module) = GetModuleHandleW(None) else {
        return;
    };
    let class = w!("WarmupClipboardListener");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: module.into(),
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let Ok(hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR::null(),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        None,
        module,
        None,
    ) else {
        crate::install::log_line("clipboard history: listener window failed");
        return;
    };
    if let Err(e) = AddClipboardFormatListener(hwnd) {
        crate::install::log_line(&format!("clipboard history: listener failed: {e}"));
        return;
    }
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        if let Some(text) = capture(hwnd) {
            if let Ok(mut h) = HISTORY.lock() {
                remember(&mut h, &text);
            }
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn open(owner: Option<HWND>) -> bool {
    for _ in 0..10 {
        if OpenClipboard(owner.unwrap_or_default()).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

unsafe fn capture(hwnd: HWND) -> Option<String> {
    let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
    let include = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
    let upload = RegisterClipboardFormatW(w!("CanUploadToCloudClipboard"));
    let excluded = IsClipboardFormatAvailable(exclude).is_ok();
    if excluded || IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
        return None;
    }
    if !open(Some(hwnd)) {
        return None;
    }
    let private = is_private(false, read_dword(include), read_dword(upload));
    let text = if private { None } else { read_text() };
    let _ = CloseClipboard();
    text
}

unsafe fn read_dword(format: u32) -> Option<u32> {
    if IsClipboardFormatAvailable(format).is_err() {
        return None;
    }
    let handle = HGLOBAL(GetClipboardData(format).ok()?.0);
    if GlobalSize(handle) < 4 {
        return None;
    }
    let ptr = GlobalLock(handle) as *const u32;
    if ptr.is_null() {
        return None;
    }
    let value = ptr.read_unaligned();
    let _ = GlobalUnlock(handle);
    Some(value)
}

unsafe fn read_text() -> Option<String> {
    let handle = HGLOBAL(GetClipboardData(CF_UNICODETEXT).ok()?.0);
    let units = GlobalSize(handle) / 2;
    let ptr = GlobalLock(handle) as *const u16;
    if ptr.is_null() {
        return None;
    }
    let slice = std::slice::from_raw_parts(ptr, units);
    let len = slice.iter().position(|&u| u == 0).unwrap_or(units);
    let text = String::from_utf16_lossy(&slice[..len]);
    let _ = GlobalUnlock(handle);
    Some(text)
}

pub fn set_text(text: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) else {
            return false;
        };
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
        let _ = GlobalUnlock(mem);
        if !open(None) {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_UNICODETEXT, HANDLE(mem.0)).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates() {
        let mut h = Vec::new();
        assert!(remember(&mut h, "a"));
        assert!(remember(&mut h, "b"));
        assert!(remember(&mut h, "a"));
        assert_eq!(h, ["a", "b"]);
    }

    #[test]
    fn keeps_only_the_last_25() {
        let mut h = Vec::new();
        for i in 0..40 {
            remember(&mut h, &i.to_string());
        }
        assert_eq!(h.len(), MAX_ITEMS);
        assert_eq!(h[0], "39");
        assert_eq!(h[MAX_ITEMS - 1], "15");
    }

    #[test]
    fn skips_blank_and_huge_text() {
        let mut h = Vec::new();
        assert!(!remember(&mut h, "  \r\n"));
        assert!(!remember(&mut h, &"x".repeat(MAX_CHARS + 1)));
        assert!(h.is_empty());
    }

    #[test]
    fn private_markers_are_honoured() {
        assert!(!is_private(false, None, None));
        assert!(is_private(true, None, None));
        assert!(is_private(false, Some(0), None));
        assert!(is_private(false, None, Some(0)));
        assert!(!is_private(false, Some(1), Some(1)));
    }
}
