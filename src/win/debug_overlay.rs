//! Temporary Winlogon debug panel. Paint-only adapter over
//! [`super::desktop_window`]: the shared band owns the thread + pump, this module
//! supplies the wndproc and show/hide bodies. UI thread attaches to the input
//! desktop on show; the poll thread stays put.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    SetBkMode, SetTextColor, BACKGROUND_MODE, DT_LEFT, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, SetTimer, SetWindowPos, ShowWindow,
    HMENU, HWND_TOPMOST, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, WM_DESTROY, WM_PAINT,
    WM_TIMER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::desktop;
use super::desktop_window::{self, DesktopApp, DesktopWindowThread};

const WINDOW_CLASS: windows::core::PCWSTR = w!("WarmupDebugOverlayWindow");

const PANEL_W: i32 = 480;
const PANEL_H: i32 = 72;
const REPAINT_TIMER_ID: usize = 11;
const REPAINT_TIMER_MS: u32 = 250;
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const PANEL_BG: u32 = 0x00101010;

struct DebugOverlayController {
    thread: Option<DesktopWindowThread>,
    last_tick: Instant,
    last_on_winlogon: bool,
    input_probe_failures: u8,
    off_winlogon_streak: u8,
}

impl Default for DebugOverlayController {
    fn default() -> Self {
        Self {
            thread: None,
            last_tick: Instant::now() - TICK_INTERVAL,
            last_on_winlogon: false,
            input_probe_failures: 0,
            off_winlogon_streak: 0,
        }
    }
}

/// Debug-overlay adapter for the shared UI-thread band.
struct DebugApp;

impl DesktopApp for DebugApp {
    const THREAD_NAME: &'static str = "warmup-debug-overlay";
    const CLASS_NAME: windows::core::PCWSTR = WINDOW_CLASS;
    const BG_COLOR: u32 = PANEL_BG;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = debug_wndproc;

    fn on_show(&mut self, _lparam: LPARAM) {
        ui_show();
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

thread_local! {
    static HWND_STATE: std::cell::Cell<Option<HWND>> = const { std::cell::Cell::new(None) };
    static F10_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static F9_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static CONTROLLER: OnceLock<Mutex<DebugOverlayController>> = OnceLock::new();
static VK_TOGGLE_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn take_vk_toggle_request() -> bool {
    VK_TOGGLE_REQUESTED.swap(false, Ordering::SeqCst)
}

pub fn tick() {
    if !crate::config::service_mode() {
        return;
    }
    let controller = CONTROLLER.get_or_init(|| Mutex::new(DebugOverlayController::default()));
    let Ok(mut c) = controller.lock() else {
        return;
    };
    if c.last_tick.elapsed() < TICK_INTERVAL {
        return;
    }
    c.last_tick = Instant::now();

    let on_winlogon = match desktop::input_desktop_name() {
        Ok(name) => {
            c.input_probe_failures = 0;
            let on_winlogon = name.eq_ignore_ascii_case("Winlogon");
            if on_winlogon {
                c.off_winlogon_streak = 0;
            } else {
                c.off_winlogon_streak = c.off_winlogon_streak.saturating_add(1);
                if c.thread.is_some() && c.last_on_winlogon && c.off_winlogon_streak < 12 {
                    return;
                }
            }
            on_winlogon
        }
        Err(e) => {
            c.input_probe_failures = c.input_probe_failures.saturating_add(1);
            if c.thread.is_some() && c.last_on_winlogon && c.input_probe_failures < 12 {
                return;
            }
            service_log(&format!("debug ui: input desktop probe failed: {e}"));
            false
        }
    };

    // Keep thread alive across desktops; toggle visibility only. Tearing down and
    // respawning on every Winlogon transition caused the "goes away / comes back"
    // flicker and a race on the next CreateWindow.
    let just_spawned = if c.thread.is_none() {
        match desktop_window::spawn(DebugApp) {
            Ok(thread) => {
                c.thread = Some(thread);
                true
            }
            Err(e) => {
                service_log(&format!("debug ui: spawn failed: {e}"));
                c.last_on_winlogon = on_winlogon;
                return;
            }
        }
    } else {
        false
    };
    let transitioned = on_winlogon != c.last_on_winlogon;
    if (just_spawned || transitioned) && c.thread.is_some() {
        let thread = c.thread.as_ref().expect("checked is_some");
        if on_winlogon {
            let _ = thread.show(LPARAM(0));
            service_log("debug ui: shown on Winlogon");
        } else {
            let _ = thread.hide();
            service_log("debug ui: hidden (left Winlogon)");
        }
    }
    c.last_on_winlogon = on_winlogon;
}

fn ui_show() {
    ui_hide();
    if let Err(e) = desktop::attach_input() {
        service_log(&format!("debug ui: desktop attach failed: {e}"));
    }
    if let Some(name) = desktop::current_desktop_name() {
        service_log(&format!("debug ui: UI thread desktop: {name}"));
    }
    match unsafe { create_debug_window() } {
        Ok(hwnd) => {
            HWND_STATE.with(|state| state.set(Some(hwnd)));
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    24,
                    24,
                    PANEL_W,
                    PANEL_H,
                    SWP_SHOWWINDOW | SWP_NOACTIVATE,
                );
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = SetTimer(hwnd, REPAINT_TIMER_ID, REPAINT_TIMER_MS, None);
                let _ = InvalidateRect(hwnd, None, true);
            }
        }
        Err(e) => service_log(&format!("debug ui: create window failed: {e}")),
    }
}

fn ui_hide() {
    let hwnd = HWND_STATE.with(|state| state.take());
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
}

unsafe fn create_debug_window() -> Result<HWND, String> {
    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        WINDOW_CLASS,
        w!("Warmup Debug Overlay"),
        WS_POPUP,
        24,
        24,
        PANEL_W,
        PANEL_H,
        None,
        HMENU::default(),
        windows::Win32::Foundation::HINSTANCE(instance.0),
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))
}

