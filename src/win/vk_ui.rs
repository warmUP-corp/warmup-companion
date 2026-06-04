//! Native `WarmupXboxVkWindow` — prototype Xbox-style on-screen keyboard.
//!
//! Paint-only adapter over [`super::desktop_window`]. The shared band owns the
//! thread, class registration, and message pump; this module supplies the
//! wndproc, the show/hide bodies, and the WinEvent reattach hooks. Show/hide are
//! handled in the band's pump (thread messages have `hwnd == NULL`) — **not** in
//! `vk_wndproc`.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::vk_nav::{self, KeyAction, KeyCell};

use windows::core::w;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, ValidateRect, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics, GetWindowRect,
    IsWindowVisible, KillTimer, PostThreadMessageW, SetTimer, SetWindowPos, ShowWindow,
    EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_FOREGROUND, HMENU, HWND_NOTOPMOST, HWND_TOPMOST,
    MA_NOACTIVATE, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WINDOWPOS, WINEVENT_OUTOFCONTEXT, WM_DESTROY,
    WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WM_WINDOWPOSCHANGING, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::desktop;
use super::desktop_window::{
    self, DesktopApp, DesktopWindowThread, WM_APP_HIDE, WM_APP_REPAINT, WM_APP_SHOW,
};
use super::vk_log;
use super::vk_renderer::{self, VkPalette, VkRenderer};

const WINDOW_CLASS: windows::core::PCWSTR = w!("WarmupXboxVkWindow");

/// Joyxoff docks the keyboard full-monitor-width at the screen bottom; its height is
/// `monitorHeight * 384/1080` (`_DAT_00494db8`=384 @ the 1080p reference monitor —
/// see `warmup_create_xbox_vk_window` + `FUN_00467190`).
const VK_REF_MONITOR_H: f32 = 1080.0;
const VK_KB_REF_H: f32 = 384.0;
/// Re-assert topmost while visible (shell search/task UI also uses HWND_TOPMOST).
const VK_ZORDER_TIMER_ID: usize = 1;
const VK_ZORDER_TIMER_MS: u32 = 200;
/// Joyxoff `FUN_00466970` drives frames from a timer (id 100); we use 16 ms (~60 Hz).
const VK_RENDER_TIMER_ID: usize = 2;
const VK_RENDER_TIMER_MS: u32 = 16;
const VK_SHOW_ANIMATION_MS: u64 = 180;
const VK_HIDE_ANIMATION_MS: u64 = 130;

static VK_VISIBLE: AtomicBool = AtomicBool::new(false);
static UI_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static VK_HWND: AtomicIsize = AtomicIsize::new(0);

/// Class background brush colour (dark default; per-paint theme overrides it).
const BG_FILL: u32 = 0x001f1f1f;

/// `0xRRGGBB` -> GDI `COLORREF` (`0x00BBGGRR`).
const fn rgb(v: u32) -> u32 {
    let r = (v >> 16) & 0xff;
    let g = (v >> 8) & 0xff;
    let b = v & 0xff;
    (b << 16) | (g << 8) | r
}

/// Dark/light accent + greys from `FUN_00466970` (dark accent `0xff4c7b99`,
/// light accent `0xff0e80c7`; the WinRT `UISettings` override is not applied).
fn vk_palette(dark: bool) -> VkPalette {
    let mut pal = if dark {
        VkPalette {
            bg: rgb(0x1f1f1f),
            key: rgb(0x2b2b2b),
            accent: rgb(0x4c7b99),
            text: rgb(0xffffff),
            sel_text: rgb(0xffffff),
            border: rgb(0x34384a),
        }
    } else {
        VkPalette {
            bg: rgb(0xf3f3f3),
            key: rgb(0xe9e9e9),
            accent: rgb(0x0e80c7),
            text: rgb(0x000000),
            sel_text: rgb(0xffffff),
            border: rgb(0xcfcfcf),
        }
    };
    let theme = crate::config::keyboard_theme();
    if let Some(v) = theme.bg {
        pal.bg = v;
    }
    if let Some(v) = theme.key {
        pal.key = v;
    }
    if let Some(v) = theme.accent {
        pal.accent = v;
    }
    if let Some(v) = theme.text {
        pal.text = v;
    }
    if let Some(v) = theme.sel_text {
        pal.sel_text = v;
    }
    if let Some(v) = theme.border {
        pal.border = v;
    }
    pal
}

