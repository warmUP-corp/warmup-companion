//! Best-effort suppression for Windows' built-in touch keyboard/input panel.
//!
//! The sign-in PIN field can ask Windows to show its own keyboard when focus is
//! retargeted. Warmup owns the visible VK, so hide any native panel windows that
//! appear on the current desktop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_USERS, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, ShowWindow, SW_HIDE, WM_CLOSE,
};

static SUPPRESSING: AtomicBool = AtomicBool::new(false);

/// Touch-keyboard auto-invoke registry control. The hide-after-show loop below
/// catches the panel (`TextInputHost` / `TabTip`) but loses the race — it
/// re-shows faster than the 25 ms sweep, so it flashes. Disabling auto-invoke
/// stops it being summoned on field focus in the first place.
///
/// LogonUI runs as SYSTEM, whose `HKCU` is `HKEY_USERS\.DEFAULT`, so the sign-in
/// touch keyboard reads its setting from there. `0` = no auto-invoke.
const TIP_SUBKEY: &str = ".DEFAULT\\Software\\Microsoft\\TabletTip\\1.7";
const AUTO_INVOKE_VALUE: &str = "EnableDesktopModeAutoInvoke";

/// `Some(prior)` while we have the value overridden; `prior` is the value we
/// must restore (`None` = the value was absent and should be deleted).
static AUTO_INVOKE_SAVED: Mutex<Option<Option<u32>>> = Mutex::new(None);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Disable the native touch keyboard's auto-invoke on the secure-desktop logon
/// profile, saving the prior value for [`restore_auto_invoke`]. Idempotent:
/// once overridden, repeated calls are no-ops (the prior value stays captured).
pub fn disable_auto_invoke() {
    let Ok(mut saved) = AUTO_INVOKE_SAVED.lock() else {
        return;
    };
    if saved.is_some() {
        return;
    }
    unsafe {
        let subkey = wide(TIP_SUBKEY);
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_USERS,
            PCWSTR(subkey.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc.0 != 0 {
            crate::install::log_line(&format!("native kbd: RegCreateKeyEx failed rc={}", rc.0));
            return;
        }
        let value = wide(AUTO_INVOKE_VALUE);
        let prior = read_dword(hkey, &value);
        let zero = 0u32.to_le_bytes();
        let rc = RegSetValueExW(hkey, PCWSTR(value.as_ptr()), 0, REG_DWORD, Some(&zero));
        if rc.0 != 0 {
            crate::install::log_line(&format!("native kbd: RegSetValueEx failed rc={}", rc.0));
        } else {
            crate::install::log_line(&format!(
                "native kbd: disabled touch keyboard auto-invoke (prior={prior:?})"
            ));
            *saved = Some(prior);
        }
        let _ = RegCloseKey(hkey);
    }
}

/// Restore the auto-invoke value saved by [`disable_auto_invoke`] (delete it if
/// it was originally absent). No-op if we never overrode it.
pub fn restore_auto_invoke() {
    let Ok(mut saved) = AUTO_INVOKE_SAVED.lock() else {
        return;
    };
    let Some(prior) = saved.take() else {
        return;
    };
    unsafe {
        let subkey = wide(TIP_SUBKEY);
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_USERS,
            PCWSTR(subkey.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc.0 != 0 {
            return;
        }
        let value = wide(AUTO_INVOKE_VALUE);
        match prior {
            Some(v) => {
                let _ = RegSetValueExW(hkey, PCWSTR(value.as_ptr()), 0, REG_DWORD, Some(&v.to_le_bytes()));
            }
            None => {
                let _ = RegDeleteValueW(hkey, PCWSTR(value.as_ptr()));
            }
        }
        crate::install::log_line("native kbd: restored touch keyboard auto-invoke");
        let _ = RegCloseKey(hkey);
    }
}

unsafe fn read_dword(hkey: HKEY, value_name: &[u16]) -> Option<u32> {
    let mut ty = REG_VALUE_TYPE::default();
    let mut data = [0u8; 4];
    let mut len = data.len() as u32;
    let rc = RegQueryValueExW(
        hkey,
        PCWSTR(value_name.as_ptr()),
        None,
        Some(&mut ty),
        Some(data.as_mut_ptr()),
        Some(&mut len),
    );
    if rc.0 == 0 && len == 4 {
        Some(u32::from_le_bytes(data))
    } else {
        None
    }
}

pub fn suppress() {
    unsafe {
        let _ = EnumWindows(Some(enum_window), LPARAM(0));
    }
}

pub fn suppress_for(duration: Duration) {
    if SUPPRESSING.swap(true, Ordering::SeqCst) {
        return;
    }
    if thread::Builder::new()
        .name("warmup-native-keyboard-suppress".into())
        .spawn(move || {
            let _ = super::desktop::attach_input();
            let deadline = Instant::now() + duration;
            while Instant::now() < deadline {
                suppress();
                thread::sleep(Duration::from_millis(25));
            }
            suppress();
            SUPPRESSING.store(false, Ordering::SeqCst);
        })
        .is_err()
    {
        SUPPRESSING.store(false, Ordering::SeqCst);
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, _param: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }

    let class = window_class(hwnd);
    let title = window_title(hwnd);
    let process = window_process_image(hwnd);
    if is_native_keyboard_window(&class, &title, process.as_deref()) {
        crate::install::log_line(&format!(
            "native keyboard suppress: class='{class}' title='{title}' process='{}'",
            process.as_deref().unwrap_or("")
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

fn window_process_image(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 32768];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result
            .ok()
            .map(|_| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn is_native_keyboard_window(class: &str, title: &str, process: Option<&str>) -> bool {
    let image = process
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .unwrap_or_default();
    class == "IPTip_Main_Window"
        || class == "IPTip_Window"
        || class == "ApplicationFrameWindow" && title == "Windows Input Experience"
        || image.eq_ignore_ascii_case("TextInputHost.exe")
        || image.eq_ignore_ascii_case("TabTip.exe")
        || image.eq_ignore_ascii_case("osk.exe")
        || (class == "Windows.UI.Core.CoreWindow"
            && (title == "Microsoft Text Input Application"
                || title == "Windows Input Experience"
                || title.contains("Text Input")))
}
