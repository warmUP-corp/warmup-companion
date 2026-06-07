//! Gamepad: PC cursor when VK closed; full keyboard control when VK open.
//!
//! Service mode uses XInput (secure desktop). Desktop `--gamepad` uses SDL3.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static RUNNING: AtomicBool = AtomicBool::new(true);

use crate::gamepad_backend::{Button, ButtonChange, GamepadBackend, SdlBackend, SdlThreadBackend};
use crate::pc_cursor::PcCursor;

#[cfg(windows)]
use crate::xinput_backend::XInputBackend;

/// Left stick click (L3) — toggles VK open/closed.
const VK_BUTTON: Button = Button::L3;

const POLL_INTERVAL: Duration = Duration::from_millis(8);
const WARMUP_LAUNCH_DEBOUNCE: Duration = Duration::from_secs(2);
/// Ignore spurious X/dpad from misaligned HID for a moment after VK opens.
const VK_NAV_INPUT_GRACE: Duration = Duration::from_millis(450);
const DESKTOP_SYNC_LOG_INTERVAL: Duration = Duration::from_secs(120);
const HAPTIC_CONFIRM_MS: u32 = 14;
const HAPTIC_ALERT_MS: u32 = 45;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkLoopAction {
    Toggle,
    Close,
    Reopen,
    LaunchWarmup,
}

enum Backend {
    /// Boxed so it can hold either the on-thread `SdlBackend` (interactive
    /// `--gamepad`) or the off-thread `SdlThreadBackend` (service worker, where
    /// the loop thread must own no window so it can `SetThreadDesktop`).
    Sdl(Box<dyn GamepadBackend>),
    #[cfg(windows)]
    XInput(XInputBackend),
}

/// One canonical frame from whichever backend is active: button edges + axes.
/// Lets the caller poll one interface instead of matching the enum per field.
struct PadFrame {
    changes: Vec<ButtonChange>,
    axes: (f32, f32, f32, f32),
    touchpad: crate::gamepad_backend::TouchpadFrame,
}

impl Backend {
    /// Poll the active backend and read back its edges + axes in one shot.
    /// The enum match lives here, not at the call site.
    fn poll_frame(&mut self) -> Result<PadFrame, String> {
        match self {
            Backend::Sdl(b) => {
                b.poll()?;
                Ok(PadFrame {
                    changes: b.button_changes(),
                    axes: b.axes(),
                    touchpad: b.touchpad(),
                })
            }
            #[cfg(windows)]
            Backend::XInput(b) => {
                b.poll()?;
                Ok(PadFrame {
                    changes: b.button_changes(),
                    axes: b.axes(),
                    touchpad: b.touchpad(),
                })
            }
        }
    }

    fn controller_label(&self) -> String {
        match self {
            Backend::Sdl(b) => b.controller_label(),
            #[cfg(windows)]
            Backend::XInput(b) => b.controller_label(),
        }
    }

