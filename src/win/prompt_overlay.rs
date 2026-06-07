//! Winlogon "Press [L3] to open keyboard" prompt. Paint-only adapter over
//! [`super::desktop_window`] (sibling to [`super::debug_overlay`]): the shared
//! band owns the thread + pump, this module supplies the wndproc and show/hide
//! bodies. The prompt is the inverse of the keyboard — it appears only while the
//! VK is *closed* on the secure desktop, so a first-time user knows the gesture.
//!
//! Shown on Winlogon while the VK is closed. Unlike the VK window it installs no
//! WinEvent reattach hooks: it is static and simply toggles visibility as the
//! loop thread reports state via [`tick`].

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, KillTimer, SetTimer,
    SetWindowPos, ShowWindow, HMENU, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WM_DESTROY, WM_PAINT, WM_TIMER, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::desktop;
use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::vk_renderer::VkRenderer;

const WINDOW_CLASS: windows::core::PCWSTR = w!("WarmupPromptOverlayWindow");

const PANEL_W: i32 = 390;
const PANEL_H: i32 = 58;
const CONNECTED_PANEL_W: i32 = 430;
const CONNECTED_PANEL_H: i32 = 188;
/// Gap between the pill's bottom edge and the bottom of the primary monitor.
const MARGIN_BOTTOM: i32 = 72;
const REPAINT_TIMER_ID: usize = 12;
const REPAINT_TIMER_MS: u32 = 50;
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const CONNECTED_VISUAL_DURATION: Duration = Duration::from_millis(2400);

const PROMPT_PREFIX: &str = "Press ";
const PROMPT_SUFFIX: &str = " for keyboard";
const NO_PAD_PROMPT: &str = "Connect controller";

// COLORREF (0x00BBGGRR) fallbacks; matched to the VK card / debug panel look.
const DEFAULT_BG: u32 = 0x00141414;
const DEFAULT_BORDER: u32 = 0x00997b4c;
const DEFAULT_TEXT: u32 = 0x00FFFFFF;

struct PromptOverlayController {
    thread: Option<DesktopWindowThread>,
    last_tick: Instant,
    last_visual: Option<PromptVisual>,
    last_connected: bool,
    connected_visual_until: Option<Instant>,
}

