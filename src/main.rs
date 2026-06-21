mod config;
/// Cursor/scroll golden-fixture loader (#346). Pure serde; used by tests and the
/// math-parity slice (#349). Unused in the normal binary build for now.
#[allow(dead_code)]
mod golden;
/// Named-pipe server (#347): streams gamepad connection state to the warmUP desktop.
mod pipe_server;
/// Companion IPC wire frames (#347). Pure serde; used by the pipe server and tests.
#[allow(dead_code)]
mod protocol;
mod sentry_telemetry;
mod symbols;
mod time_util;
mod vk_gate;

#[cfg(windows)]
mod crash;
#[cfg(windows)]
mod install;
#[cfg(all(windows, feature = "service"))]
mod service;
#[cfg(all(windows, feature = "gamepad"))]
mod tray;

#[cfg(windows)]
mod debug_state;
#[cfg(windows)]
mod predict_ngram;
#[cfg(windows)]
mod vk_commit;
#[cfg(windows)]
mod vk_nav;
#[cfg(windows)]
mod vk_predict;
#[cfg(windows)]
mod win;
#[cfg(not(windows))]
#[path = "win_stub.rs"]
mod win;

#[cfg(feature = "gamepad")]
mod gamepad;
#[cfg(feature = "gamepad")]
mod gamepad_backend;
#[cfg(all(windows, feature = "gamepad"))]
mod hid_gamepad;
#[cfg(all(windows, feature = "gamepad"))]
mod hid_reader;
#[cfg(all(windows, feature = "gamepad"))]
mod pad_decode;
#[cfg(feature = "gamepad")]
mod pc_cursor;
#[cfg(all(windows, feature = "gamepad"))]
mod xinput_backend;
#[cfg(all(windows, feature = "gamepad"))]
mod xusb_ioctl;

use std::env;
use std::fmt;
use std::io::{self, Write};

use symbols::{
    FN_APPLY_MASK_SLOT_ACTION, FN_ATTACH_INPUT_DESKTOP, FN_ATTACH_NAMED_DESKTOP,
    FN_CREATE_SPIRAL_VK_WINDOW, FN_CREATE_XBOX_VK_WINDOW, FN_EXECUTE_QUEUED_ACTION,
    FN_FOREGROUND_TIMER, FN_PROCESS_CONTROLLER_INPUT, FN_SPIRAL_VK_THREAD_ENTRY,
    FN_XBOX_VK_THREAD_ENTRY, G_APP_FEATURE_FLAGS, G_BOOT_SERVICE_MODE, G_FULLSCREEN_FG_FLAG,
    G_VK_OPEN_LATCH,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Desktop {
    Default,
    Winlogon,
}

impl fmt::Display for Desktop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Desktop::Default => write!(f, "default"),
            Desktop::Winlogon => write!(f, "winlogon"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Foreground {
    Normal,
    Uac,
    LogonUi,
    LockApp,
    Fullscreen,
}

impl fmt::Display for Foreground {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Foreground::Normal => write!(f, "normal app"),
            Foreground::Uac => write!(f, "UAC consent"),
            Foreground::LogonUi => write!(f, "LogonUI.exe"),
            Foreground::LockApp => write!(f, "LockApp.exe"),
            Foreground::Fullscreen => write!(f, "fullscreen app"),
        }
    }
}

#[derive(Debug)]
struct App {
    boot_mode: bool,
    config_winlogon_0xd9: bool,
    service_started: bool,
    attached_desktop: Desktop,
    input_desktop: Desktop,
    foreground: Foreground,
    fullscreen_profile_flag: bool,
    spiral_bit_9: bool,
    vk_open_latch: bool,
    action_latch: bool,
    modal_block_bit_4: bool,
    mask_0x200_active: bool,
    slot7_action_type: u16,
    slot7_subtype: u16,
    queued_action: u16,
    xbox_window_desktop: Option<Desktop>,
    spiral_window_desktop: Option<Desktop>,
    use_real_win32: bool,
    vk_session: Option<win::VkSession>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            boot_mode: false,
            config_winlogon_0xd9: false,
            service_started: false,
            attached_desktop: Desktop::Default,
            input_desktop: Desktop::Default,
            foreground: Foreground::Normal,
            fullscreen_profile_flag: false,
            spiral_bit_9: false,
            vk_open_latch: false,
            action_latch: false,
            modal_block_bit_4: false,
            mask_0x200_active: true,
            slot7_action_type: 6,
            slot7_subtype: 7,
            queued_action: 0,
            xbox_window_desktop: None,
            spiral_window_desktop: None,
            use_real_win32: false,
            vk_session: None,
        }
    }
}

impl App {
    fn start_normal(&mut self) {
        self.boot_mode = false;
        self.service_started = false;
        self.attach_named(Desktop::Default);
        self.log(&format!(
            "normal start: {FN_ATTACH_NAMED_DESKTOP}(\"default\")"
        ));
    }

    fn start_boot(&mut self) {
        self.boot_mode = true;
        self.service_started = true;
        self.attach_named(Desktop::Default);
        self.log(&format!(
            "boot start: -boot sets {G_BOOT_SERVICE_MODE}, service path active"
        ));
        if self.config_winlogon_0xd9 {
            self.attach_named(Desktop::Winlogon);
            self.log(&format!(
                "config +0xd9 set: {FN_ATTACH_NAMED_DESKTOP}(\"winlogon\")"
            ));
        } else {
            self.log("config +0xd9 clear: remains on default until OpenInputDesktop");
        }
    }