    /// Short backend tag for diagnostics (`SDL3` / `XInput`).
    fn kind_label(&self) -> &'static str {
        match self {
            Backend::Sdl(_) => "SDL3",
            #[cfg(windows)]
            Backend::XInput(_) => "XInput",
        }
    }

    fn live_input_summary(&self) -> String {
        match self {
            Backend::Sdl(b) => b.live_input_summary(),
            #[cfg(windows)]
            Backend::XInput(b) => b.live_input_summary(),
        }
    }

    fn is_connected(&self) -> bool {
        self.controller_label() != "none"
    }

    /// Gyro angular velocity — no companion up-frame consumes it yet (gyro-scroll is a
    /// future config behaviour), but it is wired so the data is reachable.
    #[allow(dead_code)]
    fn gyro(&self) -> Option<(f32, f32, f32)> {
        match self {
            Backend::Sdl(b) => b.gyro(),
            #[cfg(windows)]
            Backend::XInput(b) => b.gyro(),
        }
    }

    fn battery(&self) -> crate::gamepad_backend::BatteryFrame {
        match self {
            Backend::Sdl(b) => b.battery(),
            #[cfg(windows)]
            Backend::XInput(b) => b.battery(),
        }
    }

    fn set_led(&mut self, r: u8, g: u8, b: u8) {
        match self {
            Backend::Sdl(be) => be.set_led(r, g, b),
            #[cfg(windows)]
            Backend::XInput(be) => be.set_led(r, g, b),
        }
    }

    fn rumble(&mut self, strong: f32, weak: f32, duration_ms: u32) {
        match self {
            Backend::Sdl(b) => b.rumble(strong, weak, duration_ms),
            #[cfg(windows)]
            Backend::XInput(b) => b.rumble(strong, weak, duration_ms),
        }
    }

    fn trigger_rumble(&mut self, left: f32, right: f32, duration_ms: u32) {
        match self {
            Backend::Sdl(b) => b.trigger_rumble(left, right, duration_ms),
            #[cfg(windows)]
            Backend::XInput(b) => b.trigger_rumble(left, right, duration_ms),
        }
    }

    /// Apply queued device-write commands (LED/rumble) from inbound IPC frames.
    fn apply_device_commands(&mut self) {
        use crate::gamepad_backend::PadCommand;
        for cmd in crate::pipe_server::drain_device_commands() {
            match cmd {
                PadCommand::Led { r, g, b } => self.set_led(r, g, b),
                PadCommand::Rumble { strong, weak, ms } => self.rumble(strong, weak, ms),
                PadCommand::TriggerRumble { left, right, ms } => {
                    self.trigger_rumble(left, right, ms)
                }
            }
        }
    }

    fn haptic_confirm(&mut self) {
        self.rumble(0.0, 0.07, HAPTIC_CONFIRM_MS);
    }

    fn haptic_alert(&mut self) {
        self.rumble(0.24, 0.12, HAPTIC_ALERT_MS);
    }

    /// Publish device-feature reads (battery on change, touchpad coalesced) to the IPC server.
    fn publish_device_features(&self, tp: &crate::gamepad_backend::TouchpadFrame) {
        let bat = self.battery();
        crate::pipe_server::publish_battery(bat.percent, bat.charging, bat.wired);

        // Only publish when a finger slot is present this poll — avoids spamming empty frames.
        if !tp.fingers.is_empty() {
            let fingers = tp
                .fingers
                .iter()
                .map(|f| crate::protocol::TouchpadFingerPayload {
                    index: f.index,
                    down: f.down,
                    x: f.x,
                    y: f.y,
                    pressure: f.pressure,
                })
                .collect();
            crate::pipe_server::publish_touchpad(crate::protocol::TouchpadPayload { fingers });
        }
    }
}

pub struct GamepadPoll {
    backend: Backend,
    vk_down: bool,
    a_down_while_vk: bool,
    a_cursor_down: bool,
    touchpad_cursor_down: bool,
    last_vk_open: bool,
    /// The L3 press that opened the VK is still physically down; close only on the
    /// *next* press. Release-gated, not time-gated — close is instant once L3 lifts.
    vk_toggle_need_release: bool,
    vk_nav_grace_until: Option<Instant>,
    stick_nav: Option<Button>,
    launch_select_down: bool,
    launch_lb_down: bool,
    launch_x_down: bool,
    launch_armed: bool,
    last_launch: Instant,
    last_desktop_log: Instant,
    #[cfg(windows)]
    last_input_desktop: Option<String>,
    #[cfg(windows)]
    last_desktop_watch: Option<(String, String)>,
}

impl GamepadPoll {
    pub fn open_desktop() -> Result<Self, String> {
        let backend = SdlBackend::open()?;
        let label = backend.controller_label();
        if label == "none" {
            println!("> warmup-gamepad (SDL3): no pad connected yet");
        } else {
            println!("> warmup-gamepad (SDL3): {label}");
        }
        Ok(Self::new(Backend::Sdl(Box::new(backend))))
    }

