mod symbols;

#[cfg(windows)]
mod install;
#[cfg(all(windows, feature = "service"))]
mod service;

#[cfg(windows)]
mod debug_state;
#[cfg(windows)]
mod vk_nav;
#[cfg(windows)]
mod win;
#[cfg(not(windows))]
#[path = "win_stub.rs"]
mod win;

#[cfg(feature = "gamepad")]
mod gamepad;
#[cfg(feature = "gamepad")]
mod gamepad_backend;
#[cfg(feature = "gamepad")]
mod pc_cursor;
#[cfg(all(windows, feature = "gamepad"))]
mod xinput_backend;

use std::env;
use std::fmt;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// Ignore rapid Y/Triangle bounce (down-up-down) that toggles open→close in one gesture.
const VK_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(400);
const VK_TOGGLE_DEBOUNCE_SERVICE: Duration = Duration::from_millis(900);

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
    last_vk_toggle: Option<Instant>,
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
            last_vk_toggle: None,
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

    /// Joyxoff `-boot` + config `+0xd9` for sign-in / UAC (service or `--boot --cfg-winlogon`).
    pub(crate) fn configure_boot_service(&mut self) {
        self.config_winlogon_0xd9 = true;
        self.boot_mode = true;
        self.service_started = true;
        self.foreground = Foreground::LogonUi;
        self.input_desktop = Desktop::Winlogon;
        if self.use_real_win32 {
            let cur = win::current_desktop_name().unwrap_or_else(|| "?".into());
            let input = win::input_desktop_name().unwrap_or_else(|e| format!("? ({e})"));
            self.service_log(&format!(
                "worker thread desktop: {cur}; input desktop: {input}"
            ));
        }
        self.log("service: boot path (input desktop follows lock/logon/UAC)");
    }

    #[cfg(all(windows, feature = "service"))]
    fn service_log(&self, msg: &str) {
        if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
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

    /// Y/Triangle **tap** (release edge) toggles VK — stays open until next tap.
    fn toggle_virtual_keyboard_combo(&mut self) {
        if !self.mask_0x200_active {
            self.log("button: mask 0x200 absent -> slot 7 not resolved");
            self.service_log("Y tap: mask 0x200 inactive");
            return;
        }

        let now = Instant::now();
        let debounce = if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
            VK_TOGGLE_DEBOUNCE_SERVICE
        } else {
            VK_TOGGLE_DEBOUNCE
        };
        if let Some(last) = self.last_vk_toggle {
            if now.duration_since(last) < debounce {
                self.log("VK toggle debounced (button bounce)");
                self.service_log("Y tap: debounced");
                return;
            }
        }
        self.last_vk_toggle = Some(now);
        self.service_log("Y tap: toggle VK");
        #[cfg(windows)]
        crate::debug_state::record_action("Y tap: toggle VK");

        self.run_slot7_binding();
        self.dispatch_vk_toggle();
    }

    fn release_virtual_keyboard_combo(&mut self) {
        self.action_latch = false;
        self.log(&format!(
            "{}: release (VK stays open until next Y tap)",
            symbols::FN_ON_CONTROLLER_RELEASE
        ));
    }

    fn run_slot7_binding(&mut self) {
        if self.slot7_action_type == 6 {
            self.queued_action = self.slot7_subtype;
            self.log(&format!(
                "{FN_PROCESS_CONTROLLER_INPUT} -> {FN_APPLY_MASK_SLOT_ACTION}: mask 0x200 slot 7, action 7 queued"
            ));
        } else {
            self.queued_action = 0;
            self.log("slot 7 exists, but action type is not queueing VK action");
        }
    }

    fn dispatch_vk_toggle(&mut self) {
        if self.queued_action != 7 {
            self.log(&format!(
                "{FN_EXECUTE_QUEUED_ACTION}: queued action != 7 -> no VK path"
            ));
            self.service_log("VK toggle: queued action != 7");
            return;
        }

        if self.vk_session.is_some() {
            self.close_vk();
            return;
        }

        if self.modal_block_bit_4 {
            self.log("blocked: app state bit 4 set");
            self.service_log("VK toggle: blocked (modal bit 4)");
            return;
        }

        if self.spiral_bit_9 {
            self.open_spiral_vk();
        } else {
            self.open_xbox_vk();
        }
    }

    fn close_vk(&mut self) {
        self.vk_open_latch = false;
        self.action_latch = false;
        self.modal_block_bit_4 = false;
        self.xbox_window_desktop = None;
        self.spiral_window_desktop = None;
        if let Some(session) = self.vk_session.take() {
            let kind = session.describe();
            session.close();
            self.log(&format!("VK closed ({kind})"));
            #[cfg(windows)]
            crate::debug_state::record_action(format!("VK closed ({kind})"));
        } else {
            self.log(&format!("{G_VK_OPEN_LATCH} cleared -> VK closes"));
            #[cfg(windows)]
            crate::debug_state::record_action("VK close requested");
        }
    }

    fn open_xbox_vk(&mut self) {
        if !self.open_vk_common(&format!(
            "{FN_CREATE_XBOX_VK_WINDOW} / {FN_XBOX_VK_THREAD_ENTRY}"
        )) {
            return;
        }
        if self.use_real_win32 {
            let attach = if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
                // Lock screen (Win+L), logon, and UAC all need OpenInputDesktop on the UI thread.
                win::VkAttach::Input
            } else {
                match self.input_desktop {
                    Desktop::Winlogon => win::VkAttach::Input,
                    Desktop::Default => win::VkAttach::Current,
                }
            };
            match win::VkSession::open(attach) {
                Ok(session) => {
                    let kind = session.describe();
                    self.vk_open_latch = true;
                    self.log(&format!("VK shown: {kind}"));
                    #[cfg(windows)]
                    crate::debug_state::record_action(format!("VK shown: {kind}"));
                    self.vk_session = Some(session);
                }
                Err(e) => {
                    self.action_latch = false;
                    self.modal_block_bit_4 = false;
                    self.log(&format!("VK failed: {e}"));
                    #[cfg(windows)]
                    crate::debug_state::record_action(format!("VK failed: {e}"));
                    #[cfg(windows)]
                    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
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
        static ENABLED: Cell<bool> = Cell::new(false);
        static AFTER_STATE: Cell<u32> = Cell::new(0);
        static LAST_STATE_LINES: RefCell<Option<Vec<String>>> = RefCell::new(None);
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
                    print!("\n");
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
    let args: Vec<String> = env::args().collect();
    #[cfg(windows)]
    dispatch_install_or_service(&args);

    let use_real_win32 = args.iter().any(|a| a == "--real")
        || env::var_os("WARMUP_REAL_VK").is_some_and(|v| v != "0");
    if args.iter().any(|a| a == "--gamepad") {
        #[cfg(feature = "gamepad")]
        {
            return run_gamepad_mode();
        }
        #[cfg(not(feature = "gamepad"))]
        {
            eprintln!("Rebuild with: cargo run --features gamepad -- --gamepad");
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
    println!("Gamepad: `pad` or `cargo run --features gamepad -- --gamepad` (warmUP SDL3 crate)");
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
                    println!("> use: cargo run --features gamepad -- --gamepad");
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
    match args.get(1).map(String::as_str) {
        Some("install") => {
            install::run_install();
            std::process::exit(0);
        }
        Some("uninstall") => {
            install::run_uninstall();
            std::process::exit(0);
        }
        _ => {}
    }
    #[cfg(feature = "service")]
    {
        if args.iter().any(|a| a == "--service-worker") {
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
        eprintln!("Rebuild with: cargo build --release --features service");
        std::process::exit(1);
    }
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
    println!("Warmup gamepad mode — sticks move mouse; tap Y/Triangle to toggle VK");
    if use_real {
        println!("real Win32 VK enabled (WarmupXboxVkWindow)");
    }
    println!("Sign-in service: build with --features service, then `install` as Admin");
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
    };
    if service_mode {
        gamepad::run_watch_loop_service(|| vk_open.get(), on_action)
    } else {
        gamepad::run_watch_loop(|| vk_open.get(), on_action)
    }
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
        rows = rows.saturating_add(if w == 0 { 1 } else { (w + cols - 1) / cols });
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
  --real              Win32 desktop + TabTip/JoyXboxVkWindow (Windows)
  pad                 (gamepad feature) SDL3 snapshot
  --gamepad           (gamepad feature) sticks + Y/Triangle → VK

SCENARIOS
  normal -> fg uac -> press
  cfg winlogon on -> boot -> fg logon -> press
  cfg winlogon on -> boot -> fg uac -> press -> press
"#;
    repl_scroll::note_lines(help_screen_rows(HELP));
    println!("{HELP}");
}