    /// Boot-service + config `+0xd9` path for sign-in / UAC (service or `--boot --cfg-winlogon`).
    ///
    /// When `+0xd9` is set, the main thread runs
    /// `warmup_attach_named_desktop("winlogon")` *first*, then creates
    /// the controller anchor window on that desktop. Owning a window
    /// on the input desktop is the gating condition for HID/XInput delivery.
    pub(crate) fn configure_boot_service(&mut self) {
        self.config_winlogon_0xd9 = true;
        self.boot_mode = true;
        self.service_started = true;
        self.foreground = Foreground::LogonUi;
        self.input_desktop = Desktop::Winlogon;
        if self.use_real_win32 {
            self.attach_named(Desktop::Winlogon);
            let cur = win::current_desktop_name().unwrap_or_else(|| "?".into());
            let input = win::input_desktop_name().unwrap_or_else(|e| format!("? ({e})"));
            self.service_log(&format!(
                "worker thread desktop: {cur}; input desktop: {input}"
            ));
        }
        self.log("service: boot path (controller thread on winlogon; VK UI follows input desktop)");
    }

    #[cfg(all(windows, feature = "service"))]
    fn service_log(&self, msg: &str) {
        if crate::config::service_mode() {
            crate::install::log_line(msg);
        }
    }

    #[cfg(not(all(windows, feature = "service")))]
    fn service_log(&self, _msg: &str) {}

    fn attach_named(&mut self, desktop: Desktop) {
        if self.use_real_win32 {
            let name = match desktop {
                Desktop::Default => "default",
                Desktop::Winlogon => "winlogon",
            };
            match win::attach_named(name) {
                Ok(()) => {
                    if let Some(cur) = win::current_desktop_name() {
                        self.log(&format!(
                            "{FN_ATTACH_NAMED_DESKTOP}(\"{name}\") → thread on {cur}"
                        ));
                    }
                }
                Err(e) => self.log(&format!(
                    "{FN_ATTACH_NAMED_DESKTOP}(\"{name}\") failed: {e}"
                )),
            }
        }
        self.attached_desktop = desktop;
        self.input_desktop = desktop;
    }

    /// `warmup_attach_input_desktop` — no boot gate in binary; non-`-boot` winlogon attach fails in practice.
    fn open_input_desktop(&mut self) -> bool {
        if self.input_desktop == Desktop::Winlogon && !self.boot_mode {
            self.log(&format!(
                "{FN_ATTACH_INPUT_DESKTOP}: OpenInputDesktop on winlogon needs {G_BOOT_SERVICE_MODE}"
            ));
            return false;
        }

        if self.use_real_win32 {
            match win::attach_input() {
                Ok(()) => {
                    let cur = win::current_desktop_name().unwrap_or_else(|| "input".into());
                    self.attached_desktop = self.input_desktop;
                    self.log(&format!("{FN_ATTACH_INPUT_DESKTOP}: attached to {cur}"));
                    return true;
                }
                Err(e) => {
                    self.log(&format!("{FN_ATTACH_INPUT_DESKTOP} failed: {e}"));
                    return false;
                }
            }
        }

        self.attached_desktop = self.input_desktop;
        self.log(&format!(
            "{FN_ATTACH_INPUT_DESKTOP}: simulated attach to {}",
            self.input_desktop
        ));
        true
    }

    fn set_foreground(&mut self, fg: Foreground) {
        self.foreground = fg;
        self.input_desktop = match fg {
            Foreground::Uac | Foreground::LogonUi | Foreground::LockApp => Desktop::Winlogon,
            Foreground::Normal | Foreground::Fullscreen => Desktop::Default,
        };
        self.timer_100ms();
    }

    fn timer_100ms(&mut self) {
        self.fullscreen_profile_flag = match self.foreground {
            Foreground::LogonUi | Foreground::LockApp => false,
            Foreground::Fullscreen => true,
            Foreground::Normal | Foreground::Uac => false,
        };
        self.log(&format!(
            "{FN_FOREGROUND_TIMER}: profile/fullscreen detection only; no VK auto-show"
        ));
    }

    /// Snapshot the App state the gate needs. The [`config::service_mode`] read
    /// is the only impure part; `vk_gate::decide` itself reads nothing.
    fn gate_input(&self) -> vk_gate::GateInput {
        vk_gate::GateInput {
            mask_0x200_active: self.mask_0x200_active,
            slot7_action_type: self.slot7_action_type,
            slot7_subtype: self.slot7_subtype,
            vk_open: self.vk_session.is_some(),
            modal_block_bit_4: self.modal_block_bit_4,
            spiral_bit_9: self.spiral_bit_9,
            service_mode: crate::config::service_mode(),
            input_desktop: self.input_desktop,
        }
    }

