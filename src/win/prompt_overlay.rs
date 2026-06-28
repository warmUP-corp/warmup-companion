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
const CONNECTED_PANEL_W: i32 = 300;
const CONNECTED_PANEL_H: i32 = 210;
/// Gap between the pill's bottom edge and the bottom of the primary monitor.
const MARGIN_BOTTOM: i32 = 72;
/// Voice glow overlay: small + subtle, hugging the right edge, vertically centered.
const VOICE_W: i32 = 150;
const VOICE_H: i32 = 150;
const MARGIN_RIGHT: i32 = 40;
const REPAINT_TIMER_ID: usize = 12;
/// ~60 fps so the reactive voice glow animates smoothly.
const REPAINT_TIMER_MS: u32 = 16;
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const CONNECTED_VISUAL_DURATION: Duration = Duration::from_millis(2400);
const MORPH_DURATION: Duration = Duration::from_millis(420);

const PROMPT_PREFIX: &str = "Press ";
const PROMPT_SUFFIX: &str = " for keyboard";
const NO_PAD_PROMPT: &str = "Connect controller";

// COLORREF (0x00BBGGRR) fallbacks; matched to the VK card / debug panel look.
const DEFAULT_BG: u32 = 0x00141414;
const DEFAULT_BORDER: u32 = 0x00997b4c;
const DEFAULT_TEXT: u32 = 0x00FFFFFF;

struct PromptOverlayController {
    thread: Option<DesktopWindowThread>,
    thread_kind: Option<PromptThreadKind>,
    last_tick: Instant,
    last_visual: Option<PromptVisual>,
    last_connected: bool,
    connected_visual_until: Option<Instant>,
    connected_card_shown: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptThreadKind {
    Prompt,
    Voice,
}

#[derive(Clone, Copy, Debug)]
struct PromptRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, Debug)]
struct WindowMorph {
    from: PromptRect,
    to: PromptRect,
    start: Instant,
}

#[derive(Clone, Copy, Debug)]
struct VisualMorph {
    from: PromptVisual,
    to: PromptVisual,
    start: Instant,
}

impl Default for PromptOverlayController {
    fn default() -> Self {
        Self {
            thread: None,
            thread_kind: None,
            last_tick: crate::time_util::stale(TICK_INTERVAL),
            last_visual: None,
            last_connected: false,
            connected_visual_until: None,
            connected_card_shown: false,
        }
    }
}

