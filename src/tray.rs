//! Interactive companion tray icon.

use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadIconW, LoadImageW, MessageBoxW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    TrackPopupMenu, TranslateMessage, HICON, IDI_APPLICATION, IMAGE_ICON, LR_LOADFROMFILE,
    MB_ICONINFORMATION, MB_OK, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, SW_SHOWNORMAL,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_APP, WM_COMMAND, WM_DESTROY, WM_RBUTTONUP, WM_TIMER,
    WNDCLASSW, WS_OVERLAPPED,
};

const CLASS_NAME: windows::core::PCWSTR = w!("WarmupCompanionTray");
const INSTALLED_ICON_PATH: &str = r"C:\ProgramData\WarmupVk\bin\icon.ico";
const TRAY_UID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 10;
const ADD_RETRY_TIMER_ID: usize = 1;
const ADD_RETRY_TIMER_MS: u32 = 1000;
const MENU_TOGGLE_POLL: usize = 1001;
const MENU_OPEN_LOG: usize = 1002;
const MENU_DIAGNOSTICS: usize = 1003;
const MENU_PRIVACY: usize = 1004;
const MENU_RESTORE_NATIVE_KBD: usize = 1005;
const MENU_UNINSTALL: usize = 1006;
const MENU_EXIT: usize = 1007;

const SERVICE_LOG_PATH: &str = r"C:\ProgramData\WarmupVk\service.log";

static STARTED: AtomicBool = AtomicBool::new(false);
static ICON_ADDED: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

pub fn spawn() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-tray".into())
        .spawn(tray_thread);
}