    /// Y/Triangle press toggles VK — stays open until next press. The decision
    /// lives in [`vk_gate::decide`]; this method only enacts it (logs, latches,
    /// thread spawns).
    fn toggle_virtual_keyboard_combo(&mut self) {
        use vk_gate::{Blocked, VkAction};

        let input = self.gate_input();
        let action = vk_gate::decide(input);

        if action == VkAction::Blocked(Blocked::MaskAbsent) {
            self.log("button: mask 0x200 absent -> slot 7 not resolved");
            self.service_log("Y tap: mask 0x200 inactive");
            return;
        }

        self.service_log("Y tap: toggle VK");

        // `run_slot7_binding` log: slot 7 either queues action 7 or it doesn't.
        if input.slot7_action_type == 6 {
            self.log(&format!(
                "{FN_PROCESS_CONTROLLER_INPUT} -> {FN_APPLY_MASK_SLOT_ACTION}: mask 0x200 slot 7, action 7 queued"
            ));
        } else {
            self.log("slot 7 exists, but action type is not queueing VK action");
        }

        match action {
            VkAction::Blocked(Blocked::MaskAbsent) => unreachable!("handled above"),
            VkAction::Blocked(Blocked::SlotNotQueueing | Blocked::QueuedNotSeven) => {
                self.log(&format!(
                    "{FN_EXECUTE_QUEUED_ACTION}: queued action != 7 -> no VK path"
                ));
                self.service_log("VK toggle: queued action != 7");
            }
            VkAction::Close => self.close_vk(),
            VkAction::Blocked(Blocked::ModalBit4) => {
                self.log("blocked: app state bit 4 set");
                self.service_log("VK toggle: blocked (modal bit 4)");
            }
            VkAction::OpenSpiral => self.open_spiral_vk(),
            VkAction::OpenXbox { attach } => self.open_xbox_vk(attach),
        }
    }

    fn release_virtual_keyboard_combo(&mut self) {
        self.action_latch = false;
        self.log(&format!(
            "{}: release (VK stays open until next Y tap)",
            symbols::FN_ON_CONTROLLER_RELEASE
        ));
    }

    fn close_vk(&mut self) {
        self.vk_open_latch = false;
        self.action_latch = false;
        self.modal_block_bit_4 = false;
        self.xbox_window_desktop = None;
        self.spiral_window_desktop = None;
        #[cfg(windows)]
        crate::vk_nav::reset_selection();
        if let Some(session) = self.vk_session.take() {
            let kind = session.describe();
            session.close();
            self.log(&format!("VK closed ({kind})"));
        } else {
            self.log(&format!("{G_VK_OPEN_LATCH} cleared -> VK closes"));
        }
    }

    fn open_xbox_vk(&mut self, attach: vk_gate::GateAttach) {
        if !self.open_vk_common(&format!(
            "{FN_CREATE_XBOX_VK_WINDOW} / {FN_XBOX_VK_THREAD_ENTRY}"
        )) {
            return;
        }
        if self.use_real_win32 {
            // Attach desktop was decided by the gate (lock/logon/UAC and the
            // service path need OpenInputDesktop on the UI thread).
            let attach = match attach {
                vk_gate::GateAttach::Input => win::VkAttach::Input,
                vk_gate::GateAttach::Current => win::VkAttach::Current,
            };
            match win::VkSession::open(attach) {
                Ok(session) => {
                    let kind = session.describe();
                    self.vk_open_latch = true;
                    self.log(&format!("VK shown: {kind}"));
                    #[cfg(windows)]
                    crate::vk_nav::reset_selection();
                    self.vk_session = Some(session);
                }
                Err(e) => {
                    self.action_latch = false;
                    self.modal_block_bit_4 = false;
                    self.log(&format!("VK failed: {e}"));
                    #[cfg(windows)]
                    if crate::config::service_mode() {
                        install::log_line(&format!("VK failed: {e}"));
                    }
                }
            }
        } else {
            self.vk_open_latch = true;
        }
        self.xbox_window_desktop = Some(self.attached_desktop);
        self.spiral_window_desktop = None;
    }

    fn open_spiral_vk(&mut self) {
        if !self.open_vk_common(&format!(
            "{FN_SPIRAL_VK_THREAD_ENTRY} / {FN_CREATE_SPIRAL_VK_WINDOW}: SpiralVkWindow ({G_APP_FEATURE_FLAGS} bit 9)"
        )) {
            return;
        }
        self.spiral_window_desktop = Some(self.attached_desktop);
        self.xbox_window_desktop = None;
    }

    /// `warmup_execute_queued_action` case 7: `g_vk_window_open_latch`, per-slot latch, state bit 4, thread.
    fn open_vk_common(&mut self, label: &str) -> bool {
        self.action_latch = true;

        if self.use_real_win32 {
            // Native VK thread does its own `OpenInputDesktop` when needed.
            self.modal_block_bit_4 = true;
            self.log(label);
            return true;
        }

        if !self.open_input_desktop() {
            self.action_latch = false;
            return false;
        }

        self.modal_block_bit_4 = true;
        self.vk_open_latch = true;
        self.log(label);
        true
    }

    fn log(&self, msg: &str) {
        repl_scroll::note_line();
        println!("> {msg}");
    }