/// Joyxoff `param_1[0x65]` dark flag, here read live from the OS theme.
/// `HKCU\...\Themes\Personalize\AppsUseLightTheme` (0 = dark). Defaults to dark.
fn is_dark_theme() -> bool {
    unsafe {
        let mut val: u32 = 0;
        let mut sz = std::mem::size_of::<u32>() as u32;
        let res = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut val as *mut u32 as *mut core::ffi::c_void),
            Some(&mut sz),
        );
        if res.is_ok() {
            val == 0
        } else {
            true
        }
    }
}

/// Glyph + font face for a key (Joyxoff renders special keys from Segoe MDL2 Assets;
/// we use the equivalent Unicode symbols, which Segoe UI Symbol covers reliably).
/// Returns `(text, is_symbol_font)`.
fn key_glyph(key: &KeyCell) -> (String, bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN, VK_SPACE};
    match &key.action {
        KeyAction::Shift | KeyAction::CapsLock => (key.label.clone(), false),
        KeyAction::CloseVk => ("\u{2325}".to_string(), true), // keyboard dismiss
        KeyAction::VoiceInput => (String::new(), false),
        KeyAction::Paste => (key.label.clone(), false),
        KeyAction::Vk(vk) if *vk == VK_BACK => (key.label.clone(), false),
        KeyAction::Vk(vk) if *vk == VK_RETURN => (key.label.clone(), false),
        KeyAction::Vk(vk) if *vk == VK_SPACE => (String::new(), false),
        _ => (key.label.clone(), false),
    }
}