    #[cfg(windows)]
    pub fn open_service() -> Self {
        let mut poll = Self::new(Self::service_backend_for_input_desktop());
        poll.sync_service_backend();
        poll
    }

    /// Winlogon sign-in: raw HID + XInput. Logged-in desktop: SDL3 (DualSense, etc.).
    #[cfg(windows)]
    fn service_backend_for_input_desktop() -> Backend {
        if Self::input_desktop_is_winlogon() {
            service_log("gamepad backend: HID + XInput (Winlogon)");
            Backend::XInput(XInputBackend::new())
        } else {
            // Service userland: SDL on its own thread (off the loop/SendInput thread).
            match SdlThreadBackend::open() {
                Ok(b) => {
                    service_log(&format!(
                        "gamepad backend: SDL3 (userland) — {}",
                        b.controller_label()
                    ));
                    Backend::Sdl(Box::new(b))
                }
                Err(e) => {
                    service_log(&format!(
                        "gamepad backend: SDL3 unavailable ({e}); using XInput fallback"
                    ));
                    Backend::XInput(XInputBackend::new())
                }
            }
        }
    }

    #[cfg(windows)]
    fn input_desktop_is_winlogon() -> bool {
        crate::win::input_desktop_name()
            .map(|n| n.eq_ignore_ascii_case("winlogon"))
            .unwrap_or(false)
    }

    /// Switch between SDL3 (Default) and HID+XInput (Winlogon) when the service crosses desktops.
    #[cfg(windows)]
    fn sync_service_backend(&mut self) {
        if !crate::config::service_mode() {
            return;
        }
        let on_winlogon = Self::input_desktop_is_winlogon();
        // Publish each poll so the per-keystroke UIA focus redirect (vk_nav send
        // path) gates correctly and records this loop thread's apartment.
        crate::win::logon_focus::set_active(on_winlogon);
        // Keep a hide sweep alive the whole time we're on the secure desktop.
        // `EnableDesktopModeAutoInvoke` is already 0 on some machines yet the
        // touch keyboard is still summoned by the shell's CoreWindow gamepad
        // navigation when an XInput (Xbox) pad drives the lock screen — the
        // registry switch doesn't gate that path, so the panel must be hidden
        // on sight. Idempotent: `suppress_for` no-ops while a sweep is running.
        if on_winlogon {
            crate::win::native_keyboard::suppress_for(std::time::Duration::from_secs(3));
        }
        let using_xinput = matches!(self.backend, Backend::XInput(_));
        if on_winlogon == using_xinput {
            return;
        }
        self.reset_vk_controls();
        self.backend = if on_winlogon {
            service_log("input desktop → Winlogon: switching to HID+XInput");
            // Loop thread owns SendInput; bind it to winlogon so injected keys
            // (and XInput/anchor delivery) land on the secure desktop.
            if let Err(e) = crate::win::attach_named("winlogon") {
                service_log(&format!("loop thread attach winlogon failed: {e}"));
            }
            Backend::XInput(XInputBackend::new())
        } else {
            // SendInput targets the *calling thread's* desktop. Re-bind the loop
            // thread to the Default input desktop or typed keys land on winlogon
            // and never reach userland apps.
            if let Err(e) = crate::win::attach_input() {
                service_log(&format!("loop thread attach input(Default) failed: {e}"));
            }
            match SdlThreadBackend::open() {
                Ok(b) => {
                    service_log(&format!(
                        "input desktop → Default: switching to SDL3 — {}",
                        b.controller_label()
                    ));
                    Backend::Sdl(Box::new(b))
                }
                Err(e) => {
                    service_log(&format!(
                        "input desktop → Default: SDL3 failed ({e}); keeping XInput"
                    ));
                    return;
                }
            }
        };
    }

    #[cfg(not(windows))]
    pub fn open_service() -> Result<Self, String> {
        Err("XInput service backend requires Windows".to_string())
    }