    /// One screen line per entry (incl. leading/trailing blank lines).
    fn state_lines(&self) -> Vec<String> {
        let mut v = Vec::with_capacity(24);
        v.push(String::new());
        v.push("STATE".into());
        v.push(format!(
            "  boot_mode {G_BOOT_SERVICE_MODE:28} : {}",
            self.boot_mode
        ));
        v.push(format!(
            "  service WarmupSvc          : {}",
            self.service_started
        ));
        v.push(format!(
            "  config +0xd9 winlogon      : {}",
            self.config_winlogon_0xd9
        ));
        v.push(format!(
            "  attached desktop           : {}",
            self.attached_desktop
        ));
        v.push(format!(
            "  input desktop              : {}",
            self.input_desktop
        ));
        v.push(format!(
            "  foreground                 : {}",
            self.foreground
        ));
        v.push(format!(
            "  {G_FULLSCREEN_FG_FLAG:28} : {}",
            self.fullscreen_profile_flag
        ));
        v.push(format!(
            "  {G_APP_FEATURE_FLAGS} bit 9 Spiral  : {}",
            self.spiral_bit_9
        ));
        v.push(format!("  {G_VK_OPEN_LATCH:28} : {}", self.vk_open_latch));
        v.push(format!(
            "  state[0x2c] bit 4 block    : {}",
            self.modal_block_bit_4
        ));
        v.push(format!(
            "  mask bit 0x200 active      : {}",
            self.mask_0x200_active
        ));
        v.push(format!(
            "  slot7 type/subtype         : {}/{}",
            self.slot7_action_type, self.slot7_subtype
        ));
        v.push(format!(
            "  queued action              : {}",
            self.queued_action
        ));
        v.push(format!(
            "  Xbox window                : {}",
            match self.xbox_window_desktop {
                Some(d) => format!("visible on {d}"),
                None => "not visible".into(),
            }
        ));
        v.push(format!(
            "  Spiral window              : {}",
            match self.spiral_window_desktop {
                Some(d) => format!("visible on {d}"),
                None => "not visible".into(),
            }
        ));
        v.push(format!(
            "  real Win32 (--real)        : {}",
            self.use_real_win32
        ));
        v.push(format!(
            "  OS keyboard session        : {}",
            self.vk_session
                .as_ref()
                .map(|s| s.describe())
                .unwrap_or("none")
        ));
        #[cfg(windows)]
        v.push(format!(
            "  VK window visible          : {}",
            win::is_vk_visible()
        ));
        v.push(String::new());
        v
    }
}

/// Count `println!` lines after last STATE panel so CUU can reach panel top (ANSI).
mod repl_scroll {
    use std::cell::{Cell, RefCell};

    use super::{io, App, Write};

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static AFTER_STATE: Cell<u32> = const { Cell::new(0) };
        static LAST_STATE_LINES: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    }

    pub fn enable(y: bool) {
        ENABLED.with(|e| e.set(y));
    }

    pub fn note_line() {
        ENABLED.with(|en| {
            if en.get() {
                AFTER_STATE.with(|a| a.set(a.get().saturating_add(1)));
            }
        });
    }

    pub fn note_lines(n: u32) {
        ENABLED.with(|en| {
            if en.get() {
                AFTER_STATE.with(|a| a.set(a.get().saturating_add(n)));
            }
        });
    }

    fn take_after_lines() -> u32 {
        AFTER_STATE.with(|a| {
            let n = a.get();
            a.set(0);
            n
        })
    }

    /// Repaint STATE block in place: CUU to panel top, rewrite only changed rows.
    pub fn paint_state_panel(app: &App) {
        let enabled = ENABLED.with(|e| e.get());
        if !enabled {
            let lines = app.state_lines();
            for line in &lines {
                println!("{line}");
            }
            return;
        }

        let since = take_after_lines();
        let new = app.state_lines();
        let prev = LAST_STATE_LINES.with(|cell| cell.borrow_mut().take());

        if let Some(ref p) = prev {
            let up = p.len() as u32 + since;
            print!("\x1b[{up}A");
        }

        let max = prev
            .as_ref()
            .map_or(0, |p: &Vec<String>| p.len())
            .max(new.len());
        for i in 0..max {
            print!("\r");
            let old = prev.as_ref().and_then(|p: &Vec<String>| p.get(i));
            let nw = new.get(i);
            if old == nw && nw.is_some() {
                print!("\x1b[1B");
            } else {
                print!("\x1b[2K");
                if let Some(s) = nw {
                    print!("{s}");
                }
                if i + 1 < max {
                    println!();
                }
            }
        }

        LAST_STATE_LINES.with(|cell| {
            *cell.borrow_mut() = Some(new);
        });
        let _ = io::stdout().flush();
    }
}

