//! Native `WarmupXboxVkWindow` — prototype Xbox-style on-screen keyboard.
//!
//! Dedicated UI thread (Joyxoff-style). `PostThreadMessage` show/hide are handled in the
//! thread message loop — **not** in `wndproc` (thread messages have `hwnd == NULL`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::vk_nav::{self, KeyCell, ROWS};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    RedrawWindow,
    SetBkMode, SetTextColor, BACKGROUND_MODE, DT_CENTER, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
    RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetSystemMetrics, GetWindowRect, IsWindowVisible, KillTimer, LoadCursorW, PeekMessageW,
    PostQuitMessage, PostThreadMessageW, RegisterClassW, SetTimer, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TranslateMessage, SM_CXSCREEN, SM_CYSCREEN,
    CS_HREDRAW, CS_VREDRAW, HMENU, HWND_NOTOPMOST, HWND_TOPMOST, MSG, PM_NOREMOVE,
    SPI_GETWORKAREA, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPOS, WM_DESTROY, WM_LBUTTONDOWN, WM_PAINT, WM_TIMER,
    WM_USER, WM_WINDOWPOSCHANGING, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_EX_TOOLWINDOW, WS_POPUP,
    WNDCLASSW,
};

use super::desktop;
use super::vk_log;

const CLASS_NAME: windows::core::PCWSTR = w!("WarmupXboxVkWindow");
const WM_WARMUP_SHOW: u32 = WM_USER;
const WM_WARMUP_HIDE: u32 = WM_USER + 1;
const WM_WARMUP_QUIT: u32 = WM_USER + 2;
const WM_WARMUP_REPAINT: u32 = WM_USER + 3;

const VK_WIDTH: i32 = 720;
const VK_HEIGHT: i32 = 260;
/// Re-assert topmost while visible (shell search/task UI also uses HWND_TOPMOST).
const VK_ZORDER_TIMER_ID: usize = 1;
const VK_ZORDER_TIMER_MS: u32 = 200;

static VK_VISIBLE: AtomicBool = AtomicBool::new(false);
static UI_THREAD_ID: AtomicU32 = AtomicU32::new(0);

const SEL_FILL: u32 = 0x00E85D04;
const KEY_FILL: u32 = 0x00303030;
const BG_FILL: u32 = 0x001a1a1a;

#[derive(Clone, Copy, Debug)]
pub enum VkAttach {
    Current,
    Input,
}

struct UiState {
    hwnd: Option<HWND>,
}

thread_local! {
    static UI: std::cell::RefCell<UiState> = std::cell::RefCell::new(UiState { hwnd: None });
}

pub struct VkUiThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

pub fn is_vk_visible() -> bool {
    VK_VISIBLE.load(Ordering::SeqCst)
}

pub fn request_repaint() {
    let tid = UI_THREAD_ID.load(Ordering::Acquire);
    if tid == 0 {
        return;
    }
    unsafe {
        let _ = PostThreadMessageW(tid, WM_WARMUP_REPAINT, WPARAM(0), LPARAM(0));
    }
}

pub fn tick_dpad_hold(now: Instant) -> bool {
    if vk_nav::tick_dpad_hold(now) {
        request_repaint();
        true
    } else {
        false
    }
}

impl VkUiThread {
    pub fn spawn(attach: VkAttach) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);
        let join = thread::Builder::new()
            .name("warmup-vk-ui".into())
            .spawn(move || ui_thread_main(ready_tx, attach))
            .map_err(|e| format!("vk ui thread: {e}"))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| "vk ui thread exited before ready".to_string())??;
        Ok(Self {
            thread_id,
            join: Some(join),
        })
    }

    pub fn show(&self, attach: VkAttach) -> Result<(), String> {
        unsafe {
            PostThreadMessageW(
                self.thread_id,
                WM_WARMUP_SHOW,
                WPARAM(0),
                LPARAM(attach as isize),
            )
            .map_err(|e| format!("PostThreadMessageW show: {e}"))?;
        }
        Ok(())
    }

    pub fn hide(&self) -> Result<(), String> {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_WARMUP_HIDE, WPARAM(0), LPARAM(0))
                .map_err(|e| format!("PostThreadMessageW hide: {e}"))?;
        }
        Ok(())
    }
}

