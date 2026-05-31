//! Best-effort suppression for Windows' built-in touch keyboard/input panel.
//!
//! The sign-in PIN field can ask Windows to show its own keyboard when focus is
//! retargeted. Warmup owns the visible VK, so hide any native panel windows that
//! appear on the current desktop.

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible, PostMessageW, ShowWindow, SW_HIDE,
    WM_CLOSE,
};

pub fn suppress() {
    unsafe {
        let _ = EnumWindows(Some(enum_window), LPARAM(0));
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, _param: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }

    let class = window_class(hwnd);
    let title = window_title(hwnd);
    if is_native_keyboard_window(&class, &title) {
        crate::install::log_line(&format!(
            "native keyboard suppress: class='{class}' title='{title}'"
        ));
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    true.into()
}

fn window_class(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

fn is_native_keyboard_window(class: &str, title: &str) -> bool {
    class == "IPTip_Main_Window"
        || class == "IPTip_Window"
        || (class == "Windows.UI.Core.CoreWindow"
            && (title == "Microsoft Text Input Application"
                || title == "Windows Input Experience"
                || title.contains("Text Input")))
}
