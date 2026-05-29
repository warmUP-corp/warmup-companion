//! Native `WarmupXboxVkWindow` — prototype Xbox-style on-screen keyboard.
//!
//! Paint-only adapter over [`super::desktop_window`]. The shared band owns the
//! thread, class registration, and message pump; this module supplies the
//! wndproc, the show/hide bodies, and the WinEvent reattach hooks. Show/hide are
//! handled in the band's pump (thread messages have `hwnd == NULL`) — **not** in
//! `vk_wndproc`.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::vk_nav::{self, KeyCell, ROWS};

use windows::core::w;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    RedrawWindow,
    SetBkMode, SetTextColor, BACKGROUND_MODE, DT_CENTER, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
    RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW,
};
use windows::Win32::UI::WindowsAndMessaging::{SetLayeredWindowAttributes, LWA_ALPHA};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics, GetWindowRect,
    IsWindowVisible, KillTimer, PostThreadMessageW,
    EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
    SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW, SM_CXSCREEN, SM_CYSCREEN,
    HMENU, HWND_NOTOPMOST, HWND_TOPMOST,
    SPI_GETWORKAREA, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPOS, WM_DESTROY, WM_LBUTTONDOWN, WM_PAINT, WM_TIMER,
    WM_WINDOWPOSCHANGING, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOPMOST,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

use super::desktop;
use super::desktop_window::{self, DesktopApp, DesktopWindowThread, WM_APP_REPAINT, WM_APP_SHOW};
use super::vk_log;

const WINDOW_CLASS: windows::core::PCWSTR = w!("WarmupXboxVkWindow");

const VK_WIDTH: i32 = 720;
const VK_HEIGHT: i32 = 318;
/// Re-assert topmost while visible (shell search/task UI also uses HWND_TOPMOST).
const VK_ZORDER_TIMER_ID: usize = 1;
const VK_ZORDER_TIMER_MS: u32 = 200;

static VK_VISIBLE: AtomicBool = AtomicBool::new(false);
static UI_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static VK_HWND: AtomicIsize = AtomicIsize::new(0);

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
    static UI: RefCell<UiState> = const { RefCell::new(UiState { hwnd: None }) };
    /// WinEvent hooks installed on the UI thread (drained on pump exit).
    static WINEVENT_HOOKS: RefCell<Vec<HWINEVENTHOOK>> = const { RefCell::new(Vec::new()) };
}

/// VK adapter for the shared UI-thread band.
struct VkApp {
    attach: VkAttach,
}

impl DesktopApp for VkApp {
    const THREAD_NAME: &'static str = "warmup-vk-ui";
    const CLASS_NAME: windows::core::PCWSTR = WINDOW_CLASS;
    const BG_COLOR: u32 = BG_FILL;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = vk_wndproc;

    fn on_thread_start(&mut self) -> Result<(), String> {
        try_attach_for_window(self.attach).map_err(|e| format!("desktop attach failed: {e}"))
    }

    fn on_ready(&mut self, thread_id: u32) {
        UI_THREAD_ID.store(thread_id, Ordering::Release);
    }

    fn install_hooks(&mut self) {
        install_winevent_hooks();
    }

    fn remove_hooks(&mut self) {
        remove_winevent_hooks();
    }

    fn on_show(&mut self, lparam: LPARAM) {
        let attach = if lparam.0 == 1 {
            VkAttach::Input
        } else {
            VkAttach::Current
        };
        ui_show(attach);
    }

    fn on_hide(&mut self) {
        ui_hide();
    }

    fn on_repaint(&mut self) {
        ui_repaint();
    }
}

pub struct VkUiThread {
    handle: DesktopWindowThread,
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
        let _ = PostThreadMessageW(tid, WM_APP_REPAINT, WPARAM(0), LPARAM(0));
    }
}

#[cfg(feature = "gamepad")]
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
        let handle = desktop_window::spawn(VkApp { attach })?;
        Ok(Self { handle })
    }

    pub fn show(&self, attach: VkAttach) -> Result<(), String> {
        self.handle.show(LPARAM(attach as isize))
    }

    pub fn hide(&self) -> Result<(), String> {
        self.handle.hide()
    }
}