impl Default for PromptOverlayController {
    fn default() -> Self {
        Self {
            thread: None,
            last_tick: crate::time_util::stale(TICK_INTERVAL),
            last_visual: None,
            last_connected: false,
            connected_visual_until: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptVisual {
    Ready,
    NoPad,
    Connected,
}

impl PromptVisual {
    fn as_lparam(self) -> LPARAM {
        LPARAM(match self {
            PromptVisual::Ready => 1,
            PromptVisual::NoPad => 2,
            PromptVisual::Connected => 3,
        })
    }

    fn from_lparam(lparam: LPARAM) -> Self {
        match lparam.0 {
            2 => PromptVisual::NoPad,
            3 => PromptVisual::Connected,
            _ => PromptVisual::Ready,
        }
    }
}

/// Prompt-overlay adapter for the shared UI-thread band.
struct PromptApp;

impl DesktopApp for PromptApp {
    const THREAD_NAME: &'static str = "warmup-prompt-overlay";
    const CLASS_NAME: windows::core::PCWSTR = WINDOW_CLASS;
    const BG_COLOR: u32 = DEFAULT_BG;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = prompt_wndproc;

    fn on_show(&mut self, lparam: LPARAM) {
        ui_show(PromptVisual::from_lparam(lparam));
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

thread_local! {
    static HWND_STATE: std::cell::Cell<Option<HWND>> = const { std::cell::Cell::new(None) };
    static RENDERER: RefCell<Option<VkRenderer>> = const { RefCell::new(None) };
    static VISUAL_STATE: std::cell::Cell<PromptVisual> = const { std::cell::Cell::new(PromptVisual::Ready) };
}

static CONTROLLER: OnceLock<Mutex<PromptOverlayController>> = OnceLock::new();

/// Drive the prompt from the gamepad loop thread. `vk_open` is the loop's
/// single source of truth (`app.vk_session.is_some()`), so no shared atomic is
/// needed: the prompt hides the instant the keyboard opens.
pub fn tick(vk_open: bool) {
    if !crate::config::service_mode() {
        return;
    }
    let controller = CONTROLLER.get_or_init(|| Mutex::new(PromptOverlayController::default()));
    let Ok(mut c) = controller.lock() else {
        return;
    };
    if c.last_tick.elapsed() < TICK_INTERVAL {
        return;
    }
    c.last_tick = Instant::now();

    let on_winlogon = super::surface::input().is_some_and(|s| s.is_winlogon());
    let connected = crate::debug_state::snapshot().connected;
    if connected && !c.last_connected {
        c.connected_visual_until = Some(Instant::now() + CONNECTED_VISUAL_DURATION);
    } else if !connected {
        c.connected_visual_until = None;
    }
    c.last_connected = connected;
    let connected_intro_active = c
        .connected_visual_until
        .is_some_and(|until| Instant::now() < until);
    let visual = if on_winlogon {
        if vk_open {
            None
        } else if connected_intro_active {
            Some(PromptVisual::Connected)
        } else if connected {
            Some(PromptVisual::Ready)
        } else {
            Some(PromptVisual::NoPad)
        }
    } else {
        None
    };

    // Keep the thread alive across desktops; toggle visibility only (tearing the
    // thread down per transition raced the next CreateWindow — see debug overlay).
    let just_spawned = if c.thread.is_none() {
        match desktop_window::spawn(PromptApp) {
            Ok(thread) => {
                c.thread = Some(thread);
                true
            }
            Err(e) => {
                service_log(&format!("prompt ui: spawn failed: {e}"));
                c.last_visual = visual;
                return;
            }
        }
    } else {
        false
    };

    if (just_spawned || visual != c.last_visual) && c.thread.is_some() {
        let thread = c.thread.as_ref().expect("checked is_some");
        if let Some(visual) = visual {
            let _ = thread.show(visual.as_lparam());
            service_log(match visual {
                PromptVisual::Connected => "prompt ui: shown (Winlogon, pad connected animation)",
                PromptVisual::Ready => "prompt ui: shown (Winlogon, VK closed, pad connected)",
                PromptVisual::NoPad => "prompt ui: shown (Winlogon, no pad connected)",
            });
        } else {
            let _ = thread.hide();
            service_log("prompt ui: hidden");
        }
    }
    c.last_visual = visual;
}

fn ui_show(visual: PromptVisual) {
    ui_hide();
    VISUAL_STATE.with(|state| state.set(visual));
    if let Err(e) = desktop::attach_input() {
        service_log(&format!("prompt ui: desktop attach failed: {e}"));
    }
    match unsafe { create_prompt_window() } {
        Ok(hwnd) => {
            HWND_STATE.with(|state| state.set(Some(hwnd)));
            unsafe {
                let (x, y) = bottom_center();
                let (w, h) = panel_size_for_visual(visual);
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    x,
                    y,
                    w,
                    h,
                    SWP_SHOWWINDOW | SWP_NOACTIVATE,
                );
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                match VkRenderer::create(hwnd) {
                    Ok(r) => {
                        RENDERER.with(|c| *c.borrow_mut() = Some(r));
                        service_log("prompt ui: D3D11/DComp renderer created");
                    }
                    Err(e) => service_log(&format!("prompt ui: renderer init failed: {e}")),
                }
                let _ = SetTimer(hwnd, REPAINT_TIMER_ID, REPAINT_TIMER_MS, None);
                render_prompt(hwnd);
            }
        }
        Err(e) => service_log(&format!("prompt ui: create window failed: {e}")),
    }
}

fn ui_hide() {
    let hwnd = HWND_STATE.with(|state| state.take());
    if let Some(hwnd) = hwnd {
        // Drop the renderer (releases the DComp target bound to this HWND) BEFORE
        // DestroyWindow, or releasing it against a dead HWND crashes.
        RENDERER.with(|c| *c.borrow_mut() = None);
        unsafe {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// Bottom-center of the primary monitor.
unsafe fn bottom_center() -> (i32, i32) {
    let cx = GetSystemMetrics(SM_CXSCREEN);
    let cy = GetSystemMetrics(SM_CYSCREEN);
    let (w, h) = VISUAL_STATE.with(|state| panel_size_for_visual(state.get()));
    let x = ((cx - w) / 2).max(0);
    let y = (cy - h - MARGIN_BOTTOM).max(0);
    (x, y)
}

fn panel_size_for_visual(visual: PromptVisual) -> (i32, i32) {
    match visual {
        PromptVisual::Connected => (CONNECTED_PANEL_W, CONNECTED_PANEL_H),
        PromptVisual::Ready | PromptVisual::NoPad => (PANEL_W, PANEL_H),
    }
}

unsafe fn create_prompt_window() -> Result<HWND, String> {
    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let (x, y) = bottom_center();
    CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
        WINDOW_CLASS,
        w!("Warmup Prompt Overlay"),
        WS_POPUP,
        x,
        y,
        PANEL_W,
        PANEL_H,
        None,
        HMENU::default(),
        windows::Win32::Foundation::HINSTANCE(instance.0),
        None,
    )
    .map_err(|e| format!("CreateWindowExW: {e}"))
}

unsafe extern "system" fn prompt_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            render_prompt(hwnd);
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == REPAINT_TIMER_ID {
                render_prompt(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            let _ = RENDERER.try_with(|c| {
                if let Ok(mut r) = c.try_borrow_mut() {
                    *r = None;
                }
            });
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

/// Render the pill through the shared D3D11/D2D/DComp renderer. Colors follow the
/// keyboard theme so the prompt matches the VK card; the L3 chip keeps its own.
fn render_prompt(hwnd: HWND) {
    let theme = crate::config::keyboard_theme();
    let bg = theme.bg.unwrap_or(DEFAULT_BG);
    let border = theme.border.or(theme.accent).unwrap_or(DEFAULT_BORDER);
    let text = theme.text.unwrap_or(DEFAULT_TEXT);
    let visual = VISUAL_STATE.with(|state| state.get());
    RENDERER.with(|c| {
        if let Ok(mut slot) = c.try_borrow_mut() {
            if let Some(r) = slot.as_mut() {
                unsafe {
                    if let Err(e) = r.resize(hwnd) {
                        service_log(&format!("prompt ui: renderer resize: {e}"));
                    }
                    let result = match visual {
                        PromptVisual::Connected => {
                            r.draw_connected_prompt(bg, border, text, "Controller", "Connected")
                        }
                        PromptVisual::Ready => {
                            r.draw_prompt(bg, border, text, PROMPT_PREFIX, PROMPT_SUFFIX, true)
                        }
                        PromptVisual::NoPad => {
                            let muted_text = crate::win::vk_renderer::mix_color(text, bg, 0.58);
                            let muted_border = crate::win::vk_renderer::mix_color(border, bg, 0.45);
                            r.draw_prompt(bg, muted_border, muted_text, NO_PAD_PROMPT, "", false)
                        }
                    };
                    if let Err(e) = result {
                        service_log(&format!("prompt ui: renderer draw: {e}"));
                    }
                }
            }
        }
    });
}

fn service_log(msg: &str) {
    if crate::config::service_mode() {
        crate::install::log_line(msg);
    }
}
