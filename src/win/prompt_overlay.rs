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
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, GetWindowLongPtrW, KillTimer,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HMENU, HTTRANSPARENT,
    HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_NCHITTEST, WM_PAINT, WM_TIMER, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::desktop;
use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::vk_renderer::{self, VkRenderer};

const WINDOW_CLASS: windows::core::PCWSTR = w!("WarmupPromptOverlayWindow");

/// One canvas for the prompt pill AND the connection card: the pill sits in the
/// bottom `vk_renderer::PROMPT_PILL_H` band and the card grows upward out of it,
/// so the morph never moves or resizes the window (that is what made it drift).
const PANEL_W: i32 = 720;
const PANEL_H: i32 = 420;
/// Gap between the pill's bottom edge and the bottom of the primary monitor.
const MARGIN_BOTTOM: i32 = 72;
/// Dictation pill: orb + phase title + R3 stop hint, hugging the right edge,
/// vertically centered. Wide enough for "Listening" at the 28px prompt font.
/// Transcription leaves this pill: the window grows to the whole display.
const VOICE_W: i32 = 420;
const VOICE_H: i32 = 104;
const MARGIN_RIGHT: i32 = 40;
const REPAINT_TIMER_ID: usize = 12;
/// ~60 fps so the reactive voice glow animates smoothly.
const REPAINT_TIMER_MS: u32 = 16;
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const CONNECTED_VISUAL_DURATION: Duration = Duration::from_millis(2400);
const MORPH_DURATION: Duration = Duration::from_millis(420);
/// Userland debug replays the Winlogon connect sequence on a loop so the morph
/// can be watched on the normal desktop: no pad → card → ready prompt.
const DEBUG_REPLAY_PERIOD: Duration = Duration::from_millis(6500);
const DEBUG_REPLAY_CONNECT_AT: Duration = Duration::from_millis(1500);

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
    debug_epoch: Instant,
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
            debug_epoch: Instant::now(),
        }
    }
}