    fn new(backend: Backend) -> Self {
        Self {
            backend,
            vk_down: false,
            a_down_while_vk: false,
            a_cursor_down: false,
            touchpad_cursor_down: false,
            last_vk_open: false,
            vk_toggle_need_release: false,
            vk_nav_grace_until: None,
            stick_nav: None,
            launch_select_down: false,
            launch_lb_down: false,
            launch_x_down: false,
            launch_armed: true,
            last_launch: crate::time_util::stale(WARMUP_LAUNCH_DEBOUNCE),
            last_desktop_log: crate::time_util::stale(DESKTOP_SYNC_LOG_INTERVAL),
            #[cfg(windows)]
            last_input_desktop: None,
            #[cfg(windows)]
            last_desktop_watch: None,
        }
    }

    /// Clear VK navigation / Y-latch state after close, desktop change, or reopen.
    pub fn reset_vk_controls(&mut self) {
        self.vk_down = false;
        self.a_down_while_vk = false;
        self.a_cursor_down = false;
        self.touchpad_cursor_down = false;
        self.vk_toggle_need_release = false;
        self.vk_nav_grace_until = None;
        self.stick_nav = None;
        self.reset_launch_hotkey();
        self.last_vk_open = false;
        #[cfg(windows)]
        {
            crate::vk_nav::reset_selection();
            crate::win::logon_focus::clear_cache();
        }
    }

    pub fn on_vk_opened(&mut self) {
        self.vk_down = false;
        self.a_down_while_vk = false;
        self.vk_toggle_need_release = true;
        self.vk_nav_grace_until = Some(Instant::now() + VK_NAV_INPUT_GRACE);
        #[cfg(windows)]
        {
            crate::vk_nav::reset_selection();
            if Self::service_signin_desktop() {
                service_log("sign-in: VK open — LB=page RB=Enter L3=close");
            }
        }
    }

    pub fn open() -> Result<Self, String> {
        Self::open_desktop()
    }

    pub fn controller_label(&self) -> String {
        self.backend.controller_label()
    }

