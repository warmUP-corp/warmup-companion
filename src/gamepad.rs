//! Gamepad: PC cursor when VK closed; full keyboard control when VK open.
//!
//! Service mode uses XInput (secure desktop). Desktop `--gamepad` uses SDL3.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static RUNNING: AtomicBool = AtomicBool::new(true);

use crate::gamepad_backend::{ButtonChange, GamepadBackend, SdlBackend};
use crate::pc_cursor::PcCursor;

#[cfg(windows)]
use crate::xinput_backend::XInputBackend;

/// North face: Triangle / Y — toggles VK when keyboard is closed.
const VK_MASK_BUTTON: &str = "Y";

const POLL_INTERVAL: Duration = Duration::from_millis(8);
/// Ignore Y release right after opening VK (same physical tap must not close).
const Y_RELEASE_GRACE: Duration = Duration::from_millis(550);
const DESKTOP_SYNC_LOG_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkLoopAction {
    Toggle,
    Close,
    Reopen,
}

enum Backend {
    Sdl(SdlBackend),
    #[cfg(windows)]
    XInput(XInputBackend),
}

pub struct GamepadPoll {
    backend: Backend,
    vk_down: bool,
    a_down_while_vk: bool,
    last_vk_open: bool,
    y_ignore_until: Option<Instant>,
    last_desktop_log: Instant,
    /// Left-stick emulated D-pad while VK is open (secure desktop often lacks XInput dpad bits).
    stick_dpad: Option<&'static str>,
    #[cfg(windows)]
    last_input_desktop: Option<String>,
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
        Ok(Self::new(Backend::Sdl(backend)))
    }

    #[cfg(windows)]
    pub fn open_service() -> Self {
        service_log("gamepad backend: XInput (service / secure desktop)");
        Self::new(Backend::XInput(XInputBackend::new()))
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
            last_vk_open: false,
            y_ignore_until: None,
            last_desktop_log: Instant::now() - DESKTOP_SYNC_LOG_INTERVAL,
            stick_dpad: None,
            #[cfg(windows)]
            last_input_desktop: None,
        }
    }

    /// Clear VK navigation / Y-latch state after close, desktop change, or reopen.
    pub fn reset_vk_controls(&mut self) {
        self.vk_down = false;
        self.a_down_while_vk = false;
        self.y_ignore_until = None;
        self.stick_dpad = None;
        self.last_vk_open = false;
        #[cfg(windows)]
        crate::vk_nav::reset_selection();
    }

    pub fn on_vk_opened(&mut self) {
        self.vk_down = false;
        self.a_down_while_vk = false;
        self.stick_dpad = None;
        self.y_ignore_until = Some(Instant::now() + Y_RELEASE_GRACE);
        #[cfg(windows)]
        crate::vk_nav::reset_selection();
    }

    fn stick_dpad_edges(&mut self, lx: f32, ly: f32) -> Vec<ButtonChange> {
        const THRESH: f32 = 0.55;
        let dir = if ly > THRESH && ly.abs() >= lx.abs() {
            Some("UP")
        } else if ly < -THRESH && ly.abs() >= lx.abs() {
            Some("DOWN")
        } else if lx < -THRESH {
            Some("LEFT")
        } else if lx > THRESH {
            Some("RIGHT")
        } else {
            None
        };

        let mut edges = Vec::new();
        match (self.stick_dpad, dir) {
            (Some(old), Some(new)) if old != new => {
                edges.push(ButtonChange {
                    button_name: old,
                    pressed: false,
                });
                edges.push(ButtonChange {
                    button_name: new,
                    pressed: true,
                });
                self.stick_dpad = Some(new);
            }
            (Some(old), None) => {
                edges.push(ButtonChange {
                    button_name: old,
                    pressed: false,
                });
                self.stick_dpad = None;
            }
            (None, Some(new)) => {
                edges.push(ButtonChange {
                    button_name: new,
                    pressed: true,
                });
                self.stick_dpad = Some(new);
            }
            _ => {}
        }
        edges
    }

    pub fn open() -> Result<Self, String> {
        Self::open_desktop()
    }

    pub fn controller_label(&self) -> String {
        match &self.backend {
            Backend::Sdl(b) => b.controller_label(),
            #[cfg(windows)]
            Backend::XInput(b) => b.controller_label(),
        }
    }

    pub fn poll_frame(
        &mut self,
        cursor: &mut PcCursor,
        dt_secs: f32,
        vk_open: bool,
    ) -> Result<Vec<VkLoopAction>, String> {
        if vk_open && !self.last_vk_open {
            self.on_vk_opened();
        }
        if !vk_open && self.last_vk_open {
            self.stick_dpad = None;
        }
        self.last_vk_open = vk_open;

        match &mut self.backend {
            Backend::Sdl(b) => b.poll()?,
            #[cfg(windows)]
            Backend::XInput(b) => b.poll()?,
        }
        let changes = match &mut self.backend {
            Backend::Sdl(b) => b.button_changes(),
            #[cfg(windows)]
            Backend::XInput(b) => b.button_changes(),
        };
        let (lx, ly, rx, ry) = match &self.backend {
            Backend::Sdl(b) => b.axes(),
            #[cfg(windows)]
            Backend::XInput(b) => b.axes(),
        };

        #[cfg(windows)]
        let desktop_reopen = self.reopen_on_input_desktop_change(vk_open);
        #[cfg(not(windows))]
        let desktop_reopen = None;

        if vk_open {
            #[cfg(windows)]
            {
                if crate::win::vk_ui::tick_dpad_hold(Instant::now()) {
                    crate::win::vk_ui::request_repaint();
                }
            }
            let mut edges = Vec::new();
            if let Some(edge) = desktop_reopen {
                edges.push(edge);
            }
            let mut vk_changes = changes;
            vk_changes.extend(self.stick_dpad_edges(lx, ly));
            for change in &vk_changes {
                if let Some(edge) = self.handle_vk_open_button(change) {
                    edges.push(edge);
                }
            }
            return Ok(edges);
        }

        cursor.move_stick(lx, ly, dt_secs);
        cursor.scroll_stick(rx, ry, dt_secs);

        let changes = dedupe_consecutive_y_edges(changes);
        let mut edges = Vec::new();
        if let Some(edge) = desktop_reopen {
            edges.push(edge);
        }
        for change in changes {
            if change.button_name == "A" && change.pressed {
                cursor.left_click();
            }
            if change.button_name != VK_MASK_BUTTON {
                continue;
            }
            let edge = match (self.vk_down, change.pressed) {
                (false, true) => {
                    self.vk_down = true;
                    None
                }
                (true, false) => {
                    self.vk_down = false;
                    Some(VkLoopAction::Toggle)
                }
                _ => None,
            };
            if let Some(e) = edge {
                edges.push(e);
            }
        }
        Ok(edges)
    }

    #[cfg(windows)]
    fn reopen_on_input_desktop_change(&mut self, vk_open: bool) -> Option<VkLoopAction> {
        if !std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
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
        if vk_open && changed {
            service_log(&format!("input desktop changed to {input}; reopening VK"));
            self.reset_vk_controls();
            Some(VkLoopAction::Reopen)
        } else {
            None
        }
    }

    #[cfg(windows)]
    fn handle_vk_open_button(&mut self, change: &ButtonChange) -> Option<VkLoopAction> {
        use crate::vk_nav;
        use crate::win::vk_ui;

        match (change.button_name, change.pressed) {
            (VK_MASK_BUTTON, false) => {
                if self
                    .y_ignore_until
                    .is_some_and(|until| Instant::now() < until)
                {
                    return None;
                }
                Some(VkLoopAction::Toggle)
            }
            (VK_MASK_BUTTON, true) => None,
            ("UP" | "DOWN" | "LEFT" | "RIGHT", true) => {
                vk_nav::dpad_pressed(change.button_name);
                vk_ui::request_repaint();
                None
            }
            ("UP" | "DOWN" | "LEFT" | "RIGHT", false) => {
                vk_nav::dpad_released(change.button_name);
                None
            }
            ("A", true) => {
                self.a_down_while_vk = true;
                None
            }
            ("A", false) if self.a_down_while_vk => {
                self.a_down_while_vk = false;
                vk_nav::activate_selection();
                None
            }
            ("B", true) => {
                vk_nav::backspace();
                None
            }
            ("X", true) => Some(VkLoopAction::Close),
            ("LB", true) => {
                vk_nav::cursor_left();
                None
            }
            ("RB", true) => {
                vk_nav::enter();
                None
            }
            _ => None,
        }
    }

    #[cfg(not(windows))]
    fn handle_vk_open_button(&mut self, _change: &ButtonChange) -> Option<VkLoopAction> {
        None
    }

    pub fn snapshot(&mut self) -> Result<String, String> {
        match &mut self.backend {
            Backend::Sdl(b) => {
                b.poll()?;
                let name = b.controller_label();
                let (lx, ly, _, _) = b.axes();
                Ok(format!("{name} (SDL3) stick=({lx:.2},{ly:.2})"))
            }
            #[cfg(windows)]
            Backend::XInput(b) => {
                b.poll()?;
                let name = b.controller_label();
                let (lx, ly, _, _) = b.axes();
                Ok(format!("{name} (XInput) stick=({lx:.2},{ly:.2})"))
            }
        }
    }

    pub fn log_desktop_sync_if_due(&mut self, service_mode: bool) {
        if !service_mode {
            return;
        }
        if self.last_desktop_log.elapsed() < DESKTOP_SYNC_LOG_INTERVAL {
            return;
        }
        self.last_desktop_log = Instant::now();
        #[cfg(windows)]
        {
            let name = crate::win::current_desktop_name().unwrap_or_else(|| "?".into());
            let input = crate::win::input_desktop_name().unwrap_or_else(|e| format!("? ({e})"));
            service_log(&format!(
                "desktop watch: worker thread on {name}; input desktop {input}"
            ));
        }
    }
}

fn dedupe_consecutive_y_edges(changes: Vec<ButtonChange>) -> Vec<ButtonChange> {
    let mut out: Vec<ButtonChange> = Vec::with_capacity(changes.len());
    for c in changes {
        if c.button_name == VK_MASK_BUTTON {
            if let Some(last) = out.last() {
                if last.button_name == VK_MASK_BUTTON && last.pressed == c.pressed {
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
    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
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
        println!("  tap Y        → open keyboard");
        println!("Controls (VK open):");
        println!("  D-pad        → move key focus");
        println!("  A            → type selected key");
        println!("  B            → backspace");
        println!("  X            → close keyboard");
        println!("  LB           → cursor left in field");
        println!("  RB           → Enter");
        println!("  tap Y        → close keyboard");
        println!("Ctrl+C to stop.");
    } else {
        #[cfg(windows)]
        service_log("gamepad loop running (XInput, Y=toggle VK)");
    }
    let mut last_tick = Instant::now();
    while RUNNING.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f32();
        last_tick = now;

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
            crate::win::debug_overlay::tick();
            poll.log_desktop_sync_if_due(true);
            if crate::win::debug_overlay::take_vk_toggle_request() {
                on_action(VkLoopAction::Toggle);
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