/// Joyxoff `warmup_create_xbox_vk_window` registers these two hooks (OUTOFCONTEXT,
/// delivered on the UI thread during the pump). DESKTOPSWITCH re-attaches us to the
/// new input desktop on lock/logon/UAC; FOREGROUND re-asserts topmost vs LogonUI.
fn install_winevent_hooks() {
    unsafe {
        let desktop_hook = SetWinEventHook(
            EVENT_SYSTEM_DESKTOPSWITCH,
            EVENT_SYSTEM_DESKTOPSWITCH,
            HMODULE::default(),
            Some(on_desktop_switch),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        let foreground_hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            HMODULE::default(),
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        WINEVENT_HOOKS.with(|h| {
            let mut h = h.borrow_mut();
            h.push(desktop_hook);
            h.push(foreground_hook);
        });
    }
}

fn remove_winevent_hooks() {
    WINEVENT_HOOKS.with(|h| {
        for hook in h.borrow_mut().drain(..) {
            if !hook.is_invalid() {
                unsafe {
                    let _ = UnhookWinEvent(hook);
                }
            }
        }
    });
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
    VK_HWND.store(hwnd.0 as isize, Ordering::Release);
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
    VK_HWND.store(0, Ordering::Release);
    VK_VISIBLE.store(false, Ordering::SeqCst);
    vk_log::log("WarmupXboxVkWindow hidden");
}

/// EVENT_SYSTEM_DESKTOPSWITCH callback. Joyxoff `FUN_0041ece0` re-attaches the VK
/// thread to the new input desktop on every desktop switch (lock screen, logon, UAC).
/// Without this our window stays stranded on the old desktop -> invisible after a
/// switch. Delivered on this UI thread (WINEVENT_OUTOFCONTEXT) during the message
/// pump, so re-show via a posted message (re-attach + window recreate happens in the
/// band's WM_APP_SHOW handler, which destroys the old window before SetThreadDesktop).
unsafe extern "system" fn on_desktop_switch(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if !VK_VISIBLE.load(Ordering::SeqCst) {
        return;
    }
    let tid = UI_THREAD_ID.load(Ordering::Acquire);
    if tid != 0 {
        let _ = PostThreadMessageW(tid, WM_APP_SHOW, WPARAM(0), LPARAM(1));
    }
}

/// EVENT_SYSTEM_FOREGROUND callback. Joyxoff `FUN_0041ed00` re-asserts topmost when
/// foreground changes; LogonUI grabs foreground aggressively on the secure desktop.
/// Event-driven re-assert complements the 200ms z-order timer.
unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if !VK_VISIBLE.load(Ordering::SeqCst) {
        return;
    }
    let tid = UI_THREAD_ID.load(Ordering::Acquire);
    if tid != 0 {
        let _ = PostThreadMessageW(tid, WM_APP_REPAINT, WPARAM(0), LPARAM(0));
    }
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
    // Joyxoff `JoyXboxVkWindow` ex_style 0x8280088 = TOPMOST|TOOLWINDOW|NOACTIVATE|LAYERED
    // (+NOREDIRECTIONBITMAP 0x200000 omitted here — not exposed by `windows-rs` 0.58).
    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED
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
    let hwnd = CreateWindowExW(
        window_ex_style(),
        WINDOW_CLASS,
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
    .map_err(|e| format!("CreateWindowExW: {e}"))?;
    // WS_EX_LAYERED window is invisible until alpha is set. Fully opaque (255).
    let _ = SetLayeredWindowAttributes(
        hwnd,
        windows::Win32::Foundation::COLORREF(0),
        255,
        LWA_ALPHA,
    );
    Ok(hwnd)
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
    // Never activate. Joyxoff's `JoyXboxVkWindow` is NOACTIVATE and shown without
    // taking foreground, so the focused control (winlogon password edit) keeps focus
    // and Windows never auto-invokes the native touch keyboard.
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
