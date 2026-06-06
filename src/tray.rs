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
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadIconW, LoadImageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    TrackPopupMenu, TranslateMessage, HICON, IDI_APPLICATION, IMAGE_ICON, LR_LOADFROMFILE,
    MF_CHECKED, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_APP, WM_COMMAND, WM_DESTROY,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

const CLASS_NAME: windows::core::PCWSTR = w!("WarmupCompanionTray");
const INSTALLED_ICON_PATH: &str = r"C:\ProgramData\WarmupVk\bin\icon.ico";
const SOURCE_ICON_PATH: &str =
    r"C:\Users\jonas\warmUp-browser\apps\desktop\src-tauri\icons\icon.ico";
const TRAY_UID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 10;
const ADD_RETRY_TIMER_ID: usize = 1;
const ADD_RETRY_TIMER_MS: u32 = 1000;
const MENU_TOGGLE_POLL: usize = 1001;
const MENU_EXIT: usize = 1002;

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
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::SeqCst);
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
    for path in [INSTALLED_ICON_PATH, SOURCE_ICON_PATH].map(Path::new) {
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

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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
                }
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
    let flags = MF_STRING | if paused { MF_CHECKED } else { Default::default() };
    let toggle_label = if paused {
        w!("Start gamepad poll")
    } else {
        w!("Stop gamepad poll")
    };
    let _ = AppendMenuW(menu, flags, MENU_TOGGLE_POLL, toggle_label);
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