    pub fn poll_frame(
        &mut self,
        cursor: &mut PcCursor,
        dt_secs: f32,
        vk_open: bool,
    ) -> Result<Vec<VkLoopAction>, String> {
        if crate::gamepad_backend::userland_poll_paused() {
            self.backend.apply_device_commands();
            cursor.set_left_button(false);
            return Ok(Vec::new());
        }

        if vk_open && !self.last_vk_open {
            self.on_vk_opened();
            self.backend.haptic_confirm();
            self.a_cursor_down = false;
            self.touchpad_cursor_down = false;
            cursor.set_left_button(false);
        }
        self.last_vk_open = vk_open;

        #[cfg(windows)]
        if crate::config::service_mode() {
            self.sync_service_backend();
            // Route cursor injection to the secure desktop while locked: on Winlogon
            // the loop thread is attached there, so inline SendInput reaches the PIN
            // keypad; post-login the Default-desktop injector thread is used.
            cursor.set_on_winlogon(Self::input_desktop_is_winlogon());
        }

        // Apply any LED/rumble commands the desktop pushed since the last poll, then
        // poll the pad and publish its device-feature reads (battery/touchpad).
        self.backend.apply_device_commands();
        let PadFrame {
            changes,
            axes,
            touchpad,
        } = self.backend.poll_frame()?;
        self.backend.publish_device_features(&touchpad);
        let (lx, ly, rx, ry) = axes;

        #[cfg(windows)]
        if crate::config::service_mode() {
            let input = self.backend.live_input_summary();
            crate::debug_state::set_gamepad(self.backend.is_connected(), input);
        }

        #[cfg(windows)]
        let desktop_reopen = self.reopen_on_input_desktop_change(vk_open);
        #[cfg(not(windows))]
        let desktop_reopen = None;

        if vk_open {
            #[cfg(windows)]
            {
                self.sync_stick_nav(lx, ly);
                if crate::win::vk_ui::tick_dpad_hold(Instant::now()) {
                    crate::win::vk_ui::request_repaint();
                }
            }
            let mut edges = Vec::new();
            if let Some(edge) = desktop_reopen {
                edges.push(edge);
            }
            for change in &changes {
                if let Some(edge) = self.handle_vk_open_button(change) {
                    edges.push(edge);
                }
            }
            return Ok(edges);
        }

        cursor.move_touchpad(touchpad.delta);
        cursor.move_stick(lx, ly, dt_secs);
        cursor.scroll_stick(rx, ry, dt_secs);

        let changes = dedupe_consecutive_toggle_edges(changes);
        let mut edges = Vec::new();
        if let Some(edge) = desktop_reopen {
            edges.push(edge);
        }
        for change in changes {
            // Forward every edge to the warmUP desktop over the pipe so the launcher grid
            // is gamepad-navigable (#348). The companion still drives its own VK/cursor below.
            crate::pipe_server::publish_button(change.button.as_str(), change.pressed);
            if change.button == Button::A || change.button == Button::Touchpad {
                // Hold: button down -> mouse-left down, up -> up, so
                // the PIN keypad sees a real press duration (not an instant click).
                // Gate the *press* by cursor mode (#349); always forward the release so a
                // click can't get stuck down if the mode flips while A is held.
                match change.button {
                    Button::A => self.a_cursor_down = change.pressed,
                    Button::Touchpad => self.touchpad_cursor_down = change.pressed,
                    _ => {}
                }
                let any_click_down = self.a_cursor_down || self.touchpad_cursor_down;
                cursor.set_left_button(any_click_down && crate::pipe_server::clicks_enabled());
            }
            if crate::pipe_server::native_vk_suppressed() {
                continue;
            }
            if self.update_launch_hotkey(change) {
                self.backend.haptic_alert();
                edges.push(VkLoopAction::LaunchWarmup);
            }
            if change.button != VK_BUTTON {
                continue;
            }
            let edge = match (self.vk_down, change.pressed) {
                (false, true) => {
                    self.vk_down = true;
                    self.backend.haptic_confirm();
                    Some(VkLoopAction::Toggle)
                }
                (true, false) => {
                    self.vk_down = false;
                    None
                }
                _ => None,
            };
            if let Some(e) = edge {
                edges.push(e);
            }
        }
        Ok(edges)
    }

    fn update_launch_hotkey(&mut self, change: ButtonChange) -> bool {
        #[cfg(windows)]
        if Self::service_signin_desktop() {
            self.reset_launch_hotkey();
            return false;
        }

        match change.button {
            Button::Select => self.launch_select_down = change.pressed,
            Button::Lb => self.launch_lb_down = change.pressed,
            Button::X => self.launch_x_down = change.pressed,
            _ => {}
        }

        let combo_down = self.launch_select_down && self.launch_lb_down && self.launch_x_down;
        if !combo_down {
            self.launch_armed = true;
            return false;
        }
        if !self.launch_armed || self.last_launch.elapsed() < WARMUP_LAUNCH_DEBOUNCE {
            return false;
        }
        self.launch_armed = false;
        self.last_launch = Instant::now();
        true
    }

    fn reset_launch_hotkey(&mut self) {
        self.launch_select_down = false;
        self.launch_lb_down = false;
        self.launch_x_down = false;
        self.launch_armed = true;
    }

    #[cfg(windows)]
    fn reopen_on_input_desktop_change(&mut self, vk_open: bool) -> Option<VkLoopAction> {
        if !crate::config::service_mode() {
            return None;
        }
        let Ok(input) = crate::win::input_desktop_name() else {
            return None;
        };
        let changed = self
            .last_input_desktop
            .as_ref()
            .is_some_and(|old| old != &input);
        self.last_input_desktop = Some(input.clone());
        if !vk_open || !changed {
            return None;
        }
        let on_winlogon = input.eq_ignore_ascii_case("winlogon");
        self.reset_vk_controls();
        if on_winlogon {
            service_log(&format!("input desktop changed to {input}; reopening VK"));
            Some(VkLoopAction::Reopen)
        } else {
            service_log(&format!(
                "input desktop changed to {input}; closing VK (left Winlogon)"
            ));
            Some(VkLoopAction::Close)
        }
    }