fn main() {
    let _sentry = sentry_telemetry::init();
    let args: Vec<String> = env::args().collect();
    #[cfg(windows)]
    dispatch_install_or_service(&args);
    #[cfg(all(windows, feature = "gamepad"))]
    tray::spawn();

    let use_real_win32 = args.iter().any(|a| a == "--real")
        || env::var_os("WARMUP_REAL_VK").is_some_and(|v| v != "0");
    if args.iter().any(|a| a == "--gamepad") {
        #[cfg(feature = "gamepad")]
        {
            return run_gamepad_mode();
        }
        #[cfg(not(feature = "gamepad"))]
        {
            eprintln!("Rebuild with: cargo run -- --gamepad");
            std::process::exit(1);
        }
    }

    let mut app = App::default();
    app.use_real_win32 = use_real_win32;
    println!("Warmup UAC/sign-in + Xbox VK prototype");
    if use_real_win32 {
        println!("--real: WarmupXboxVkWindow (native UI thread)");
    }
    println!("Type `help` for commands. State prints after each command.");
    #[cfg(feature = "gamepad")]
    println!("Gamepad: `pad` or `cargo run -- --gamepad` (warmUP SDL3 crate)");
    repl_scroll::enable(true);
    repl_scroll::paint_state_panel(&app);

    loop {
        print!("warmup> ");
        io::stdout().flush().ok();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            break;
        }
        repl_scroll::note_line();

        let cmd = line.trim().to_ascii_lowercase();
        if cmd.is_empty() {
            repl_scroll::paint_state_panel(&app);
            continue;
        }
        match cmd.as_str() {
            "help" => print_help(),
            "state" => {}
            "normal" => app.start_normal(),
            "boot" => app.start_boot(),
            "cfg winlogon on" => {
                app.config_winlogon_0xd9 = true;
                repl_scroll::note_line();
                println!("> config +0xd9 set");
            }
            "cfg winlogon off" => {
                app.config_winlogon_0xd9 = false;
                repl_scroll::note_line();
                println!("> config +0xd9 clear");
            }
            "fg normal" => app.set_foreground(Foreground::Normal),
            "fg uac" => app.set_foreground(Foreground::Uac),
            "fg logon" => app.set_foreground(Foreground::LogonUi),
            "fg lock" => app.set_foreground(Foreground::LockApp),
            "fg fullscreen" => app.set_foreground(Foreground::Fullscreen),
            "attach input" => {
                let _ = app.open_input_desktop();
            }
            "press" => app.toggle_virtual_keyboard_combo(),
            "release" => app.release_virtual_keyboard_combo(),
            "spiral on" => {
                app.spiral_bit_9 = true;
                repl_scroll::note_line();
                println!("> {G_APP_FEATURE_FLAGS} bit 9: {FN_EXECUTE_QUEUED_ACTION} use_spiral=1");
            }
            "spiral off" => {
                app.spiral_bit_9 = false;
                repl_scroll::note_line();
                println!("> bit 9 clear: {FN_EXECUTE_QUEUED_ACTION} use_spiral=0 -> Xbox");
            }
            "block on" => {
                app.modal_block_bit_4 = true;
                repl_scroll::note_line();
                println!("> state[0x2c] bit 4 set");
            }
            "block off" => {
                app.modal_block_bit_4 = false;
                repl_scroll::note_line();
                println!("> state[0x2c] bit 4 clear");
            }
            "mask on" => {
                app.mask_0x200_active = true;
                repl_scroll::note_line();
                println!("> mask 0x200 active");
            }
            "mask off" => {
                app.mask_0x200_active = false;
                repl_scroll::note_line();
                println!("> mask 0x200 inactive");
            }
            "slot good" => {
                app.slot7_action_type = 6;
                app.slot7_subtype = 7;
                repl_scroll::note_line();
                println!("> slot 7 queues action 7");
            }
            "slot bad" => {
                app.slot7_action_type = 1;
                app.slot7_subtype = 0;
                repl_scroll::note_line();
                println!("> slot 7 no longer queues action 7");
            }
            "reset" => {
                let real = app.use_real_win32;
                app = App::default();
                app.use_real_win32 = real;
                repl_scroll::note_line();
                println!("> reset");
            }
            "quit" | "exit" => break,
            #[cfg(feature = "gamepad")]
            "pad" => match gamepad::GamepadPoll::open().and_then(|mut g| g.snapshot()) {
                Ok(s) => {
                    repl_scroll::note_line();
                    println!("> {s}");
                }
                Err(e) => {
                    repl_scroll::note_line();
                    println!("> gamepad error: {e}");
                }
            },
            other => {
                #[cfg(feature = "gamepad")]
                if other == "gamepad" {
                    repl_scroll::note_line();
                    println!("> use: cargo run -- --gamepad");
                } else {
                    repl_scroll::note_line();
                    println!("> unknown command: {other}");
                }
                #[cfg(not(feature = "gamepad"))]
                {
                    repl_scroll::note_line();
                    println!("> unknown command: {other}");
                }
            }
        }

        repl_scroll::paint_state_panel(&app);
    }
}

#[cfg(windows)]
fn has_interactive_console() -> bool {
    use windows::Win32::System::Console::GetConsoleWindow;
    unsafe {
        let hwnd = GetConsoleWindow();
        !hwnd.0.is_null()
    }
}