impl Drop for VkUiThread {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_WARMUP_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn ui_thread_main(ready: mpsc::SyncSender<Result<u32, String>>, attach: VkAttach) {
    if let Err(e) = try_attach_for_window(attach) {
        let _ = ready.send(Err(format!("desktop attach failed: {e}")));
        return;
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).expect("module handle");
        let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x001a1a1a));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(vk_wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            hCursor: LoadCursorW(None, windows::Win32::UI::WindowsAndMessaging::IDC_ARROW)
                .expect("cursor"),
            hbrBackground: bg,
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&wc);
        // Message queue must exist before main thread posts WM_WARMUP_SHOW.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    }
    let thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    UI_THREAD_ID.store(thread_id, Ordering::Release);
    let _ = ready.send(Ok(thread_id));

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        match msg.message {
            WM_WARMUP_SHOW => {
                let attach = if msg.lParam.0 == 1 {
                    VkAttach::Input
                } else {
                    VkAttach::Current
                };
                ui_show(attach);
            }
            WM_WARMUP_HIDE => ui_hide(),
            WM_WARMUP_REPAINT => ui_repaint(),
            WM_WARMUP_QUIT => unsafe {
                PostQuitMessage(0);
            },
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
}

fn ui_show(attach: VkAttach) {
    // Drop RefCell borrow before DestroyWindow — wndproc re-enters and must not borrow UI.
    let old = UI.with(|ui| ui.borrow_mut().hwnd.take());
    if let Some(h) = old {
        unsafe {
            destroy_vk_window(h);
        }
    }
    if let Err(e) = try_attach_for_window(attach) {
        vk_log::log(&format!("desktop attach failed: {e} (continuing)"));
    }
    if let Some(name) = desktop::current_desktop_name() {
        vk_log::log(&format!("UI thread desktop: {name}"));
    }
    let hwnd = match unsafe { create_vk_window() } {
        Ok(h) => h,
        Err(e) => {
            vk_log::log(&format!("create window failed: {e}"));
            VK_VISIBLE.store(false, Ordering::SeqCst);
            return;
        }
    };
    UI.with(|ui| ui.borrow_mut().hwnd = Some(hwnd));
    vk_nav::reset_selection();
    unsafe {
        show_and_place(hwnd);
    }
    let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
    VK_VISIBLE.store(visible, Ordering::SeqCst);
    if visible {
        vk_log::log("WarmupXboxVkWindow shown");
    } else {
        vk_log::log("ShowWindow done but IsWindowVisible=false (wrong desktop/session?)");
    }
}

fn ui_repaint() {
    UI.with(|ui| {
        if let Some(h) = ui.borrow().hwnd {
            unsafe {
                ensure_topmost(h);
                let _ = InvalidateRect(h, None, true);
                let _ = RedrawWindow(h, None, None, RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN);
            }
        }
    });
}

fn ui_hide() {
    let hwnd = UI.with(|ui| ui.borrow_mut().hwnd.take());
    let Some(h) = hwnd else {
        return;
    };
    unsafe {
        destroy_vk_window(h);
    }
    VK_VISIBLE.store(false, Ordering::SeqCst);
    vk_log::log("WarmupXboxVkWindow hidden");
}

unsafe extern "system" fn vk_wndproc(
    hwnd: HWND,
    msg: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_keys(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i32;
            if let Some(key) = hit_test(hwnd, x, y) {
                vk_nav::activate_key(key);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if _wparam.0 == VK_ZORDER_TIMER_ID {
                ensure_topmost(hwnd);
            }
            LRESULT(0)
        }
        WM_WINDOWPOSCHANGING => {
            // Keep above fullscreen apps and other topmost windows (Joyxoff-style).
            let pos = lparam.0 as *mut WINDOWPOS;
            if !pos.is_null() {
                let pos = &mut *pos;
                if !pos.flags.contains(SWP_NOZORDER) {
                    pos.hwndInsertAfter = HWND_TOPMOST;
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            stop_zorder_timer(hwnd);
            let _ = UI.try_with(|ui| {
                if let Ok(mut state) = ui.try_borrow_mut() {
                    if state.hwnd == Some(hwnd) {
                        state.hwnd = None;
                    }
                }
            });
            VK_VISIBLE.store(false, Ordering::SeqCst);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) },
    }
}

fn try_attach_for_window(attach: VkAttach) -> Result<(), String> {
    match attach {
        VkAttach::Current => Ok(()),
        VkAttach::Input => desktop::attach_input(),
    }
}

fn window_style() -> windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE {
    WS_POPUP
}

fn window_ex_style() -> windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE {
    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
}

unsafe fn outer_window_size() -> (i32, i32) {
    (VK_WIDTH, VK_HEIGHT)
}

unsafe fn work_area_bottom_center(outer_w: i32, outer_h: i32) -> (i32, i32) {
    let mut work = windows::Win32::Foundation::RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut work as *mut _ as *mut _),
        windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    let (w, h) = if work.right > work.left && work.bottom > work.top {
        (work.right - work.left, work.bottom - work.top)
    } else {
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        vk_log::log(&format!("SPI_GETWORKAREA empty; using screen {sw}x{sh}"));
        (sw, sh)
    };
    let x = work.left + (w - outer_w) / 2;
    let y = work.top + h - outer_h - 12;
    (x, y)
}