fn tray_thread() {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: windows::Win32::Foundation::HINSTANCE(instance.0),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
        let Ok(hwnd) = CreateWindowExW(
            Default::default(),
            CLASS_NAME,
            w!("Warmup Companion"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            windows::Win32::Foundation::HINSTANCE(instance.0),
            None,
        ) else {
            return;
        };
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::SeqCst,
        );
        try_add_icon(hwnd);
        if !ICON_ADDED.load(Ordering::SeqCst) {
            let _ = SetTimer(hwnd, ADD_RETRY_TIMER_ID, ADD_RETRY_TIMER_MS, None);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        delete_icon(hwnd);
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn try_add_icon(hwnd: HWND) {
    let mut nid = notify_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = load_tray_icon();
    write_tip(&mut nid, "Warmup Companion");
    if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
        ICON_ADDED.store(true, Ordering::SeqCst);
        let _ = KillTimer(hwnd, ADD_RETRY_TIMER_ID);
    }
}

unsafe fn load_tray_icon() -> HICON {
    for path in [INSTALLED_ICON_PATH].map(Path::new) {
        if !path.is_file() {
            continue;
        }
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        if let Ok(icon) = LoadImageW(
            HMODULE::default(),
            PCWSTR(wide.as_ptr()),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE,
        ) {
            return HICON(icon.0);
        }
    }
    LoadIconW(HMODULE::default(), IDI_APPLICATION).unwrap_or_default()
}

unsafe fn delete_icon(hwnd: HWND) {
    let nid = notify_data(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    ICON_ADDED.store(false, Ordering::SeqCst);
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        ..Default::default()
    }
}

fn write_tip(nid: &mut NOTIFYICONDATAW, text: &str) {
    for (slot, ch) in nid
        .szTip
        .iter_mut()
        .zip(text.encode_utf16().chain(std::iter::once(0)))
    {
        *slot = ch;
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == ADD_RETRY_TIMER_ID => {
            if !ICON_ADDED.load(Ordering::SeqCst) {
                try_add_icon(hwnd);
            }
            LRESULT(0)
        }
        WM_TRAY if lparam.0 as u32 == WM_RBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        msg if msg == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            ICON_ADDED.store(false, Ordering::SeqCst);
            try_add_icon(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            match wparam.0 & 0xffff {
                MENU_TOGGLE_POLL => {
                    let paused = !crate::gamepad_backend::userland_poll_paused();
                    crate::gamepad_backend::set_userland_poll_paused(paused);
                    let _ = crate::config::write_userland_poll_paused(paused);
                }
                MENU_OPEN_LOG => open_log(),
                MENU_DIAGNOSTICS => open_diagnostics(),
                MENU_PRIVACY => show_privacy(hwnd),
                MENU_RESTORE_NATIVE_KBD => restore_native_keyboard(hwnd),
                MENU_UNINSTALL => uninstall(),
                MENU_EXIT => {
                    crate::gamepad::request_stop();
                    PostQuitMessage(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            delete_icon(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap_or_default();
    if menu.0.is_null() {
        return;
    }
    let paused = crate::gamepad_backend::userland_poll_paused();
    let flags = MF_STRING
        | if paused {
            MF_CHECKED
        } else {
            Default::default()
        };
    let toggle_label = if paused {
        w!("Start gamepad poll")
    } else {
        w!("Stop gamepad poll")
    };
    let _ = AppendMenuW(menu, flags, MENU_TOGGLE_POLL, toggle_label);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN_LOG, w!("Open service log"));
    let _ = AppendMenuW(menu, MF_STRING, MENU_DIAGNOSTICS, w!("Run diagnostics"));
    let _ = AppendMenuW(menu, MF_STRING, MENU_PRIVACY, w!("Privacy / trust model"));
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_RESTORE_NATIVE_KBD,
        w!("Restore Windows keyboard services"),
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_UNINSTALL,
        w!("Uninstall Warmup Companion"),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        Some(null_mut()),
    );
    let _ = DestroyMenu(menu);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn shell_execute(verb: &str, file: &str, params: Option<&str>) {
    let verb = wide(verb);
    let file = wide(file);
    let params = params.map(wide);
    let _ = ShellExecuteW(
        None,
        PCWSTR(verb.as_ptr()),
        PCWSTR(file.as_ptr()),
        params
            .as_ref()
            .map(|p| PCWSTR(p.as_ptr()))
            .unwrap_or_else(PCWSTR::null),
        PCWSTR::null(),
        SW_SHOWNORMAL,
    );
}

unsafe fn open_log() {
    shell_execute("open", "notepad.exe", Some(SERVICE_LOG_PATH));
}

unsafe fn open_diagnostics() {
    let cmd = format!(
        "-NoProfile -NoExit -Command \"Write-Host 'WarmupVk service'; sc.exe qc WarmupVkSvc; sc.exe query WarmupVkSvc; Write-Host ''; Write-Host 'Recent service log'; if (Test-Path '{SERVICE_LOG_PATH}') {{ Get-Content '{SERVICE_LOG_PATH}' -Tail 240 }} else {{ Write-Host 'Missing {SERVICE_LOG_PATH}' }}\""
    );
    shell_execute("open", "powershell.exe", Some(&cmd));
}

unsafe fn show_privacy(hwnd: HWND) {
    let title = wide("Warmup Companion privacy");
    let body = wide(
        "Warmup Companion does not read host app text for prediction.\r\n\
         Prediction uses VK-only local context and is disabled on UAC, lock, and sign-in.\r\n\
         Personal dictionary learning skips password fields and UIA failures.\r\n\
         Sentry is disabled unless WARMUP_SENTRY_DSN is set.\r\n\
         Service log: C:\\ProgramData\\WarmupVk\\service.log",
    );
    let _ = MessageBoxW(
        hwnd,
        PCWSTR(body.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_OK | MB_ICONINFORMATION,
    );
}

unsafe fn restore_native_keyboard(hwnd: HWND) {
    crate::win::native_keyboard::restore_auto_invoke();
    crate::win::native_keyboard::ensure_search_service_running();
    crate::install::log_line("tray: requested Windows keyboard service restore");
    let title = wide("Warmup Companion");
    let body = wide("Requested restore of Windows touch keyboard/search services.");
    let _ = MessageBoxW(
        hwnd,
        PCWSTR(body.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_OK | MB_ICONINFORMATION,
    );
}

unsafe fn uninstall() {
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.display().to_string();
        shell_execute("runas", &exe, Some("uninstall"));
    }
}
