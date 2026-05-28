//! Temporary Winlogon debug panel. UI thread attaches to input desktop, poll thread stays put.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    SetBkMode, SetTextColor, BACKGROUND_MODE, DT_LEFT, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    LoadCursorW, PeekMessageW, PostQuitMessage, PostThreadMessageW, RegisterClassW, SetTimer,
    SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HMENU, HWND_TOPMOST, MSG,
    PM_NOREMOVE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, WM_DESTROY, WM_PAINT, WM_TIMER,
    WM_USER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WNDCLASSW,
};

use super::desktop;

const CLASS_NAME: windows::core::PCWSTR = w!("WarmupDebugOverlayWindow");
const WM_DEBUG_SHOW: u32 = WM_USER + 20;
const WM_DEBUG_HIDE: u32 = WM_USER + 21;
const WM_DEBUG_QUIT: u32 = WM_USER + 22;

const PANEL_W: i32 = 520;
const PANEL_H: i32 = 412;
const REPAINT_TIMER_ID: usize = 11;
const REPAINT_TIMER_MS: u32 = 250;
const TICK_INTERVAL: Duration = Duration::from_millis(250);

struct DebugOverlayController {
    thread: Option<DebugOverlayThread>,
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

struct DebugOverlayThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

thread_local! {
    static HWND_STATE: std::cell::Cell<Option<HWND>> = const { std::cell::Cell::new(None) };
    static F10_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static CONTROLLER: OnceLock<Mutex<DebugOverlayController>> = OnceLock::new();
static VK_TOGGLE_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn take_vk_toggle_request() -> bool {
    VK_TOGGLE_REQUESTED.swap(false, Ordering::SeqCst)
}

pub fn tick() {
    if std::env::var_os("WARMUP_VK_SERVICE").is_none_or(|v| v == "0") {
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

    if on_winlogon {
        if c.thread.is_none() {
            match DebugOverlayThread::spawn() {
                Ok(thread) => {
                    if thread.show().is_ok() {
                        service_log("debug ui: shown on Winlogon");
                    }
                    c.thread = Some(thread);
                }
                Err(e) => service_log(&format!("debug ui: spawn failed: {e}")),
            }
        } else if !c.last_on_winlogon {
            if let Some(thread) = c.thread.as_ref() {
                let _ = thread.show();
                service_log("debug ui: shown on Winlogon");
            }
        }
    } else if let Some(mut thread) = c.thread.take() {
        let _ = thread.hide();
        service_log("debug ui: hidden");
        thread.stop();
    }
    c.last_on_winlogon = on_winlogon;
}

impl DebugOverlayThread {
    fn spawn() -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);
        let join = thread::Builder::new()
            .name("warmup-debug-overlay".into())
            .spawn(move || ui_thread_main(ready_tx))
            .map_err(|e| format!("debug ui thread: {e}"))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| "debug ui thread exited before ready".to_string())??;
        Ok(Self {
            thread_id,
            join: Some(join),
        })
    }

    fn show(&self) -> Result<(), String> {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_DEBUG_SHOW, WPARAM(0), LPARAM(0))
                .map_err(|e| format!("PostThreadMessageW debug show: {e}"))?;
        }
        Ok(())
    }

    fn hide(&self) -> Result<(), String> {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_DEBUG_HIDE, WPARAM(0), LPARAM(0))
                .map_err(|e| format!("PostThreadMessageW debug hide: {e}"))?;
        }
        Ok(())
    }

    fn stop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_DEBUG_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for DebugOverlayThread {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ui_thread_main(ready: mpsc::SyncSender<Result<u32, String>>) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).expect("module handle");
        let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00101010));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(debug_wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            hCursor: LoadCursorW(None, windows::Win32::UI::WindowsAndMessaging::IDC_ARROW)
                .expect("cursor"),
            hbrBackground: bg,
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    }

    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        match msg.message {
            WM_DEBUG_SHOW => ui_show(),
            WM_DEBUG_HIDE => ui_hide(),
            WM_DEBUG_QUIT => unsafe {
                PostQuitMessage(0);
            },
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
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
        CLASS_NAME,
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
        let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00101010));
        let _ = FillRect(hdc, &rect, bg);
        let _ = DeleteObject(bg);
        let _ = SetBkMode(hdc, BACKGROUND_MODE(1));

        let snapshot = crate::debug_state::snapshot();
        let thread_desktop = desktop::current_desktop_name().unwrap_or_else(|| "?".into());
        let input_desktop = desktop::input_desktop_name().unwrap_or_else(|e| format!("? ({e})"));
        let mut lines = vec![
            "DEBUG WINLOGON".to_string(),
            format!("thread desktop: {thread_desktop}"),
            format!("input desktop: {input_desktop}"),
            format!("pid: {}", std::process::id()),
            format!("xinput: {}", snapshot.xinput_loader),
            format!("buttons: {}", snapshot.last_buttons),
            format!("action: {}", snapshot.last_action),
            format!("vk visible: {}", super::vk_ui::is_vk_visible()),
            "F10: toggle VK (debug)".to_string(),
        ];
        lines.push("---- log tail ----".to_string());
        lines.extend(snapshot.log_tail.iter().cloned());

        for (i, line) in lines.iter().enumerate() {
            let _ = SetTextColor(
                hdc,
                windows::Win32::Foundation::COLORREF(if i == 0 { 0x0000FF80 } else { 0x00FFFFFF }),
            );
            draw_line(hdc, 14, 10 + (i as i32 * 22), line);
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
    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
        crate::install::log_line(msg);
    }
}