/// Visual for the userland debug replay at `elapsed` since the loop started.
fn debug_replay_visual(elapsed: Duration) -> PromptVisual {
    let phase = Duration::from_nanos((elapsed.as_nanos() % DEBUG_REPLAY_PERIOD.as_nanos()) as u64);
    if phase < DEBUG_REPLAY_CONNECT_AT {
        PromptVisual::NoPad
    } else if phase < DEBUG_REPLAY_CONNECT_AT + CONNECTED_VISUAL_DURATION {
        PromptVisual::Connected
    } else {
        PromptVisual::Ready
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
    /// Mic is open. Same fullscreen frame as transcription; the cloud uses the
    /// speaking state.
    Listening,
    /// Recording stopped. Same frame; the cloud uses the thinking state.
    Transcribing,
    /// Keyboard is open during any voice phase: the same fullscreen border, and
    /// the cloud stays on the mic key at the same size.
    VoiceBorder,
    /// Voice helper launched but the mic isn't capturing yet.
    Starting,
}

impl PromptVisual {
    fn as_lparam(self) -> LPARAM {
        LPARAM(match self {
            PromptVisual::Ready => 1,
            PromptVisual::NoPad => 2,
            PromptVisual::Connected => 3,
            PromptVisual::Listening => 4,
            PromptVisual::Transcribing => 5,
            PromptVisual::Starting => 6,
            PromptVisual::VoiceBorder => 7,
        })
    }

    fn from_lparam(lparam: LPARAM) -> Self {
        match lparam.0 {
            2 => PromptVisual::NoPad,
            3 => PromptVisual::Connected,
            4 => PromptVisual::Listening,
            5 => PromptVisual::Transcribing,
            6 => PromptVisual::Starting,
            7 => PromptVisual::VoiceBorder,
            _ => PromptVisual::Ready,
        }
    }

    fn voice_phase(self) -> Option<vk_renderer::VoicePhase> {
        match self {
            PromptVisual::Starting => Some(vk_renderer::VoicePhase::Starting),
            PromptVisual::Listening | PromptVisual::VoiceBorder => {
                Some(vk_renderer::VoicePhase::Listening)
            }
            PromptVisual::Transcribing => Some(vk_renderer::VoicePhase::Transcribing),
            _ => None,
        }
    }

    /// Helper phase string (`speech_input::voice_ui_phase`) -> pill visual.
    fn from_voice_phase(phase: &str) -> Self {
        match phase {
            "transcribing" => PromptVisual::Transcribing,
            "starting" => PromptVisual::Starting,
            _ => PromptVisual::Listening,
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
    static VISUAL_MORPH: std::cell::Cell<Option<VisualMorph>> = const { std::cell::Cell::new(None) };
    /// Dictation pill clocks: when it appeared (entrance), when its phase title
    /// last changed (label fade), and when its exit fade began (`ui_hide`).
    static VOICE_SHOWN_AT: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    static VOICE_LABEL_AT: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    static VOICE_EXIT_AT: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// `(alpha, scale, label_alpha)` for the dictation pill at `now`: entrance
/// fade/scale, times the exit fade once one has started, plus the phase-label fade.
fn voice_transition(now: Instant) -> (f32, f32, f32) {
    let ms =
        |at: Option<Instant>| at.map(|t| now.saturating_duration_since(t).as_secs_f32() * 1000.0);
    let elapsed = ms(VOICE_SHOWN_AT.with(|c| c.get()));
    let mut alpha = elapsed
        .map(crate::vk_motion::voice_frame_enter)
        .unwrap_or(1.0);
    let scale = elapsed
        .map(|ms| crate::vk_motion::voice_enter(ms).1)
        .unwrap_or(1.0);
    if let Some(exit_ms) = ms(VOICE_EXIT_AT.with(|c| c.get())) {
        alpha *= crate::vk_motion::voice_exit(exit_ms);
    }
    let label_alpha = ms(VOICE_LABEL_AT.with(|c| c.get()))
        .map(crate::vk_motion::voice_label_fade)
        .unwrap_or(1.0);
    (alpha, scale, label_alpha)
}

fn visual_is_voice(visual: PromptVisual) -> bool {
    visual.voice_phase().is_some()
}

/// Starting -> Listening -> Transcribing all live in the one dictation pill: keep
/// the window and swap the title, instead of tearing the pill down and back up.
fn same_voice_pill(from: PromptVisual, to: PromptVisual) -> bool {
    visual_is_voice(from) && visual_is_voice(to)
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

fn fullscreen_border(visual: PromptVisual) -> bool {
    visual_is_voice(visual)
}

/// Where the overlay sits. Transcription covers the primary display so the
/// border can run along its edges; everything else stays a small panel.
fn overlay_rect(visual: PromptVisual, screen_w: i32, screen_h: i32) -> PromptRect {
    let screen_w = screen_w.max(1);
    let screen_h = screen_h.max(1);
    if fullscreen_border(visual) {
        return PromptRect {
            x: 0,
            y: 0,
            w: screen_w,
            h: screen_h,
        };
    }
    let (w, h) = panel_size_for_visual(visual);
    let (x, y) = if visual_is_voice(visual) {
        (
            (screen_w - w - MARGIN_RIGHT).max(0),
            ((screen_h - h) / 2).max(0),
        )
    } else {
        (
            ((screen_w - w) / 2).max(0),
            (screen_h - h - MARGIN_BOTTOM).max(0),
        )
    };
    PromptRect { x, y, w, h }
}

unsafe fn target_rect_for_visual(visual: PromptVisual) -> PromptRect {
    overlay_rect(
        visual,
        GetSystemMetrics(SM_CXSCREEN),
        GetSystemMetrics(SM_CYSCREEN),
    )
}

/// Move the overlay to `visual`'s rect. Transcription is click-through: the
/// window covers the display, and a hit would otherwise swallow the desktop
/// and the keyboard under the border.
unsafe fn place_overlay(hwnd: HWND, visual: PromptVisual, show: bool) {
    let rect = target_rect_for_visual(visual);
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let bit = WS_EX_TRANSPARENT.0;
    let ex = if fullscreen_border(visual) {
        ex | bit
    } else {
        ex & !bit
    };
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
    let mut flags = SWP_NOACTIVATE | SWP_FRAMECHANGED;
    if show {
        flags |= SWP_SHOWWINDOW;
    }
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, rect.x, rect.y, rect.w, rect.h, flags);
}

/// Talking and transcription share one fullscreen frame. With the keyboard open
/// the cloud stays on the mic key, so the overlay is only the border.
fn voice_overlay_visual(phase: Option<&str>, vk_open: bool) -> Option<PromptVisual> {
    let phase = phase?;
    if vk_open {
        Some(PromptVisual::VoiceBorder)
    } else {
        Some(PromptVisual::from_voice_phase(phase))
    }
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
    let on_winlogon = super::surface::input().is_some_and(|s| s.is_winlogon())
        && !super::native_keyboard::yield_logon_to_native();
    let connected = crate::debug_state::snapshot().connected;
    c.update_connected_visual(connected, now);
    let connected_intro_active = c.connected_visual_until.is_some_and(|until| now < until);
    // Voice dictation takes priority on ANY desktop. While the VK is open its
    // mic key shows listening/starting, so the pill yields — transcription
    // still takes the screen, because that's the full-display border.
    let voice = crate::win::speech_input::voice_ui_phase();
    let visual = if voice.is_some() {
        voice_overlay_visual(voice.as_deref(), vk_open)
    } else if userland_debug {
        Some(debug_replay_visual(now.duration_since(c.debug_epoch)))
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
                (PromptVisual::VoiceBorder, _) => "prompt ui: shown (voice, display border)",
                (PromptVisual::Starting, _) => "prompt ui: shown (voice starting)",
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
    // Prompt visuals (ready / no pad / card) share one canvas and swap or morph in
    // place; only a voice <-> prompt change needs a different window.
    if existing.is_some() && visual_is_voice(previous) != visual_is_voice(visual) {
        ui_hide();
    }

    VISUAL_STATE.with(|state| state.set(visual));
    let now = Instant::now();
    if let Some(hwnd) = HWND_STATE.with(|state| state.get()) {
        // Same window, new visual: either a voice phase swap or a pill ⇄ card
        // morph. Both keep the window where it is and animate on the surface.
        if previous != visual && same_voice_pill(previous, visual) {
            VOICE_LABEL_AT.with(|c| c.set(Some(now)));
        }
        // Listening -> transcription grows the pill to the display. Hide across
        // the resize: showing first stretches the old pill over the whole screen
        // for a frame (the "flash"), and a failed swapchain resize then leaves
        // that frame up.
        if fullscreen_border(previous) != fullscreen_border(visual) {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
                place_overlay(hwnd, visual, false);
                render_prompt(hwnd);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
        if can_morph_between(previous, visual) {
            VISUAL_MORPH.with(|state| {
                state.set(Some(VisualMorph {
                    from: previous,
                    to: visual,
                    start: now,
                }))
            });
        }
        render_prompt(hwnd);
        return;
    }

    VISUAL_MORPH.with(|state| state.set(None));
    // The entrance clock starts at the first paint, not at window creation.
    // Creating the device takes long enough that a clock started here would
    // already be finished, and the frame would pop in.
    VOICE_LABEL_AT.with(|c| c.set(None));
    VOICE_EXIT_AT.with(|c| c.set(None));
    // Voice pill is userland-only and the thread is already on the user desktop
    // (attached once in on_ready); re-attaching there fails with ERROR_BUSY and
    // mis-places the window. Only the Winlogon prompts re-attach per show.
    let userland_only = visual_is_voice(visual);
    if !userland_only {
        if let Err(e) = desktop::attach_input() {
            service_log(&format!("prompt ui: desktop attach failed: {e}"));
        }
    }
    match unsafe { create_prompt_window() } {
        Ok(hwnd) => {
            HWND_STATE.with(|state| state.set(Some(hwnd)));
            unsafe {
                place_overlay(hwnd, visual, false);
                match VkRenderer::create(hwnd) {
                    Ok(mut r) => {
                        // Decode the card artwork now so the first card frame
                        // doesn't stall the morph mid-expand.
                        if !userland_only {
                            let name = crate::debug_state::snapshot().name;
                            let label = if name.trim().is_empty() {
                                "Xbox Wireless Controller".to_string()
                            } else {
                                name
                            };
                            if let Err(e) = r.preload_controller_art(&label) {
                                service_log(&format!("prompt ui: art preload: {e}"));
                            }
                        }
                        RENDERER.with(|c| *c.borrow_mut() = Some(r));
                        service_log("prompt ui: D3D11/DComp renderer created");
                    }
                    Err(e) => service_log(&format!("prompt ui: renderer init failed: {e}")),
                }
                let _ = SetTimer(hwnd, REPAINT_TIMER_ID, REPAINT_TIMER_MS, None);
                if visual_is_voice(visual) {
                    VOICE_SHOWN_AT.with(|c| c.set(Some(Instant::now())));
                }
                render_prompt(hwnd);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
        Err(e) => service_log(&format!("prompt ui: create window failed: {e}")),
    }
}

fn ui_hide() {
    let hwnd = HWND_STATE.with(|state| state.get());
    if let Some(hwnd) = hwnd {
        if visual_is_voice(VISUAL_STATE.with(|s| s.get())) {
            unsafe { fade_out_voice(hwnd) };
        }
    }
    let hwnd = HWND_STATE.with(|state| state.take());
    if let Some(hwnd) = hwnd {
        // Drop the renderer (releases the DComp target bound to this HWND) BEFORE
        // DestroyWindow, or releasing it against a dead HWND crashes.
        RENDERER.with(|c| *c.borrow_mut() = None);
        VISUAL_MORPH.with(|state| state.set(None));
        VOICE_SHOWN_AT.with(|c| c.set(None));
        VOICE_LABEL_AT.with(|c| c.set(None));
        VOICE_EXIT_AT.with(|c| c.set(None));
        unsafe {
            let _ = KillTimer(hwnd, REPAINT_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// Fade the dictation pill out over `VOICE_EXIT_MS` before it is destroyed, so
/// dictation ending (auto-stop, R3, or the keyboard opening) doesn't just blink
/// the pill away. Steps at the repaint cadence on this UI thread, like the
/// keyboard's own slide-out.
unsafe fn fade_out_voice(hwnd: HWND) {
    let started = Instant::now();
    VOICE_EXIT_AT.with(|c| c.set(Some(started)));
    let dur = Duration::from_millis(crate::vk_motion::VOICE_EXIT_MS as u64);
    loop {
        render_prompt(hwnd);
        if started.elapsed() >= dur {
            break;
        }
        std::thread::sleep(Duration::from_millis(REPAINT_TIMER_MS as u64));
    }
}

fn panel_size_for_visual(visual: PromptVisual) -> (i32, i32) {
    if visual_is_voice(visual) {
        (VOICE_W, VOICE_H)
    } else {
        (PANEL_W, PANEL_H)
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
        // Fullscreen transcription frame must not eat clicks meant for the
        // desktop or the keyboard underneath the border.
        WM_NCHITTEST if fullscreen_border(VISUAL_STATE.with(|state| state.get())) => {
            LRESULT(HTTRANSPARENT as isize)
        }
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
    let theme = crate::config::keyboard_theme();
    let bg = theme.bg.unwrap_or(DEFAULT_BG);
    let border = theme.border.or(theme.accent).unwrap_or(DEFAULT_BORDER);
    let text = theme.text.unwrap_or(DEFAULT_TEXT);
    let visual = VISUAL_STATE.with(|state| state.get());
    let snapshot = crate::debug_state::snapshot();
    // Pill ⇄ card blend, and which pill (ready / no pad) sits under the card.
    let (card_t, pill_visual) = match active_visual_morph(now) {
        Some((from, PromptVisual::Connected, t)) => (t, from),
        Some((PromptVisual::Connected, to, t)) => (1.0 - t, to),
        _ if visual == PromptVisual::Connected => (1.0, PromptVisual::Ready),
        _ => (0.0, visual),
    };
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
                    let result = if visual_is_voice(visual) {
                        let accent = theme.accent.or(theme.border).unwrap_or(DEFAULT_BORDER);
                        let (alpha, scale, label_alpha) = voice_transition(now);
                        r.draw_voice(&vk_renderer::VoicePill {
                            bg,
                            border,
                            accent,
                            text,
                            level: crate::win::speech_input::voice_level(),
                            phase: visual
                                .voice_phase()
                                .unwrap_or(vk_renderer::VoicePhase::Listening),
                            controller_label: name,
                            alpha,
                            scale,
                            label_alpha,
                            // Keyboard-open transcription keeps the cloud on the mic key.
                            show_orb: visual != PromptVisual::VoiceBorder,
                        })
                    } else if pill_visual == PromptVisual::NoPad {
                        r.draw_prompt_card(&vk_renderer::PromptCard {
                            bg,
                            border,
                            text_color: text,
                            pill_border: vk_renderer::mix_color(border, bg, 0.45),
                            pill_text: vk_renderer::mix_color(text, bg, 0.58),
                            prefix: NO_PAD_PROMPT,
                            suffix: "",
                            show_l3: false,
                            title: &title,
                            controller_label,
                            card_t,
                        })
                    } else {
                        r.draw_prompt_card(&vk_renderer::PromptCard {
                            bg,
                            border,
                            text_color: text,
                            pill_border: border,
                            pill_text: text,
                            prefix: PROMPT_PREFIX,
                            suffix: PROMPT_SUFFIX,
                            show_l3: true,
                            title: &title,
                            controller_label,
                            card_t,
                        })
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
    fn debug_replay_walks_no_pad_card_ready() {
        assert_eq!(debug_replay_visual(Duration::ZERO), PromptVisual::NoPad);
        assert_eq!(
            debug_replay_visual(DEBUG_REPLAY_CONNECT_AT),
            PromptVisual::Connected
        );
        assert_eq!(
            debug_replay_visual(DEBUG_REPLAY_CONNECT_AT + CONNECTED_VISUAL_DURATION),
            PromptVisual::Ready
        );
        assert_eq!(
            debug_replay_visual(DEBUG_REPLAY_PERIOD),
            PromptVisual::NoPad
        );
        assert!(can_morph_between(
            PromptVisual::NoPad,
            PromptVisual::Connected
        ));
        assert!(can_morph_between(
            PromptVisual::Connected,
            PromptVisual::Ready
        ));
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
    fn voice_phases_share_one_pill_and_never_morph_into_prompts() {
        assert_eq!(
            PromptVisual::from_voice_phase("starting"),
            PromptVisual::Starting
        );
        assert_eq!(
            PromptVisual::from_voice_phase("listening"),
            PromptVisual::Listening
        );
        assert_eq!(
            PromptVisual::from_voice_phase("transcribing"),
            PromptVisual::Transcribing
        );
        // Unknown/empty phase strings default to Listening, never to a prompt.
        assert!(visual_is_voice(PromptVisual::from_voice_phase("")));

        assert!(same_voice_pill(
            PromptVisual::Starting,
            PromptVisual::Listening
        ));
        assert!(same_voice_pill(
            PromptVisual::Listening,
            PromptVisual::Transcribing
        ));
        assert!(!same_voice_pill(
            PromptVisual::Listening,
            PromptVisual::Ready
        ));
        assert!(!can_morph_between(
            PromptVisual::Listening,
            PromptVisual::Connected
        ));
        assert_eq!(
            PromptVisual::Starting.voice_phase(),
            Some(vk_renderer::VoicePhase::Starting)
        );
        assert_eq!(PromptVisual::Ready.voice_phase(), None);
    }

    #[test]
    fn voice_uses_separate_overlay_thread_kind() {
        assert_eq!(
            thread_kind_for_visual(PromptVisual::Listening),
            PromptThreadKind::Voice
        );
        assert_eq!(
            thread_kind_for_visual(PromptVisual::Starting),
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

    #[test]
    fn talking_and_transcription_share_the_fullscreen_frame() {
        for visual in [
            PromptVisual::Listening,
            PromptVisual::Transcribing,
            PromptVisual::Starting,
            PromptVisual::VoiceBorder,
        ] {
            let rect = overlay_rect(visual, 1920, 1080);
            assert_eq!((rect.x, rect.y, rect.w, rect.h), (0, 0, 1920, 1080));
            assert!(fullscreen_border(visual));
        }
    }

    #[test]
    fn voice_border_while_the_keyboard_is_open_keeps_the_same_frame() {
        assert_eq!(
            voice_overlay_visual(Some("listening"), false),
            Some(PromptVisual::Listening)
        );
        assert_eq!(
            voice_overlay_visual(Some("transcribing"), false),
            Some(PromptVisual::Transcribing)
        );
        assert_eq!(
            voice_overlay_visual(Some("listening"), true),
            Some(PromptVisual::VoiceBorder)
        );
        assert_eq!(
            voice_overlay_visual(Some("transcribing"), true),
            Some(PromptVisual::VoiceBorder)
        );
        assert_eq!(voice_overlay_visual(None, false), None);
        assert!(same_voice_pill(
            PromptVisual::Listening,
            PromptVisual::Transcribing
        ));
        assert!(same_voice_pill(
            PromptVisual::Transcribing,
            PromptVisual::VoiceBorder
        ));
    }
}
