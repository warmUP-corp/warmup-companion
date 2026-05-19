//! Gamepad: PC cursor when VK closed; full keyboard control when VK open.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static RUNNING: AtomicBool = AtomicBool::new(true);

use warmup_gamepad::{ButtonChange, GamepadInput};

use crate::pc_cursor::PcCursor;

/// North face: Triangle / Y — toggles VK when keyboard is closed.
const VK_MASK_BUTTON: &str = "Y";

const POLL_INTERVAL: Duration = Duration::from_millis(8);
/// Ignore Y release right after opening VK (same physical tap must not close).
const Y_RELEASE_GRACE: Duration = Duration::from_millis(550);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkLoopAction {
    Toggle,
    Close,
}

pub struct GamepadPoll {
    input: GamepadInput,
    vk_down: bool,
    a_down_while_vk: bool,
    last_vk_open: bool,
    y_ignore_until: Option<Instant>,
}

impl GamepadPoll {
    pub fn open() -> Result<Self, String> {
        let db = mapping_db_path();
        let input = GamepadInput::new(&db)?;
        if let Some(name) = input.active_controller_name() {
            println!("> warmup-gamepad: {name}");
        } else {
            println!("> warmup-gamepad: no pad connected yet");
        }
        Ok(Self {
            input,
            vk_down: false,
            a_down_while_vk: false,
            last_vk_open: false,
            y_ignore_until: None,
        })
    }

    pub fn controller_label(&self) -> String {
        self.input
            .active_controller_name()
            .unwrap_or_else(|| "none".to_string())
    }

    pub fn poll_frame(
        &mut self,
        cursor: &mut PcCursor,
        dt_secs: f32,
        vk_open: bool,
    ) -> Result<Vec<VkLoopAction>, String> {
        if vk_open && !self.last_vk_open {
            self.vk_down = false;
            self.y_ignore_until = Some(Instant::now() + Y_RELEASE_GRACE);
        }
        self.last_vk_open = vk_open;

        self.input.poll_events();
        let changes = self.input.detect_button_changes();

        if vk_open {
            #[cfg(windows)]
            {
                if crate::win::vk_ui::tick_dpad_hold(Instant::now()) {
                    crate::win::vk_ui::request_repaint();
                }
            }
            let mut edges = Vec::new();
            for change in &changes {
                if let Some(edge) = self.handle_vk_open_button(change) {
                    edges.push(edge);
                }
            }
            return Ok(edges);
        }

        let (lx, ly, rx, ry) = self.input.axes();
        cursor.move_stick(lx, ly, dt_secs);
        cursor.scroll_stick(rx, ry, dt_secs);

        let changes = dedupe_consecutive_y_edges(changes);
        let mut edges = Vec::new();
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
        self.input.poll_events();
        let name = self
            .input
            .active_controller_name()
            .unwrap_or_else(|| "none".to_string());
        let ty = self.input.active_controller_type();
        let (lx, ly, _, _) = self.input.axes();
        Ok(format!("{name} ({ty}) stick=({lx:.2},{ly:.2})"))
    }
}

fn mapping_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("WARMUP_GAMECONTROLLER_DB") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        let installed =
            PathBuf::from(r"C:\ProgramData\WarmupVk\gamecontrollerdb.txt");
        if installed.is_file() {
            return installed;
        }
    }
    PathBuf::from(r"C:\Users\jonas\warmUp\apps\desktop\src-tauri\resources\gamecontrollerdb.txt")
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

pub fn run_watch_loop<V, A>(mut vk_open: V, mut on_action: A) -> Result<(), String>
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
    let mut poll = match GamepadPoll::open() {
        Ok(p) => p,
        Err(e) => {
            if service_mode {
                #[cfg(windows)]
                service_log(&format!("gamepad open failed: {e}"));
                return Err(e);
            }
            return Err(e);
        }
    };
    #[cfg(windows)]
    if service_mode {
        service_log(&format!("gamepad SDL ready: {}", poll.controller_label()));
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
        service_log("gamepad loop running (service mode, Y=toggle VK)");
    }
    let mut last_tick = Instant::now();
    while RUNNING.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f32();
        last_tick = now;

        #[cfg(windows)]
        if service_mode {
            crate::win::sync_input_desktop();
        }

        match poll.poll_frame(&mut cursor, dt, vk_open()) {
            Ok(actions) => {
                for action in actions {
                    on_action(action);
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