unsafe extern "system" fn debug_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_debug(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == REPAINT_TIMER_ID {
                poll_debug_shortcut();
                let _ = InvalidateRect(hwnd, None, true);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            let _ = HWND_STATE.try_with(|state| {
                if state.get() == Some(hwnd) {
                    state.set(None);
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn poll_debug_shortcut() {
    let down = unsafe { GetAsyncKeyState(0x79) < 0 }; // VK_F10
    F10_DOWN.with(|was_down| {
        let pressed = down && !was_down.get();
        was_down.set(down);
        if pressed {
            VK_TOGGLE_REQUESTED.store(true, Ordering::SeqCst);
            service_log("debug shortcut: F10 toggle VK requested");
        }
    });
    // F9: dump the foreground UIA subtree (runs on the loop thread, which owns
    // the COM apartment). Probe for the credential password element's identity.
    let f9 = unsafe { GetAsyncKeyState(0x78) < 0 }; // VK_F9
    F9_DOWN.with(|was_down| {
        let pressed = f9 && !was_down.get();
        was_down.set(f9);
        if pressed {
            super::logon_focus::request_dump();
            service_log("debug shortcut: F9 UIA foreground dump requested");
        }
    });
}

fn paint_debug(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.0.is_null() {
            return;
        }

        let rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: PANEL_W,
            bottom: PANEL_H,
        };
        let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(PANEL_BG));
        let _ = FillRect(hdc, &rect, bg);
        let _ = DeleteObject(bg);
        let _ = SetBkMode(hdc, BACKGROUND_MODE(1));

        let snapshot = crate::debug_state::snapshot();
        let connected = if snapshot.connected {
            "connected"
        } else {
            "not connected"
        };
        let input = if snapshot.input.is_empty() {
            "—".to_string()
        } else {
            snapshot.input.clone()
        };
        let lines = [
            format!("gamepad: {connected}"),
            format!("input: {input}"),
        ];

        for (i, line) in lines.iter().enumerate() {
            let _ = SetTextColor(
                hdc,
                windows::Win32::Foundation::COLORREF(if i == 0 { 0x0000FF80 } else { 0x00FFFFFF }),
            );
            draw_line(hdc, 14, 10 + (i as i32 * 28), line);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe fn draw_line(hdc: windows::Win32::Graphics::Gdi::HDC, x: i32, y: i32, text: &str) {
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let mut rect = windows::Win32::Foundation::RECT {
        left: x,
        top: y,
        right: PANEL_W - 12,
        bottom: y + 20,
    };
    let _ = DrawTextW(
        hdc,
        &mut buf,
        &mut rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
}

fn service_log(msg: &str) {
    if crate::config::service_mode() {
        crate::install::log_line(msg);
    }
}