    #[cfg(windows)]
    fn service_signin_desktop() -> bool {
        crate::config::service_mode() && Self::input_desktop_is_winlogon()
    }

    #[cfg(windows)]
    fn handle_vk_open_button(&mut self, change: &ButtonChange) -> Option<VkLoopAction> {
        use crate::vk_nav;
        use crate::win::vk_ui;

        if change.button != VK_BUTTON
            && self
                .vk_nav_grace_until
                .is_some_and(|until| Instant::now() < until)
        {
            return None;
        }

        // A=activate, B=backspace, X=space, Y=voice input, LB=page, RB=Enter, LT=shift, RT=caps, L3=close,
        // D-pad/L-stick axis=move focus.
        match (change.button, change.pressed) {
            (VK_BUTTON, true) => {
                // Same press that opened the VK is still held — wait for release.
                if self.vk_toggle_need_release {
                    return None;
                }
                self.backend.haptic_confirm();
                Some(VkLoopAction::Close)
            }
            (VK_BUTTON, false) => {
                self.vk_down = false;
                // L3 lifted: a fresh press may now close immediately.
                self.vk_toggle_need_release = false;
                None
            }
            (Button::Up | Button::Down | Button::Left | Button::Right, true) => {
                vk_nav::dpad_pressed(change.button);
                vk_ui::request_repaint();
                None
            }
            (Button::Up | Button::Down | Button::Left | Button::Right, false) => {
                vk_nav::dpad_released(change.button);
                None
            }
            (Button::A, true) => {
                self.a_down_while_vk = true;
                None
            }
            (Button::A, false) if self.a_down_while_vk => {
                self.a_down_while_vk = false;
                let mut sink = vk_nav::SendInputSink;
                if crate::vk_predict::commit_if_engaged(&mut sink).is_none() {
                    vk_nav::activate_selection();
                }
                vk_ui::request_repaint();
                None
            }
            (Button::B, true) => {
                vk_nav::backspace();
                vk_ui::request_repaint();
                None
            }
            (Button::X, true) => {
                vk_nav::space();
                vk_ui::request_repaint();
                None
            }
            (Button::Y, true) => {
                vk_nav::start_voice_input();
                vk_ui::request_repaint();
                self.backend.haptic_alert();
                None
            }
            (Button::Lb, true) => {
                // cycle_prev returns true iff the strip was active (context swap).
                if crate::vk_predict::cycle_prev() {
                    vk_ui::request_repaint();
                } else {
                    vk_nav::next_layer();
                }
                None
            }
            (Button::Rb, true) => {
                if crate::vk_predict::cycle_next() {
                    vk_ui::request_repaint();
                } else {
                    vk_nav::enter();
                }
                None
            }
            (Button::Lt, true) => {
                vk_nav::set_shift(true);
                None
            }
            (Button::Lt, false) => {
                vk_nav::set_shift(false);
                None
            }
            (Button::Rt, true) => {
                vk_nav::toggle_caps();
                None
            }
            _ => None,
        }
    }

    #[cfg(not(windows))]
    fn handle_vk_open_button(&mut self, _change: &ButtonChange) -> Option<VkLoopAction> {
        None
    }