impl PromptOverlayController {
    fn update_connected_visual(&mut self, connected: bool, now: Instant) {
        if connected && !self.last_connected && !self.connected_card_shown {
            self.connected_visual_until = Some(now + CONNECTED_VISUAL_DURATION);
            self.connected_card_shown = true;
        } else if !connected {
            self.connected_visual_until = None;
        }
        self.last_connected = connected;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptVisual {
    Ready,
    NoPad,
    Connected,
    /// Voice dictation is recording (mic on, VK closed).
    Listening,
    /// Voice dictation stopped; transcribing the recording.
    Transcribing,
}

impl PromptVisual {
    fn as_lparam(self) -> LPARAM {
        LPARAM(match self {
            PromptVisual::Ready => 1,
            PromptVisual::NoPad => 2,
            PromptVisual::Connected => 3,
            PromptVisual::Listening => 4,
            PromptVisual::Transcribing => 5,
        })
    }

    fn from_lparam(lparam: LPARAM) -> Self {
        match lparam.0 {
            2 => PromptVisual::NoPad,
            3 => PromptVisual::Connected,
            4 => PromptVisual::Listening,
            5 => PromptVisual::Transcribing,
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

    fn on_ready(&mut self, _thread_id: u32) {
        // Attach to the input desktop ONCE while the thread is clean (no windows),
        // so SetThreadDesktop can't later fail with ERROR_BUSY (0x800700AA). In a
        // userland session that's the user's Default desktop — where the voice pill
        // lives — so voice shows then need no (failing) per-show re-attach.
        if let Err(e) = desktop::attach_input() {
            service_log(&format!("prompt ui: initial desktop attach failed: {e}"));
        }
    }

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
    static WINDOW_RECT_STATE: std::cell::Cell<Option<PromptRect>> = const { std::cell::Cell::new(None) };
    static WINDOW_MORPH: std::cell::Cell<Option<WindowMorph>> = const { std::cell::Cell::new(None) };
    static VISUAL_MORPH: std::cell::Cell<Option<VisualMorph>> = const { std::cell::Cell::new(None) };
    /// Render-side smoothed mic level — lerps toward the helper's published value
    /// each (60 fps) repaint so the glow glides instead of stepping at the helper rate.
    static VOICE_LEVEL: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

fn smoothed_voice_level() -> f32 {
    let target = crate::win::speech_input::voice_level();
    VOICE_LEVEL.with(|c| {
        let v = c.get();
        let n = v + (target - v) * 0.35;
        c.set(n);
        n
    })
}

fn visual_is_voice(visual: PromptVisual) -> bool {
    matches!(visual, PromptVisual::Listening | PromptVisual::Transcribing)
}

fn thread_kind_for_visual(visual: PromptVisual) -> PromptThreadKind {
    if visual_is_voice(visual) {
        PromptThreadKind::Voice
    } else {
        PromptThreadKind::Prompt
    }
}

fn can_morph_between(from: PromptVisual, to: PromptVisual) -> bool {
    from != to
        && !visual_is_voice(from)
        && !visual_is_voice(to)
        && (matches!(from, PromptVisual::Connected) || matches!(to, PromptVisual::Connected))
}

fn raw_morph_progress(start: Instant, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / MORPH_DURATION.as_secs_f32())
        .clamp(0.0, 1.0)
}

fn eased_morph_progress(start: Instant, now: Instant) -> f32 {
    let t = raw_morph_progress(start, now);
    t * t * (3.0 - 2.0 * t)
}

fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t).round() as i32
}

fn lerp_rect(a: PromptRect, b: PromptRect, t: f32) -> PromptRect {
    PromptRect {
        x: lerp_i32(a.x, b.x, t),
        y: lerp_i32(a.y, b.y, t),
        w: lerp_i32(a.w, b.w, t).max(1),
        h: lerp_i32(a.h, b.h, t).max(1),
    }
}

fn active_visual_morph(now: Instant) -> Option<(PromptVisual, PromptVisual, f32)> {
    VISUAL_MORPH.with(|state| {
        let morph = state.get()?;
        let raw = raw_morph_progress(morph.start, now);
        if raw >= 1.0 {
            state.set(None);
            None
        } else {
            Some((morph.from, morph.to, eased_morph_progress(morph.start, now)))
        }
    })
}

unsafe fn target_rect_for_visual(visual: PromptVisual) -> PromptRect {
    let cx = GetSystemMetrics(SM_CXSCREEN);
    let cy = GetSystemMetrics(SM_CYSCREEN);
    let (w, h) = panel_size_for_visual(visual);
    let (x, y) = if visual_is_voice(visual) {
        ((cx - w - MARGIN_RIGHT).max(0), ((cy - h) / 2).max(0))
    } else {
        (((cx - w) / 2).max(0), (cy - h - MARGIN_BOTTOM).max(0))
    };
    PromptRect { x, y, w, h }
}

unsafe fn tick_window_morph(hwnd: HWND, now: Instant) {
    WINDOW_MORPH.with(|state| {
        let Some(morph) = state.get() else {
            return;
        };
        let raw = raw_morph_progress(morph.start, now);
        let rect = if raw >= 1.0 {
            state.set(None);
            morph.to
        } else {
            lerp_rect(morph.from, morph.to, eased_morph_progress(morph.start, now))
        };
        WINDOW_RECT_STATE.with(|s| s.set(Some(rect)));
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
    });
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
    let now = Instant::now();
    c.last_tick = now;

    let userland_debug = crate::config::prompt_userland_debug();
    let on_winlogon = super::surface::input().is_some_and(|s| s.is_winlogon());
    let connected = crate::debug_state::snapshot().connected;
    c.update_connected_visual(connected, now);
    let connected_intro_active = c.connected_visual_until.is_some_and(|until| now < until);
    // Voice dictation takes priority on ANY desktop: R3 can start it with the VK
    // closed, so this pill is the "currently listening" indicator. While the VK is
    // open its own mic-key halo shows the phase, so the pill yields then.
    let voice = crate::win::speech_input::voice_ui_phase();
    let visual = if let Some(p) = voice.as_deref() {
        if vk_open {
            None
        } else if p == "transcribing" {
            Some(PromptVisual::Transcribing)
        } else {
            Some(PromptVisual::Listening)
        }
    } else if userland_debug {
        Some(PromptVisual::Connected)
    } else if on_winlogon {
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

    let desired_thread_kind = visual.map(thread_kind_for_visual);
    if c.thread.is_some() && desired_thread_kind.is_some() && c.thread_kind != desired_thread_kind {
        if let Some(thread) = c.thread.take() {
            let _ = thread.hide();
        }
        c.thread_kind = None;
    }

    // Keep the thread alive while it belongs to the same desktop class; voice
    // lives on Default, while the prompt/card lives on the current input desktop.
    let just_spawned = if c.thread.is_none() && desired_thread_kind.is_some() {
        match desktop_window::spawn(PromptApp) {
            Ok(thread) => {
                c.thread = Some(thread);
                c.thread_kind = desired_thread_kind;
                true
            }
            Err(e) => {
                service_log(&format!("prompt ui: spawn failed: {e}"));
                c.last_visual = visual;
                c.thread_kind = None;
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
            service_log(match (visual, userland_debug) {
                (PromptVisual::Connected, true) => {
                    "prompt ui: shown (userland debug, connected animation)"
                }
                (PromptVisual::Connected, false) => {
                    "prompt ui: shown (Winlogon, pad connected animation)"
                }
                (PromptVisual::Ready, _) => "prompt ui: shown (Winlogon, VK closed, pad connected)",
                (PromptVisual::NoPad, _) => "prompt ui: shown (Winlogon, no pad connected)",
                (PromptVisual::Listening, _) => "prompt ui: shown (voice listening)",
                (PromptVisual::Transcribing, _) => "prompt ui: shown (voice transcribing)",
            });
        } else {
            let _ = thread.hide();
            service_log("prompt ui: hidden");
        }
    }
    c.last_visual = visual;
}

fn ui_show(visual: PromptVisual) {
    let previous = VISUAL_STATE.with(|state| state.get());
    let existing = HWND_STATE.with(|state| state.get());
    if existing.is_some() && !can_morph_between(previous, visual) && previous != visual {
        ui_hide();
    }

    VISUAL_STATE.with(|state| state.set(visual));
    let target = unsafe { target_rect_for_visual(visual) };
    let now = Instant::now();
    if let Some(hwnd) = HWND_STATE.with(|state| state.get()) {
        let from = WINDOW_RECT_STATE
            .with(|state| state.get())
            .unwrap_or_else(|| unsafe { target_rect_for_visual(previous) });
        if previous != visual {
            WINDOW_MORPH.with(|state| {
                state.set(Some(WindowMorph {
                    from,
                    to: target,
                    start: now,
                }))
            });
            VISUAL_MORPH.with(|state| {
                state.set(if can_morph_between(previous, visual) {
                    Some(VisualMorph {
                        from: previous,
                        to: visual,
                        start: now,
                    })
                } else {
                    None
                })
            });
        }
        unsafe {
            tick_window_morph(hwnd, now);
            render_prompt(hwnd);
        }
        return;
    }

    WINDOW_RECT_STATE.with(|state| state.set(Some(target)));
    WINDOW_MORPH.with(|state| state.set(None));
    VISUAL_MORPH.with(|state| state.set(None));
    // Voice pill is userland-only and the thread is already on the user desktop
    // (attached once in on_ready); re-attaching there fails with ERROR_BUSY and
    // mis-places the window. Only the Winlogon prompts re-attach per show.
    let userland_only = matches!(visual, PromptVisual::Listening | PromptVisual::Transcribing);
    if !userland_only {
        if let Err(e) = desktop::attach_input() {
            service_log(&format!("prompt ui: desktop attach failed: {e}"));
        }
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
        WINDOW_RECT_STATE.with(|state| state.set(None));
        WINDOW_MORPH.with(|state| state.set(None));
        VISUAL_MORPH.with(|state| state.set(None));
        unsafe {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// Bottom-center of the primary monitor.
unsafe fn bottom_center() -> (i32, i32) {
    let visual = VISUAL_STATE.with(|state| state.get());
    let rect = target_rect_for_visual(visual);
    (rect.x, rect.y)
}

fn panel_size_for_visual(visual: PromptVisual) -> (i32, i32) {
    match visual {
        PromptVisual::Connected => (CONNECTED_PANEL_W, CONNECTED_PANEL_H),
        PromptVisual::Listening | PromptVisual::Transcribing => (VOICE_W, VOICE_H),
        PromptVisual::Ready | PromptVisual::NoPad => (PANEL_W, PANEL_H),
    }
}

unsafe fn create_prompt_window() -> Result<HWND, String> {
    let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let rect = target_rect_for_visual(VISUAL_STATE.with(|state| state.get()));
    CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
        WINDOW_CLASS,
        w!("Warmup Prompt Overlay"),
        WS_POPUP,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
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

/// User-facing title for the connection card. Prefers the real device name;
/// backend slot labels ("XInput slot 0", "HID slot 0") aren't presentable, so
/// they collapse to a controller family instead. Keep the keyword families in
/// sync with `ControllerArt::from_label` so the title and artwork agree.
fn connected_card_title(label: &str) -> String {
    let l = label.trim();
    let low = l.to_ascii_lowercase();
    if l.is_empty() || low == "none" {
        "Controller".to_string()
    } else if low.contains("slot") {
        // Winlogon reads PlayStation pads via direct HID and XInput pads via XUSB.
        if low.contains("hid") {
            "PlayStation Controller".to_string()
        } else {
            "Xbox Controller".to_string()
        }
    } else {
        l.to_string()
    }
}

/// Render the pill through the shared D3D11/D2D/DComp renderer. Colors follow the
/// keyboard theme so the prompt matches the VK card; the L3 chip keeps its own.
fn render_prompt(hwnd: HWND) {
    let now = Instant::now();
    unsafe {
        tick_window_morph(hwnd, now);
    }
    let theme = crate::config::keyboard_theme();
    let bg = theme.bg.unwrap_or(DEFAULT_BG);
    let border = theme.border.or(theme.accent).unwrap_or(DEFAULT_BORDER);
    let text = theme.text.unwrap_or(DEFAULT_TEXT);
    let visual = VISUAL_STATE.with(|state| state.get());
    let visual_morph = active_visual_morph(now);
    let snapshot = crate::debug_state::snapshot();
    RENDERER.with(|c| {
        if let Ok(mut slot) = c.try_borrow_mut() {
            if let Some(r) = slot.as_mut() {
                unsafe {
                    if let Err(e) = r.resize(hwnd) {
                        service_log(&format!("prompt ui: renderer resize: {e}"));
                    }
                    // The live device name (e.g. "DualSense Wireless Controller")
                    // drives both the card title and the controller artwork. Fall
                    // back to a generic pad name only when the backend hasn't
                    // published one yet (no service-mode publish, mid-connect, ...).
                    let name = snapshot.name.trim();
                    let controller_label = if name.is_empty() || name.eq_ignore_ascii_case("none") {
                        "Xbox Wireless Controller"
                    } else {
                        name
                    };
                    let title = connected_card_title(controller_label);
                    let result = if let Some((from, to, t)) = visual_morph {
                        if matches!(from, PromptVisual::Connected)
                            || matches!(to, PromptVisual::Connected)
                        {
                            let card_t = if matches!(to, PromptVisual::Connected) {
                                t
                            } else {
                                1.0 - t
                            };
                            r.draw_prompt_card_morph(
                                bg,
                                border,
                                text,
                                PROMPT_PREFIX,
                                PROMPT_SUFFIX,
                                true,
                                &title,
                                controller_label,
                                card_t,
                            )
                        } else {
                            r.draw_prompt(
                                bg,
                                border,
                                text,
                                PROMPT_PREFIX,
                                PROMPT_SUFFIX,
                                true,
                                snapshot.name.trim(),
                            )
                        }
                    } else {
                        match visual {
                            PromptVisual::Connected => {
                                r.draw_connected_prompt(bg, border, text, &title, controller_label)
                            }
                            PromptVisual::Ready => r.draw_prompt(
                                bg,
                                border,
                                text,
                                PROMPT_PREFIX,
                                PROMPT_SUFFIX,
                                true,
                                snapshot.name.trim(),
                            ),
                            PromptVisual::NoPad => {
                                let muted_text = crate::win::vk_renderer::mix_color(text, bg, 0.58);
                                let muted_border =
                                    crate::win::vk_renderer::mix_color(border, bg, 0.45);
                                r.draw_prompt(
                                    bg,
                                    muted_border,
                                    muted_text,
                                    NO_PAD_PROMPT,
                                    "",
                                    false,
                                    "",
                                )
                            }
                            PromptVisual::Listening => {
                                let accent =
                                    theme.accent.or(theme.border).unwrap_or(DEFAULT_BORDER);
                                r.draw_voice(accent, smoothed_voice_level(), false)
                            }
                            PromptVisual::Transcribing => {
                                let accent =
                                    theme.accent.or(theme.border).unwrap_or(DEFAULT_BORDER);
                                r.draw_voice(accent, smoothed_voice_level(), true)
                            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_card_is_one_shot() {
        let mut controller = PromptOverlayController::default();
        let now = Instant::now();

        controller.update_connected_visual(true, now);
        assert!(controller.connected_visual_until.is_some());

        controller.update_connected_visual(false, now + CONNECTED_VISUAL_DURATION);
        assert!(controller.connected_visual_until.is_none());

        controller.update_connected_visual(true, now + CONNECTED_VISUAL_DURATION * 2);
        assert!(controller.connected_visual_until.is_none());
    }

    #[test]
    fn morph_progress_eases_without_overshoot() {
        let start = Instant::now();

        assert_eq!(eased_morph_progress(start, start), 0.0);
        assert!(
            (eased_morph_progress(start, start + Duration::from_millis(210)) - 0.5).abs() < 0.01
        );
        assert_eq!(eased_morph_progress(start, start + MORPH_DURATION * 2), 1.0);
    }

    #[test]
    fn voice_uses_separate_overlay_thread_kind() {
        assert_eq!(
            thread_kind_for_visual(PromptVisual::Listening),
            PromptThreadKind::Voice
        );
        assert_eq!(
            thread_kind_for_visual(PromptVisual::Connected),
            PromptThreadKind::Prompt
        );
        assert_eq!(
            thread_kind_for_visual(PromptVisual::Ready),
            PromptThreadKind::Prompt
        );
    }
}