#[cfg(windows)]
fn dispatch_install_or_service(args: &[String]) {
    // Mic recognition runs here, as the real logged-in user (the worker spawns us
    // via CreateProcessAsUserW). Short-lived: recognize until silence, then exit.
    if args.iter().any(|a| a == "--speech-helper") {
        let code = match crate::win::speech_input::run_blocking() {
            Ok(()) => 0,
            Err(e) => {
                install::log_line(&format!("speech helper failed: {e}"));
                1
            }
        };
        std::process::exit(code);
    }
    match args.get(1).map(String::as_str) {
        Some("install") => {
            let debug_ui = args.iter().any(|a| a == "--debug-ui" || a == "--debug");
            install::run_install(debug_ui);
            std::process::exit(0);
        }
        Some("uninstall") => {
            install::run_uninstall();
            std::process::exit(0);
        }
        Some("verify") => {
            install::run_verify();
            std::process::exit(0);
        }
        Some("stop") => {
            install::run_stop();
            std::process::exit(0);
        }
        Some("restore-keyboard") | Some("restore-native-keyboard") => {
            crate::win::native_keyboard::restore_auto_invoke();
            crate::win::native_keyboard::ensure_search_service_running();
            install::log_line("restore-keyboard: requested Windows keyboard service restore");
            println!("Requested Windows touch keyboard/search service restore.");
            std::process::exit(0);
        }
        #[cfg(feature = "gamepad")]
        Some("settings") => {
            run_settings_command(args);
            std::process::exit(0);
        }
        _ => {}
    }
    #[cfg(feature = "service")]
    {
        if args.iter().any(|a| a == "--service-worker") {
            #[cfg(feature = "gamepad")]
            tray::spawn();
            match service::run_worker() {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    install::log_line(&format!("service worker failed: {e}"));
                    std::process::exit(1);
                }
            }
        }
        let scm_start = args.len() <= 1 && !has_interactive_console();
        let force_service = args.iter().any(|a| a == "--service");
        if scm_start || force_service {
            if service::run_dispatcher().is_ok() {
                std::process::exit(0);
            } else if force_service {
                install::log_line("--service: not running under SCM");
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(feature = "service"))]
    if args.iter().any(|a| a == "--service") {
        eprintln!("Rebuild with default features enabled: cargo build --release");
        std::process::exit(1);
    }
}