    /// Left stick → grid focus with the same hold-repeat path as the D-pad (`FUN_00464d00`).
    #[cfg(windows)]
    fn sync_stick_nav(&mut self, lx: f32, ly: f32) {
        use crate::vk_nav;
        use crate::win::vk_ui;

        const THRESH: f32 = 0.55;
        let dir = if lx.abs().max(ly.abs()) < THRESH {
            None
        } else if lx.abs() > ly.abs() {
            Some(if lx > 0.0 {
                Button::Right
            } else {
                Button::Left
            })
        } else {
            Some(if ly > 0.0 { Button::Up } else { Button::Down })
        };

        match (self.stick_nav, dir) {
            (None, None) => {}
            (None, Some(d)) => {
                vk_nav::dpad_pressed(d);
                vk_ui::request_repaint();
            }
            (Some(old), None) => vk_nav::dpad_released(old),
            (Some(old), Some(d)) if old != d => {
                vk_nav::dpad_released(old);
                vk_nav::dpad_pressed(d);
                vk_ui::request_repaint();
            }
            _ => {}
        }
        self.stick_nav = dir;
    }

    pub fn snapshot(&mut self) -> Result<String, String> {
        let kind = self.backend.kind_label();
        let frame = self.backend.poll_frame()?;
        let name = self.backend.controller_label();
        let (lx, ly, _, _) = frame.axes;
        Ok(format!("{name} ({kind}) stick=({lx:.2},{ly:.2})"))
    }

    pub fn log_desktop_sync_if_due(&mut self, service_mode: bool) {
        if !service_mode {
            return;
        }
        #[cfg(windows)]
        {
            let name = crate::win::current_desktop_name().unwrap_or_else(|| "?".into());
            let input = crate::win::input_desktop_name().unwrap_or_else(|e| format!("? ({e})"));
            let current = (name, input);
            let changed = self.last_desktop_watch.as_ref() != Some(&current);
            if !changed && self.last_desktop_log.elapsed() < DESKTOP_SYNC_LOG_INTERVAL {
                return;
            }
            service_log(&format!(
                "desktop watch: worker thread on {}; input desktop {}",
                current.0, current.1
            ));
            self.last_desktop_watch = Some(current);
            self.last_desktop_log = Instant::now();
        }
    }
}