fn key_hint(key: &KeyCell) -> Option<&'static str> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN, VK_SPACE};
    match &key.action {
        // Badges only for face buttons that fire without selecting the key first
        // (`gamepad::handle_vk_open_button`).
        KeyAction::Vk(vk) if *vk == VK_BACK => Some("B"),
        KeyAction::Vk(vk) if *vk == VK_RETURN => Some("RB"),
        KeyAction::Vk(vk) if *vk == VK_SPACE => Some("X"),
        KeyAction::Shift => Some("LT"),
        KeyAction::CapsLock => Some("RT"),
        KeyAction::VoiceInput => Some("Y"),
        KeyAction::CloseVk => Some("L3"),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum VkAttach {
    Current,
    Input,
}

struct UiState {
    hwnd: Option<HWND>,
    renderer: Option<VkRenderer>,
    /// Foreground app window we shrank to make room for the keyboard, and its
    /// original rect to restore on hide.
    reserved: Option<(HWND, windows::Win32::Foundation::RECT)>,
}

thread_local! {
    static UI: RefCell<UiState> = const {
        RefCell::new(UiState {
            hwnd: None,
            renderer: None,
            reserved: None,
        })
    };
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

pub fn request_hide() {
    let tid = UI_THREAD_ID.load(Ordering::Acquire);
    if tid == 0 {
        return;
    }
    unsafe {
        let _ = PostThreadMessageW(tid, WM_APP_HIDE, WPARAM(0), LPARAM(0));
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
    if matches!(attach, VkAttach::Input) {
        super::native_keyboard::suppress_for(Duration::from_secs(10));
    }
    // Capture the app that currently has focus BEFORE we create our (NOACTIVATE)
    // window, so we can shrink it to make room for the keyboard.
    let prev_fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    // Read the HWND only; destroy_vk_window clears state + drops the renderer in order.
    // The borrow is released here, so the synchronous WM_DESTROY can re-borrow safely.
    let old = UI.with(|ui| ui.borrow().hwnd);
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
    match unsafe { VkRenderer::create(hwnd) } {
        Ok(r) => {
            UI.with(|ui| {
                let mut state = ui.borrow_mut();
                state.hwnd = Some(hwnd);
                state.renderer = Some(r);
            });
        }
        Err(e) => {
            vk_log::log(&format!("D2D/DComp renderer init failed: {e}"));
            unsafe {
                destroy_vk_window(hwnd);
            }
            VK_VISIBLE.store(false, Ordering::SeqCst);
            return;
        }
    }
    vk_nav::reset_selection();
    unsafe {
        show_and_place(hwnd);
        // Shrink the previously-focused app so the keyboard doesn't cover it.
        let (_, dock_top, _, _) = vk_dock_rect();
        let reserved = reserve_app_space(prev_fg, dock_top);
        UI.with(|ui| ui.borrow_mut().reserved = reserved);
        render_frame();
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
    // Copy the HWND out and drop the borrow before calling render_frame, which takes
    // its own borrow_mut on UI — holding this borrow across it panics ("already borrowed").
    let hwnd = UI.with(|ui| ui.borrow().hwnd);
    if let Some(h) = hwnd {
        unsafe {
            ensure_topmost(h);
        }
        render_frame();
    }
}

fn ui_hide() {
    // Read the HWND only; destroy_vk_window owns clearing state + dropping the renderer
    // so teardown order (renderer before DestroyWindow) stays correct.
    let hwnd = UI.with(|ui| ui.borrow().hwnd);
    let Some(h) = hwnd else {
        return;
    };
    // Restore the app window we shrank on show.
    let reserved = UI.with(|ui| ui.borrow_mut().reserved.take());
    unsafe {
        animate_hide(h);
        destroy_vk_window(h);
        restore_app_space(reserved);
    }
    VK_HWND.store(0, Ordering::Release);
    VK_VISIBLE.store(false, Ordering::SeqCst);
    vk_log::log("WarmupXboxVkWindow hidden");
}

/// Shrink `app` so its bottom sits at `dock_top`, freeing the keyboard's strip.
/// Returns the original rect to restore later. Skips our own window and shell/system
/// windows (don't reflow the desktop or sign-in UI).
unsafe fn reserve_app_space(
    app: HWND,
    dock_top: i32,
) -> Option<(HWND, windows::Win32::Foundation::RECT)> {
    if app.0.is_null() {
        return None;
    }
    let mut cls = [0u16; 64];
    let n = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(app, &mut cls);
    let name = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
    let blocked = [
        "WarmupXboxVkWindow",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "Progman",
        "WorkerW",
        "Windows.UI.Core.CoreWindow",
        "LogonUI",
    ];
    if blocked.iter().any(|b| name == *b) {
        return None;
    }
    let mut r = windows::Win32::Foundation::RECT::default();
    if GetWindowRect(app, &mut r).is_err() || r.bottom <= dock_top {
        return None;
    }
    let new_h = (dock_top - r.top).max(120);
    let _ = SetWindowPos(
        app,
        HWND::default(),
        r.left,
        r.top,
        r.right - r.left,
        new_h,
        SWP_NOACTIVATE | SWP_NOZORDER,
    );
    vk_log::log(&format!(
        "reserved space: shrank '{name}' to bottom={dock_top}"
    ));
    Some((app, r))
}

/// Restore a window shrunk by [`reserve_app_space`].
unsafe fn restore_app_space(saved: Option<(HWND, windows::Win32::Foundation::RECT)>) {
    if let Some((app, r)) = saved {
        let _ = SetWindowPos(
            app,
            HWND::default(),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
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
        super::native_keyboard::suppress();
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
            render_frame();
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i32;
            if let Some(key) = hit_test(hwnd, x, y) {
                vk_nav::activate_key(&key);
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => {
            match _wparam.0 {
                VK_ZORDER_TIMER_ID => {
                    ensure_topmost(hwnd);
                    super::native_keyboard::suppress();
                }
                VK_RENDER_TIMER_ID => render_frame(),
                _ => {}
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
            stop_timers(hwnd);
            let _ = UI.try_with(|ui| {
                if let Ok(mut state) = ui.try_borrow_mut() {
                    if state.hwnd == Some(hwnd) {
                        state.hwnd = None;
                        state.renderer = None;
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
    // Joyxoff `JoyXboxVkWindow` ex_style is 0x8280088, but its LAYERED bit is dropped
    // here: DirectComposition owns the surface via NOREDIRECTIONBITMAP, and a LAYERED
    // window stays invisible until SetLayeredWindowAttributes/UpdateLayeredWindow — which
    // can't apply with no redirection bitmap. NOREDIRECTIONBITMAP is the required flag.
    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP
}

/// Full bounds of the monitor that hosts the foreground window (Joyxoff
/// `FUN_00467190`: `MonitorFromWindow` + `GetMonitorInfo`, full `rcMonitor`).
unsafe fn target_monitor_rect() -> windows::Win32::Foundation::RECT {
    let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
    let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(mon, &mut mi).as_bool() {
        return mi.rcMonitor;
    }
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    vk_log::log(&format!("GetMonitorInfo failed; using screen {sw}x{sh}"));
    windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: sw,
        bottom: sh,
    }
}

/// Keyboard geometry. Returns `(x, y, width, height)`.
///
/// - **Docked** (Joyxoff default): full monitor width along the bottom edge, height
///   = `monitorHeight * 384/1080`.
/// - **Floating**: a compact, horizontally-centred panel raised off the bottom edge —
///   emulates the warmUP webview keyboard card. Pushed via the desktop `config.vkMode`.
unsafe fn vk_dock_rect() -> (i32, i32, i32, i32) {
    let m = target_monitor_rect();
    let full_w = (m.right - m.left).max(1);
    let full_h = (m.bottom - m.top).max(1);
    let h = ((full_h as f32) * VK_KB_REF_H / VK_REF_MONITOR_H).round() as i32;
    let h = h.clamp(160, full_h);
    match crate::config::vk_layout_mode() {
        crate::config::VkLayoutMode::Floating => {
            // Size the card to wrap chips + keys at the *docked* key scale (scale_w = monitor
            // width), so floating keys keep the same spacing as the docked bar. The card height
            // is the chip chrome + the key block + one pad of slack — no full-bar letterbox.
            let rows = vk_nav::rows_snapshot();
            let scale_w = full_w as f32;
            let (grid_w, block_h) = vk_renderer::grid_size(scale_w, &rows);
            let pad = vk_renderer::FLOATING_PAD;
            let chrome = vk_renderer::top_chrome_inset();
            let w = ((grid_w + pad * 2.0).round() as i32).min(full_w);
            let card_h = ((chrome + block_h + pad).round() as i32).clamp(160, full_h);
            // Sit close to the bottom edge — just a small breathing gap.
            let margin = (((full_h as f32) * 0.015).round() as i32).clamp(10, 48);
            let x = m.left + (full_w - w) / 2;
            let y = m.bottom - card_h - margin;
            (x, y, w, card_h)
        }
        crate::config::VkLayoutMode::Docked => (m.left, m.bottom - h, full_w, h),
    }
}

/// Width used to scale key size (Joyxoff 92px @ 1920 reference). Always the monitor
/// width so floating keys render at the same scale as the docked bar, independent of
/// the narrower floating card width.
unsafe fn vk_scale_w() -> f32 {
    let m = target_monitor_rect();
    ((m.right - m.left).max(1)) as f32
}

unsafe fn create_vk_window() -> Result<HWND, String> {
    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let (x, y, outer_w, outer_h) = vk_dock_rect();
    let hwnd = CreateWindowExW(
        window_ex_style(),
        WINDOW_CLASS,
        w!("Warmup Companion"),
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
    // No SetLayeredWindowAttributes: with NOREDIRECTIONBITMAP there is no GDI surface;
    // the DirectComposition swapchain (premultiplied alpha) supplies all pixels.
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

unsafe fn start_timers(hwnd: HWND) {
    let _ = SetTimer(hwnd, VK_ZORDER_TIMER_ID, VK_ZORDER_TIMER_MS, None);
    let _ = SetTimer(hwnd, VK_RENDER_TIMER_ID, VK_RENDER_TIMER_MS, None);
}

unsafe fn stop_timers(hwnd: HWND) {
    let _ = KillTimer(hwnd, VK_ZORDER_TIMER_ID);
    let _ = KillTimer(hwnd, VK_RENDER_TIMER_ID);
}

unsafe fn show_and_place(hwnd: HWND) {
    let (x, y, outer_w, outer_h) = vk_dock_rect();
    let start_y = y + outer_h;
    // Never activate. Joyxoff's `JoyXboxVkWindow` is NOACTIVATE and shown without
    // taking foreground, so the focused control (winlogon password edit) keeps focus
    // and Windows never auto-invokes the native touch keyboard.
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        start_y,
        outer_w,
        outer_h,
        SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    ensure_topmost(hwnd);
    // Draw the keyboard once up front; the slide then just moves the window, so the
    // already-composited DComp content rides along without a per-frame redraw.
    render_frame();
    animate_window_y(hwnd, x, start_y, y, outer_w, outer_h, VK_SHOW_ANIMATION_MS);
    start_timers(hwnd);
    log_window_rect(hwnd);
}

unsafe fn animate_hide(hwnd: HWND) {
    let (x, y, outer_w, outer_h) = vk_dock_rect();
    animate_window_y(
        hwnd,
        x,
        y,
        y + outer_h,
        outer_w,
        outer_h,
        VK_HIDE_ANIMATION_MS,
    );
}

unsafe fn animate_window_y(
    hwnd: HWND,
    x: i32,
    from_y: i32,
    to_y: i32,
    width: i32,
    height: i32,
    duration_ms: u64,
) {
    let started = Instant::now();
    let duration = Duration::from_millis(duration_ms.max(1));
    loop {
        let t = (started.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.0);
        let eased = ease_out_cubic(t);
        let y = from_y as f32 + (to_y - from_y) as f32 * eased;
        // Move only — freeze z-order (SWP_NOZORDER) so the compositor doesn't reinsert
        // the window every frame, and skip the per-frame redraw (content is unchanged
        // during the slide). Topmost was asserted once before the loop; the 200ms timer
        // + WM_WINDOWPOSCHANGING keep it pinned afterwards.
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y.round() as i32,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
        );
        if t >= 1.0 {
            break;
        }
        thread::sleep(Duration::from_millis(VK_RENDER_TIMER_MS as u64));
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

unsafe fn destroy_vk_window(hwnd: HWND) {
    stop_timers(hwnd);
    // Release the renderer (D3D/D2D/DirectComposition, all bound to this HWND) BEFORE
    // DestroyWindow. Dropping the composition target after its HWND is gone crashes.
    // The borrow is dropped before DestroyWindow, so the synchronous WM_DESTROY can
    // re-borrow UI safely.
    let renderer = UI.with(|ui| {
        let mut state = ui.borrow_mut();
        if state.hwnd == Some(hwnd) {
            state.hwnd = None;
            state.renderer.take()
        } else {
            None
        }
    });
    drop(renderer);
    let _ = DestroyWindow(hwnd);
}

fn render_frame() {
    UI.with(|ui| {
        let mut state = ui.borrow_mut();
        let Some(hwnd) = state.hwnd else {
            return;
        };
        let Some(renderer) = state.renderer.as_mut() else {
            return;
        };
        unsafe {
            if let Err(e) = renderer.resize(hwnd) {
                vk_log::log(&format!("renderer resize: {e}"));
            }
            let pal = vk_palette(is_dark_theme());
            let rows = vk_nav::rows_snapshot();
            let sel = vk_nav::selection();
            let candidates = crate::vk_predict::strip_view();
            let top_inset = vk_renderer::top_chrome_inset();
            let scale_w = vk_scale_w();
            let floating = matches!(
                crate::config::vk_layout_mode(),
                crate::config::VkLayoutMode::Floating
            );
            if let Err(e) = renderer.draw(
                &pal,
                &rows,
                sel,
                key_glyph,
                key_hint,
                top_inset,
                scale_w,
                candidates.as_ref(),
                floating,
            ) {
                vk_log::log(&format!("renderer draw: {e}"));
            }
        }
    });
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

fn hit_test(hwnd: HWND, x: i32, y: i32) -> Option<KeyCell> {
    let mut client = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let rows = vk_nav::rows_snapshot();
    let (xf, yf) = (x as f32, y as f32);
    // Same layout the renderer draws with, so clicks always match the visible keys.
    let top_inset = vk_renderer::top_chrome_inset();
    let scale_w = unsafe { vk_scale_w() };
    for kr in vk_renderer::key_rects(
        client.right as f32,
        client.bottom as f32,
        scale_w,
        &rows,
        top_inset,
    ) {
        if xf >= kr.left && xf < kr.right && yf >= kr.top && yf < kr.bottom {
            return rows
                .get(kr.pos.row)
                .and_then(|r| r.keys.get(kr.pos.col))
                .cloned();
        }
    }
    None
}