#[cfg(all(windows, feature = "gamepad"))]
fn run_settings_command(args: &[String]) {
    let usage = "usage:
  warmup-companion.exe settings get
  warmup-companion.exe settings path
  warmup-companion.exe settings set <key> <value>
  warmup-companion.exe settings sleep-on-game <get|on|off>
  warmup-companion.exe settings auto-stop-on-game <get|on|off>
  warmup-companion.exe settings userland-poll <get|full|sleep|path>";
    match args.get(2).map(String::as_str) {
        Some("get") | None => print_gamepad_settings(),
        Some("path") => match crate::config::settings_path() {
            Some(path) => println!("{}", path.display()),
            None => {
                eprintln!("LOCALAPPDATA is not set");
                std::process::exit(1);
            }
        },
        Some("set") => {
            let Some(key) = args.get(3) else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            let Some(value) = args.get(4) else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            if let Err(e) = crate::config::set_gamepad_setting(key, value) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!("{key}={value}");
        }
        Some("userland-poll") => match args.get(3).map(String::as_str) {
            Some("get") | None => {
                let mode = crate::config::userland_gamepad_poll_mode();
                println!("{}", poll_mode_name(mode));
            }
            Some("full") => {
                if let Err(e) =
                    crate::config::set_userland_gamepad_poll_mode(warmup_gamepad::PollMode::Full)
                {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("full");
            }
            Some("sleep") | Some("guide") | Some("guide-only") => {
                if let Err(e) =
                    crate::config::set_userland_gamepad_poll_mode(warmup_gamepad::PollMode::Sleep)
                {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("sleep");
            }
            Some("path") => match crate::config::settings_path() {
                Some(path) => println!("{}", path.display()),
                None => {
                    eprintln!("LOCALAPPDATA is not set");
                    std::process::exit(1);
                }
            },
            Some(_) => {
                eprintln!("{usage}");
                std::process::exit(2);
            }
        },
        Some("sleep-on-game") => match args.get(3).map(String::as_str) {
            Some("get") | None => {
                println!("{}", crate::config::gamepad_settings().sleep_on_game);
            }
            Some("on") | Some("true") | Some("1") => {
                if let Err(e) = crate::config::set_gamepad_setting("sleep_on_game", "true") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("true");
            }
            Some("off") | Some("false") | Some("0") => {
                if let Err(e) = crate::config::set_gamepad_setting("sleep_on_game", "false") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("false");
            }
            Some(_) => {
                eprintln!("{usage}");
                std::process::exit(2);
            }
        },
        Some("auto-stop-on-game") => match args.get(3).map(String::as_str) {
            Some("get") | None => {
                println!("{}", crate::config::gamepad_settings().auto_stop_on_game);
            }
            Some("on") | Some("true") | Some("1") => {
                if let Err(e) = crate::config::set_gamepad_setting("auto_stop_on_game", "true") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("true");
            }
            Some("off") | Some("false") | Some("0") => {
                if let Err(e) = crate::config::set_gamepad_setting("auto_stop_on_game", "false") {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("false");
            }
            Some(_) => {
                eprintln!("{usage}");
                std::process::exit(2);
            }
        },
        Some(_) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }
}

#[cfg(all(windows, feature = "gamepad"))]
fn poll_mode_name(mode: warmup_gamepad::PollMode) -> &'static str {
    match mode {
        warmup_gamepad::PollMode::Full => "full",
        warmup_gamepad::PollMode::Sleep => "sleep",
    }
}

#[cfg(all(windows, feature = "gamepad"))]
fn print_gamepad_settings() {
    let s = crate::config::gamepad_settings();
    println!("userland_poll={}", poll_mode_name(s.userland_poll_mode));
    println!("sleep_on_game={}", s.sleep_on_game);
    println!("auto_stop_on_game={}", s.auto_stop_on_game);
    println!("cursor_deadzone={}", s.cursor_deadzone);
    println!("cursor_speed={}", s.cursor_speed);
    println!("cursor_accel={}", s.cursor_accel);
    println!("scroll_deadzone={}", s.scroll_deadzone);
    println!("scroll_speed={}", s.scroll_speed);
    println!("scroll_accel={}", s.scroll_accel);
}

#[cfg(feature = "gamepad")]
fn run_gamepad_mode() {
    let use_real = env::args().any(|a| a == "--real")
        || env::var_os("WARMUP_REAL_VK").is_some_and(|v| v != "0")
        || cfg!(windows);
    let args: Vec<String> = env::args().collect();
    let mut app = App::default();
    app.use_real_win32 = use_real;
    if args.iter().any(|a| a == "--boot") {
        app.start_boot();
        println!("> --boot: service path + {G_BOOT_SERVICE_MODE}");
    }
    if args
        .iter()
        .any(|a| a == "--cfg-winlogon" || a == "--winlogon")
    {
        app.config_winlogon_0xd9 = true;
        println!("> --cfg-winlogon: config +0xd9 set");
        if app.boot_mode {
            app.attach_named(Desktop::Winlogon);
        }
    }
    println!("Warmup Companion gamepad mode — sticks move mouse; L3 toggles VK");
    if use_real {
        println!("real Win32 VK enabled (WarmupXboxVkWindow)");
    }
    println!("Sign-in service: build default release, then `install` as Admin");
    repl_scroll::paint_state_panel(&app);
    let vk_open = std::cell::Cell::new(false);
    let result = run_boot_gamepad_loop(&mut app, &vk_open, false);
    if let Some(session) = app.vk_session.take() {
        session.close();
    }
    match result {
        Ok(()) => println!("> exited"),
        Err(e) => {
            eprintln!("gamepad: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "gamepad")]
pub(crate) fn run_boot_gamepad_loop(
    app: &mut App,
    vk_open: &std::cell::Cell<bool>,
    service_mode: bool,
) -> Result<(), String> {
    // The companion owns the device; host the pipe so the warmUP desktop can read
    // connection state over IPC (#347). No-op on non-Windows.
    crate::pipe_server::spawn();
    let on_action = |action: gamepad::VkLoopAction| match action {
        gamepad::VkLoopAction::Toggle => {
            app.toggle_virtual_keyboard_combo();
            vk_open.set(app.vk_session.is_some());
            if !service_mode {
                repl_scroll::paint_state_panel(&*app);
            } else {
                #[cfg(windows)]
                {
                    if app.vk_session.is_some() {
                        let vis = win::is_vk_visible();
                        install::log_line(&format!("VK opened (window visible={vis})"));
                    } else {
                        install::log_line("VK closed");
                    }
                }
            }
        }
        gamepad::VkLoopAction::Close => {
            app.close_vk();
            vk_open.set(false);
            if !service_mode {
                repl_scroll::paint_state_panel(&*app);
            } else {
                #[cfg(windows)]
                install::log_line("VK closed");
            }
        }
        gamepad::VkLoopAction::Reopen => {
            app.close_vk();
            let attach = vk_gate::attach_for(app.gate_input());
            app.open_xbox_vk(attach);
            vk_open.set(app.vk_session.is_some());
            if !service_mode {
                repl_scroll::paint_state_panel(&*app);
            } else {
                #[cfg(windows)]
                {
                    if app.vk_session.is_some() {
                        let vis = win::is_vk_visible();
                        install::log_line(&format!("VK reopened (window visible={vis})"));
                    } else {
                        install::log_line("VK reopen failed");
                    }
                }
            }
        }
        gamepad::VkLoopAction::LaunchWarmup => {
            if let Err(e) = launch_warmup_exe() {
                eprintln!("launch warmup.exe: {e}");
                #[cfg(windows)]
                if service_mode {
                    install::log_line(&format!("launch warmup.exe failed: {e}"));
                }
            } else {
                #[cfg(windows)]
                if service_mode {
                    install::log_line("launched warmup.exe from controller hotkey");
                }
            }
        }
    };
    if service_mode {
        gamepad::run_watch_loop_service(|| vk_open.get(), on_action)
    } else {
        gamepad::run_watch_loop(|| vk_open.get(), on_action)
    }
}

#[cfg(feature = "gamepad")]
fn launch_warmup_exe() -> Result<(), String> {
    let exe = warmup_exe_path()?;
    spawn_warmup(&exe).map_err(|e| format!("{}: {e}", exe.display()))
}

#[cfg(feature = "gamepad")]
fn warmup_exe_path() -> Result<std::path::PathBuf, String> {
    if let Some(path) = std::env::var_os("WARMUP_EXE") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("WARMUP_EXE does not exist: {}", path.display()));
    }

    if let Ok(raw) = std::fs::read_to_string(r"C:\ProgramData\WarmupVk\warmup-exe.path") {
        let path = std::path::PathBuf::from(raw.trim().trim_matches('"'));
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            r"C:\ProgramData\WarmupVk\warmup-exe.path points to missing exe: {}",
            path.display()
        ));
    }

    let current = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let dir = current
        .parent()
        .ok_or_else(|| format!("current exe has no parent: {}", current.display()))?;
    let mut candidates = Vec::new();
    candidates.push(dir.join("warmup.exe"));
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(std::path::PathBuf::from(program_files).join(r"warmUP\warmup.exe"));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(std::path::PathBuf::from(program_files_x86).join(r"warmUP\warmup.exe"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let local_app_data = std::path::PathBuf::from(local_app_data);
        candidates.push(local_app_data.join(r"dev.warmup.console\warmup.exe"));
        candidates.push(local_app_data.join(r"warmUP\warmup.exe"));
        candidates.push(local_app_data.join(r"Programs\warmUP\warmup.exe"));
    }
    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        candidates.push(
            std::path::PathBuf::from(user_profile)
                .join(r"warmUp\apps\desktop\src-tauri\target\debug\warmup.exe"),
        );
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "warmup.exe not found; set WARMUP_EXE or write the full path to {}",
                r"C:\ProgramData\WarmupVk\warmup-exe.path"
            )
        })
}