fn dedupe_consecutive_toggle_edges(changes: Vec<ButtonChange>) -> Vec<ButtonChange> {
    let mut out: Vec<ButtonChange> = Vec::with_capacity(changes.len());
    for c in changes {
        if c.button == VK_BUTTON {
            if let Some(last) = out.last() {
                if last.button == VK_BUTTON && last.pressed == c.pressed {
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

pub fn request_stop() {
    RUNNING.store(false, Ordering::SeqCst);
}

pub fn install_ctrlc_handler() -> Result<(), String> {
    RUNNING.store(true, Ordering::SeqCst);
    ctrlc::set_handler(|| {
        RUNNING.store(false, Ordering::SeqCst);
        eprintln!("\n> Ctrl+C — stopping…");
    })
    .map_err(|e| format!("ctrl+c handler: {e}"))
}

fn interruptible_sleep(duration: Duration) {
    const SLICE: Duration = Duration::from_millis(16);
    let mut remaining = duration;
    while remaining > Duration::ZERO && RUNNING.load(Ordering::SeqCst) {
        let step = remaining.min(SLICE);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

pub fn run_watch_loop<V, A>(vk_open: V, on_action: A) -> Result<(), String>
where
    V: FnMut() -> bool,
    A: FnMut(VkLoopAction),
{
    run_watch_loop_inner(vk_open, on_action, true, false)
}

pub fn run_watch_loop_service<V, A>(vk_open: V, on_action: A) -> Result<(), String>
where
    V: FnMut() -> bool,
    A: FnMut(VkLoopAction),
{
    run_watch_loop_inner(vk_open, on_action, false, true)
}

#[cfg(windows)]
fn service_log(msg: &str) {
    if crate::config::service_mode() {
        crate::install::log_line(msg);
    }
}

fn run_watch_loop_inner<V, A>(
    mut vk_open: V,
    mut on_action: A,
    use_ctrlc: bool,
    service_mode: bool,
) -> Result<(), String>
where
    V: FnMut() -> bool,
    A: FnMut(VkLoopAction),
{
    let mut poll = if service_mode {
        #[cfg(windows)]
        {
            GamepadPoll::open_service()
        }
        #[cfg(not(windows))]
        {
            return GamepadPoll::open_service();
        }
    } else {
        match GamepadPoll::open_desktop() {
            Ok(p) => p,
            Err(e) => return Err(e),
        }
    };

    #[cfg(windows)]
    if service_mode {
        service_log(&format!("gamepad ready: {}", poll.controller_label()));
    }
    if use_ctrlc {
        install_ctrlc_handler()?;
    } else {
        RUNNING.store(true, Ordering::SeqCst);
    }
    let mut cursor = if service_mode {
        PcCursor::new_service()
    } else {
        PcCursor::new()?
    };
    if !service_mode {
        println!("Controls (VK closed):");
        println!("  left stick   → mouse");
        println!("  right stick  → scroll");
        println!("  A            → click");
        println!("  L3 (stick click) → open keyboard");
        println!("Controls (VK open):");
        println!("  D-pad/L-stick → move key focus");
        println!("  A            → type selected key");
        println!("  B            → backspace");
        println!("  X            → space");
        println!("  L3           → close keyboard");
        println!("  LB           → next page (#+= symbols, ABC)");
        println!("  RB           → Enter");
        println!("  Shift        → on-screen ⇧ key + A");
        println!("Ctrl+C to stop.");
    } else {
        #[cfg(windows)]
        service_log(&format!(
            "gamepad loop running ({}; L3=toggle VK)",
            poll.controller_label()
        ));
    }
    let mut last_tick = Instant::now();
    while RUNNING.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f32();
        last_tick = now;

        // Publish the current controller connection state to the pipe server (#347).
        crate::pipe_server::publish_from_label(&poll.controller_label());

        match poll.poll_frame(&mut cursor, dt, vk_open()) {
            Ok(actions) => {
                for action in actions {
                    match action {
                        VkLoopAction::Close => {
                            poll.reset_vk_controls();
                            on_action(action);
                        }
                        VkLoopAction::Reopen => {
                            poll.reset_vk_controls();
                            on_action(action);
                            if vk_open() {
                                poll.on_vk_opened();
                            }
                        }
                        VkLoopAction::Toggle => {
                            let was_open = vk_open();
                            on_action(action);
                            if vk_open() && !was_open {
                                poll.on_vk_opened();
                            } else if !vk_open() && was_open {
                                poll.reset_vk_controls();
                            }
                        }
                        VkLoopAction::LaunchWarmup => on_action(action),
                    }
                }
            }
            Err(e) => {
                if service_mode {
                    #[cfg(windows)]
                    service_log(&format!("gamepad poll error (continuing): {e}"));
                } else {
                    return Err(e);
                }
            }
        }

        #[cfg(windows)]
        if service_mode {
            if crate::config::debug_ui_enabled() {
                crate::win::debug_overlay::tick();
            }
            if crate::config::prompt_overlay_enabled() {
                crate::win::prompt_overlay::tick(vk_open());
            }
            poll.log_desktop_sync_if_due(true);
            if crate::config::debug_ui_enabled() {
                if crate::win::debug_overlay::take_vk_toggle_request() {
                    on_action(VkLoopAction::Toggle);
                }
                // Loop thread owns the UIA/COM apartment and (on winlogon) is
                // attached there, so the foreground dump must run here.
                if crate::win::logon_focus::take_dump_request() {
                    crate::win::logon_focus::dump_foreground_tree();
                }
            }
        }

        let elapsed = now.elapsed();
        if elapsed < POLL_INTERVAL {
            interruptible_sleep(POLL_INTERVAL - elapsed);
        }
    }
    #[cfg(windows)]
    if service_mode {
        service_log("gamepad loop exited (stop flag)");
    }
    Ok(())
}