unsafe fn create_vk_window() -> Result<HWND, String> {
    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let (outer_w, outer_h) = outer_window_size();
    let (x, y) = work_area_bottom_center(outer_w, outer_h);
    CreateWindowExW(
        window_ex_style(),
        CLASS_NAME,
        w!("Warmup Xbox VK"),
        window_style(),
        x,
        y,
        outer_w,
        outer_h,
        None,
        HMENU::default(),
        windows::Win32::Foundation::HINSTANCE(instance.0),
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))
}

const TOPMOST_FLAGS: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(
        SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0 | SWP_SHOWWINDOW.0,
    );

const TOPMOST_REFRESH_FLAGS: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS =
    windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(
        SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0,
    );

/// Re-assert HWND_TOPMOST (call after show, repaint, or any external z-order change).
unsafe fn ensure_topmost(hwnd: HWND) {
    // Toggle topmost band so we sit above other topmost shell UI (Search, etc.).
    let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, TOPMOST_REFRESH_FLAGS);
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, TOPMOST_FLAGS);
}

unsafe fn start_zorder_timer(hwnd: HWND) {
    let _ = SetTimer(hwnd, VK_ZORDER_TIMER_ID, VK_ZORDER_TIMER_MS, None);
}

unsafe fn stop_zorder_timer(hwnd: HWND) {
    let _ = KillTimer(hwnd, VK_ZORDER_TIMER_ID);
}

unsafe fn show_and_place(hwnd: HWND) {
    let (outer_w, outer_h) = outer_window_size();
    let (x, y) = work_area_bottom_center(outer_w, outer_h);
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        y,
        outer_w,
        outer_h,
        SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    ensure_topmost(hwnd);
    start_zorder_timer(hwnd);
    let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN);
    log_window_rect(hwnd);
}

unsafe fn destroy_vk_window(hwnd: HWND) {
    stop_zorder_timer(hwnd);
    let _ = DestroyWindow(hwnd);
}

unsafe fn log_window_rect(hwnd: HWND) {
    let mut r = windows::Win32::Foundation::RECT::default();
    let _ = GetWindowRect(hwnd, &mut r);
    vk_log::log(&format!(
        "hwnd={:?} visible={} rect=({},{} {}x{})",
        hwnd.0,
        IsWindowVisible(hwnd).as_bool(),
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top
    ));
}

/// Block until the UI thread reports visible, or timeout (service needs real HWND).
pub fn wait_until_visible(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_vk_visible() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    is_vk_visible()
}

fn paint_keys(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.0.is_null() {
            return;
        }
        let mut client = windows::Win32::Foundation::RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let sel = vk_nav::selection();
        let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(BG_FILL));
        let _ = FillRect(hdc, &client, bg);
        let _ = DeleteObject(bg);
        let _ = SetBkMode(hdc, BACKGROUND_MODE(1));
        let _ = SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        let (kw, kh) = key_metrics(client.right);
        let key_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(KEY_FILL));
        let sel_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(SEL_FILL));
        let mut y = 8i32;
        for (ri, row) in ROWS.iter().enumerate() {
            let row_w = kw * row.len() as i32 + 4 * (row.len().saturating_sub(1) as i32);
            let mut x = (client.right - row_w) / 2;
            for (ci, key) in row.iter().enumerate() {
                let cell = windows::Win32::Foundation::RECT {
                    left: x,
                    top: y,
                    right: x + kw,
                    bottom: y + kh,
                };
                let brush = if sel.row == ri && sel.col == ci {
                    sel_brush
                } else {
                    key_brush
                };
                let _ = FillRect(hdc, &cell, brush);
                let mut label: Vec<u16> = key.label.encode_utf16().collect();
                let mut text_rect = cell;
                let _ = DrawTextW(
                    hdc,
                    &mut label,
                    &mut text_rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
                x += kw + 4;
            }
            y += kh + 6;
        }
        let _ = DeleteObject(key_brush);
        let _ = DeleteObject(sel_brush);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn key_metrics(client_w: i32) -> (i32, i32) {
    let max_cols = ROWS.iter().map(|r| r.len()).max().unwrap_or(10) as i32;
    let kw = ((client_w - 32) / max_cols).clamp(48, 72);
    (kw, 48)
}

fn hit_test(hwnd: HWND, x: i32, y: i32) -> Option<KeyCell> {
    let mut client = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let (kw, kh) = key_metrics(client.right);
    let mut row_y = 8i32;
    for row in ROWS {
        let row_w = kw * row.len() as i32 + 4 * (row.len().saturating_sub(1) as i32);
        let mut row_x = (client.right - row_w) / 2;
        for &key in *row {
            if x >= row_x && x < row_x + kw && y >= row_y && y < row_y + kh {
                return Some(key);
            }
            row_x += kw + 4;
        }
        row_y += kh + 6;
    }
    None
}