#[cfg(all(feature = "gamepad", windows))]
fn spawn_warmup(exe: &std::path::Path) -> std::io::Result<()> {
    if crate::config::service_mode() {
        return spawn_warmup_as_active_user(exe);
    }

    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
}

#[cfg(all(feature = "gamepad", windows))]
fn spawn_warmup_as_active_user(exe: &std::path::Path) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
        DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
    };

    fn wide_os(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    unsafe {
        let session_id = WTSGetActiveConsoleSessionId();
        let mut token = Default::default();
        WTSQueryUserToken(session_id, &mut token)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let exe_w = wide_os(exe.as_os_str());
        let mut cmd_w = wide(&format!("\"{}\"", exe.display()));
        let cwd_w = exe.parent().map(|parent| wide_os(parent.as_os_str()));
        let mut desktop = wide("winsta0\\default");
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        let mut env = std::ptr::null_mut();
        let env_created = CreateEnvironmentBlock(&mut env, token, false).is_ok();
        let env_arg = if env_created {
            Some(env.cast_const().cast())
        } else {
            None
        };
        let cwd_arg = cwd_w
            .as_ref()
            .map(|cwd| PCWSTR(cwd.as_ptr()))
            .unwrap_or_else(PCWSTR::null);
        let mut flags = CREATE_UNICODE_ENVIRONMENT
            | PROCESS_CREATION_FLAGS(DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0);
        if !env_created {
            flags = PROCESS_CREATION_FLAGS(DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0);
        }

        let created = CreateProcessAsUserW(
            token,
            PCWSTR(exe_w.as_ptr()),
            PWSTR(cmd_w.as_mut_ptr()),
            None,
            None,
            false,
            flags,
            env_arg,
            cwd_arg,
            &startup,
            &mut info,
        );
        if env_created {
            let _ = DestroyEnvironmentBlock(env);
        }
        let _ = CloseHandle(token);
        created.map_err(|e| std::io::Error::other(e.to_string()))?;
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(())
}

#[cfg(all(feature = "gamepad", not(windows)))]
fn spawn_warmup(exe: &std::path::Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    cmd.spawn().map(|_| ())
}

fn help_screen_rows(help: &str) -> u32 {
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(120)
        .max(40);
    let mut rows = 0u32;
    for line in help.split('\n') {
        let w = line.chars().count() as u32;
        rows = rows.saturating_add(if w == 0 { 1 } else { w.div_ceil(cols) });
    }
    // println! adds one '\n' after HELP → cursor sits on following row
    rows.saturating_add(1)
}

fn print_help() {
    const HELP: &str = r#"COMMANDS
  normal              start normal user instance on default desktop
  cfg winlogon on     set config.bin +0xd9
  cfg winlogon off    clear config.bin +0xd9
  boot                start -boot service path
  fg normal           foreground normal user app
  fg uac              foreground UAC consent, input desktop winlogon
  fg logon            foreground LogonUI.exe, input desktop winlogon
  fg lock             foreground LockApp.exe, input desktop winlogon
  fg fullscreen       foreground fullscreen app, profile flag on
  attach input        warmup_attach_input_desktop
  press               mask 0x200 -> warmup_process_controller_input
  release             warmup_on_controller_release
  spiral on/off       g_app_feature_flags bit 9 -> Spiral vs Xbox path
  block on/off        toggle state[0x2c] bit 4
  mask on/off         toggle physical mask bit 0x200
  slot good           slot 7 type 6 subtype 7 -> queue action 7
  slot bad            slot 7 does not queue action 7
  reset               reset state
  quit                exit
  --real              Win32 desktop + TabTip/WarmupXboxVkWindow (Windows)
  pad                 (gamepad feature) SDL3 snapshot
  --gamepad           (gamepad feature) sticks + L3 → VK

SCENARIOS
  normal -> fg uac -> press
  cfg winlogon on -> boot -> fg logon -> press
  cfg winlogon on -> boot -> fg uac -> press -> press
"#;
    repl_scroll::note_lines(help_screen_rows(HELP));
    println!("{HELP}");
}
