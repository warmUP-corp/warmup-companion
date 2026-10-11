use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED;
use windows::Win32::Graphics::Gdi::{
    GdiFlush, GetMonitorInfoW, GetObjectW, MonitorFromPoint, BITMAP, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList, GetConsoleWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, ShellExecuteExW, ShellExecuteW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetForegroundWindow, GetWindowRect,
    KillTimer,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, HMENU, HTCAPTION, HTCLIENT,
    SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_SHOW, SW_SHOWNORMAL, WM_CLOSE,
    WM_DESTROY, WM_DPICHANGED, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEWHEEL, WM_NCHITTEST, WM_TIMER,
    WS_EX_APPWINDOW, WS_EX_LAYERED, WS_POPUP,
};

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::mono_ui::*;
use super::tray_menu::lucide;

const CLASS: PCWSTR = w!("WarmupInstallerWizard");
const TIMER_ID: usize = 51;
const TIMER_MS: u32 = 33;
const WIN_W: f32 = 744.0;
const WIN_H: f32 = 640.0;
const WIN_R: f32 = 24.0;
const PAD_X: f32 = 72.0;
const PAD_TOP: f32 = 48.0;
const PAD_BOTTOM: f32 = 96.0;
const SHADOW_DY: f32 = 24.0;
const SHADOW_SIGMA: f32 = 30.0;
const SCREEN_MARGIN: f32 = 16.0;
const TITLE_H: f32 = 64.0;
const RAIL_X: f32 = 24.0;
const RAIL_Y: f32 = 68.0;
const RAIL_W: f32 = 180.0;
const STEP_H: f32 = 44.0;
const STEP_PITCH: f32 = 48.0;
const CX: f32 = 224.0;
const CW: f32 = 496.0;
const FOOTER_Y: f32 = 556.0;
const BTN_Y: f32 = 572.0;
const BTN_H: f32 = 44.0;
const CONTENT_BOTTOM: f32 = 540.0;
const CLOSE: Rect = rect(702.0, 25.0, 18.0, 18.0);
const WEBSITE: &str = "https://www.warmup-gamelauncher.com";
const LICENSE: &str = include_str!("../../LICENSE");
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ADMIN_MSG: &str = "Administrator permission is needed to install the service.";
const UN_ADMIN_MSG: &str = "Administrator permission is needed to remove the service.";
const UNINSTALL_CMD: &str = "uninstall-ui";
const EXIT_CANCELLED: i32 = 2;
const EXIT_NO_WINDOW: i32 = 3;

const BG: u32 = 0x001E1C1C;
const CARD: u32 = 0x002E2C2C;
const HI: u32 = 0x003C3A3A;
const LAV: u32 = 0x00FFA0B6;
const ACCENT: u32 = 0x00FFBCC9;
const ON_ACCENT: u32 = 0x00471021;
const WHITE: u32 = 0x00FFFFFF;
const DIM: u32 = 0x00F5EBEB;
const DIM_A: f32 = 0.6;
const HINT_A: f32 = 0.4;
const LINE_A: f32 = 0x14 as f32 / 255.0;
const MARKER_OFF: u32 = 0x004A4848;
const RADIO_OFF: u32 = 0x00666363;
const BADGE: u32 = 0x00FF5C7C;
const TOGGLE_OFF: u32 = 0x004A4848;
const DANGER: u32 = 0x006B6BFF;

const SHIELD_CHECK: &str = lucide!(
    r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#
);
const HARD_DRIVE: &str = lucide!(
    r#"<line x1="22" x2="2" y1="12" y2="12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/><line x1="6" x2="6.01" y1="16" y2="16"/><line x1="10" x2="10.01" y1="16" y2="16"/>"#
);
const LOCK: &str = lucide!(
    r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#
);
const LOCK_KEYHOLE: &str = lucide!(
    r#"<circle cx="12" cy="16" r="1"/><rect x="3" y="10" width="18" height="12" rx="2"/><path d="M7 10V7a5 5 0 0 1 10 0v3"/>"#
);
const CIRCLE_CHECK: &str = lucide!(r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#);
const CIRCLE: &str = lucide!(r#"<circle cx="12" cy="12" r="10"/>"#);
const CIRCLE_X: &str =
    lucide!(r#"<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>"#);
const LOADER: &str = lucide!(r#"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"#);
const ALERT: &str = lucide!(
    r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
);
const INFO: &str =
    lucide!(r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#);
const MIC: &str = lucide!(
    r#"<path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" x2="12" y1="19" y2="22"/>"#
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    License,
    Features,
    Install,
    Finish,
    Confirm,
    Removing,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Install,
    Uninstall,
}

impl Mode {
    fn steps(self) -> &'static [Step] {
        match self {
            Mode::Install => &Step::INSTALL,
            Mode::Uninstall => &Step::UNINSTALL,
        }
    }
}

impl Step {
    const INSTALL: [Step; 5] = [
        Step::Welcome,
        Step::License,
        Step::Features,
        Step::Install,
        Step::Finish,
    ];
    const UNINSTALL: [Step; 3] = [Step::Confirm, Step::Removing, Step::Done];

    fn label(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::License => "License",
            Step::Features => "Features",
            Step::Install => "Install",
            Step::Finish => "Finish",
            Step::Confirm => "Confirm",
            Step::Removing => "Removing",
            Step::Done => "Done",
        }
    }

    fn index(self, mode: Mode) -> usize {
        mode.steps().iter().position(|s| *s == self).unwrap_or(0)
    }

    fn parse(mode: Mode, s: &str) -> Option<Step> {
        mode.steps()
            .iter()
            .copied()
            .find(|st| st.label().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Parakeet,
    Tiny,
    Base,
    Small,
    Medium,
}

impl Model {
    const ALL: [Model; 5] = [
        Model::Parakeet,
        Model::Tiny,
        Model::Base,
        Model::Small,
        Model::Medium,
    ];

    fn title(self) -> &'static str {
        match self {
            Model::Parakeet => "Parakeet (NVIDIA)",
            Model::Tiny => "Whisper Tiny",
            Model::Base => "Whisper Base",
            Model::Small => "Whisper Small",
            Model::Medium => "Whisper Medium",
        }
    }

    fn caption(self) -> &'static str {
        match self {
            Model::Parakeet => "Fast, multilingual",
            Model::Tiny => "Fastest, least accurate",
            Model::Base => "Balanced",
            Model::Small => "Better accuracy",
            Model::Medium => "Best accuracy, slowest",
        }
    }

    fn size(self) -> &'static str {
        match self {
            Model::Parakeet => "670 MB",
            Model::Tiny => "75 MB",
            Model::Base => "142 MB",
            Model::Small => "466 MB",
            Model::Medium => "1.5 GB",
        }
    }

    fn download(self) -> &'static str {
        match self {
            Model::Parakeet => "~700 MB download",
            Model::Tiny => "~75 MB download",
            Model::Base => "~142 MB download",
            Model::Small => "~466 MB download",
            Model::Medium => "~1.5 GB download",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Model::Parakeet => "parakeet",
            Model::Tiny => "tiny",
            Model::Base => "base",
            Model::Small => "small",
            Model::Medium => "medium",
        }
    }

    fn parse(s: &str) -> Option<Model> {
        Model::ALL.iter().copied().find(|m| m.id() == s)
    }

    fn index(self) -> usize {
        Model::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

fn speech_dir() -> PathBuf {
    PathBuf::from(crate::install::DATA_DIR).join("speech")
}

fn model_installed(m: Model) -> bool {
    let dir = speech_dir();
    match m {
        Model::Parakeet => {
            let pk = dir.join("parakeet");
            pk.join("vocab.txt").is_file()
                && pk.join("onnxruntime.dll").is_file()
                && (pk.join("encoder-model.int8.onnx").is_file()
                    || pk.join("encoder-model.onnx").is_file())
        }
        w => {
            dir.join(format!("ggml-{}.bin", w.id())).is_file()
                && dir.join("whisper-server.exe").is_file()
        }
    }
}

fn dir_bytes(p: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(p) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

fn format_bytes(b: u64) -> String {
    let mb = b as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 1.0 {
        format!("{:.0} MB", mb)
    } else {
        format!("{:.0} KB", (b as f64 / 1024.0).max(1.0))
    }
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn preferred_model(installed: &[bool; 5]) -> Model {
    let dir = speech_dir();
    if read_trimmed(&dir.join("engine.txt")) == "parakeet" && installed[0] {
        return Model::Parakeet;
    }
    let chosen = read_trimmed(&dir.join("model.txt"));
    if let Some(m) = Model::ALL
        .iter()
        .copied()
        .find(|m| *m != Model::Parakeet && chosen == format!("ggml-{}.bin", m.id()))
    {
        if installed[m.index()] {
            return m;
        }
    }
    Model::ALL
        .iter()
        .copied()
        .find(|m| installed[m.index()])
        .unwrap_or(Model::Parakeet)
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Back,
    Next,
    Voice,
    Engine(usize),
    Link,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Task {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Clone)]
struct JobState {
    serial: u64,
    phase: Phase,
    service: Task,
    voice: Task,
    status: String,
    error: String,
    note: String,
    needs_admin: bool,
    hide: bool,
    exit: Option<i32>,
    progress: Option<f32>,
}

impl JobState {
    const fn new() -> Self {
        JobState {
            serial: 0,
            phase: Phase::Idle,
            service: Task::Pending,
            voice: Task::Pending,
            status: String::new(),
            error: String::new(),
            note: String::new(),
            needs_admin: false,
            hide: false,
            exit: None,
            progress: None,
        }
    }
}

static JOB: Mutex<JobState> = Mutex::new(JobState::new());

fn job_update(f: impl FnOnce(&mut JobState)) {
    let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g);
    g.serial += 1;
}

fn job_snapshot() -> JobState {
    JOB.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Clone)]
struct Options {
    mode: Mode,
    preview: bool,
    no_admin: bool,
    step: Option<Step>,
    install: Option<Option<Model>>,
    remove: Option<bool>,
    snapshot: Option<PathBuf>,
}

impl Options {
    fn new(mode: Mode) -> Self {
        Options {
            mode,
            preview: false,
            no_admin: false,
            step: None,
            install: None,
            remove: None,
            snapshot: None,
        }
    }
}

fn parse_options(args: &[String]) -> Options {
    let mode = if args.get(1).map(String::as_str) == Some(UNINSTALL_CMD) {
        Mode::Uninstall
    } else {
        Mode::Install
    };
    let mut o = Options::new(mode);
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--preview" => o.preview = true,
            "--no-admin" => o.no_admin = true,
            "--step" => {
                o.step = args.get(i + 1).and_then(|s| Step::parse(mode, s));
                i += 1;
            }
            "--snapshot" => {
                o.snapshot = args.get(i + 1).map(PathBuf::from);
                o.preview = true;
                i += 1;
            }
            "--install" => {
                o.install = args.get(i + 1).map(|s| Model::parse(s));
                i += 1;
            }
            "--remove" => {
                o.remove = args.get(i + 1).map(|s| s == "models");
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    o
}

struct Ui {
    hwnd: HWND,
    scale: f32,
    gfx: Option<Gfx>,
    opts: Options,
    elevated: bool,
    elevate_error: String,
    step: Step,
    focus: Target,
    kb: bool,
    voice_on: bool,
    model: Model,
    installed: [bool; 5],
    models_on: bool,
    models_bytes: u64,
    job: JobState,
    hidden: bool,
    scroll: f32,
    scroll_max: Cell<f32>,
    back_r: Cell<Option<Rect>>,
    next_r: Cell<Option<Rect>>,
    started: Instant,
    pad: Pad,
}

const PAD_UP: u32 = 1 << 0;
const PAD_DOWN: u32 = 1 << 1;
const PAD_LEFT: u32 = 1 << 2;
const PAD_RIGHT: u32 = 1 << 3;
/// South face button: ✕ on PlayStation, A on Xbox.
const PAD_CONFIRM: u32 = 1 << 4;
/// East face button: ○ on PlayStation, B on Xbox.
const PAD_BACK: u32 = 1 << 5;
const PAD_DIRS: u32 = PAD_UP | PAD_DOWN | PAD_LEFT | PAD_RIGHT;

/// Controller input for the wizard. Reads pads itself through the shared SDL3 layer, so
/// PlayStation, Xbox, Switch and generic pads work whether or not the service is running.
struct Pad {
    #[cfg(feature = "gamepad")]
    input: Option<crate::gamepad_backend::GamepadInput>,
    buttons: u32,
    held: u32,
    since: Option<Instant>,
    last_repeat: Option<Instant>,
}

impl Pad {
    fn new() -> Self {
        Pad {
            #[cfg(feature = "gamepad")]
            input: crate::gamepad_backend::GamepadInput::new(&crate::gamepad_backend::mapping_db_path())
                .map_err(|e| crate::install::log_line(&format!("installer ui: gamepad: {e}")))
                .ok(),
            buttons: 0,
            held: 0,
            since: None,
            last_repeat: None,
        }
    }

    fn connected(&self) -> bool {
        #[cfg(feature = "gamepad")]
        return self.input.as_ref().is_some_and(|i| i.active_controller_name().is_some());
        #[cfg(not(feature = "gamepad"))]
        false
    }

    /// Generic and Switch pads use the Xbox art, like the rest of the Companion.
    fn playstation(&self) -> bool {
        #[cfg(feature = "gamepad")]
        return self.input.as_ref().is_some_and(|i| i.active_controller_type() == "playstation");
        #[cfg(not(feature = "gamepad"))]
        false
    }

    /// Returns (new presses including repeats, right stick Y up-positive, connection changed).
    /// Presses only count while `focused`, so input meant for another window never clicks
    /// through the wizard.
    #[cfg(feature = "gamepad")]
    fn poll(&mut self, focused: bool) -> (Vec<u32>, f32, bool) {
        use crate::gamepad_backend::Button;
        let Some(input) = self.input.as_mut() else {
            return (Vec::new(), 0.0, false);
        };
        let changed = input.poll_events();
        for c in input.detect_button_changes() {
            let bit = match c.button {
                Button::A => PAD_CONFIRM,
                Button::B => PAD_BACK,
                Button::Up => PAD_UP,
                Button::Down => PAD_DOWN,
                Button::Left => PAD_LEFT,
                Button::Right => PAD_RIGHT,
                _ => continue,
            };
            if c.pressed {
                self.buttons |= bit;
            } else {
                self.buttons &= !bit;
            }
        }
        let (lx, ly, _, ry) = input.axes();
        let mut held = self.buttons;
        for (on, bit) in [(ly > 0.5, PAD_UP), (ly < -0.5, PAD_DOWN), (lx < -0.5, PAD_LEFT), (lx > 0.5, PAD_RIGHT)] {
            if on {
                held |= bit;
            }
        }
        if changed {
            // Baseline on (dis)connect so a button already held does not fire.
            self.buttons = 0;
            self.held = held & PAD_DIRS;
            return (Vec::new(), 0.0, true);
        }
        let now = Instant::now();
        let pressed = held & !self.held;
        if held != self.held {
            self.since = Some(now);
            self.last_repeat = None;
        }
        let mut edges: Vec<u32> = [PAD_CONFIRM, PAD_BACK, PAD_UP, PAD_DOWN, PAD_LEFT, PAD_RIGHT]
            .into_iter()
            .filter(|b| pressed & b != 0)
            .collect();
        // A held direction repeats after 400 ms, every 120 ms, like a console menu.
        let dirs = held & PAD_DIRS;
        if dirs != 0 && pressed == 0 {
            let held_for = self.since.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
            let since_repeat = self.last_repeat.map_or(f32::MAX, |t| now.duration_since(t).as_secs_f32());
            if held_for > 0.4 && since_repeat > 0.12 {
                self.last_repeat = Some(now);
                edges.extend([PAD_UP, PAD_DOWN, PAD_LEFT, PAD_RIGHT].into_iter().filter(|b| dirs & b != 0));
            }
        }
        self.held = held;
        if !focused {
            return (Vec::new(), 0.0, false);
        }
        (edges, if ry.abs() > 0.25 { ry } else { 0.0 }, false)
    }

    #[cfg(not(feature = "gamepad"))]
    fn poll(&mut self, _focused: bool) -> (Vec<u32>, f32, bool) {
        (Vec::new(), 0.0, false)
    }
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static THREAD: OnceLock<Mutex<Option<DesktopWindowThread>>> = OnceLock::new();
static RESULT_TX: Mutex<Option<SyncSender<i32>>> = Mutex::new(None);
static START: Mutex<Option<(Options, bool, String)>> = Mutex::new(None);

struct InstallerApp;

impl DesktopApp for InstallerApp {
    const THREAD_NAME: &'static str = "warmup-installer-ui";
    const CLASS_NAME: PCWSTR = CLASS;
    const BG_COLOR: u32 = BG;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = wndproc;

    fn on_show(&mut self, _lparam: LPARAM) {
        ui_show();
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

fn payload_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn detach_console() {
    unsafe {
        let console = GetConsoleWindow();
        if console.0.is_null() {
            return;
        }
        let mut pids = [0u32; 4];
        if GetConsoleProcessList(&mut pids) <= 1 {
            let _ = ShowWindow(console, SW_HIDE);
        }
        let _ = FreeConsole();
    }
}

fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    format!("\"{}\"", a.replace('"', "\\\""))
}

enum Elevation {
    Exited(i32),
    Declined,
    Failed(String),
}

fn run_elevated(params: &str, on_started: impl FnOnce()) -> Elevation {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return Elevation::Failed(format!("Could not locate the setup program: {e}")),
    };
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Elevation::Declined;
            }
            return Elevation::Failed(format!(
                "Windows could not start setup as administrator: {}",
                e.message()
            ));
        }
        if info.hProcess.is_invalid() {
            return Elevation::Failed("Windows did not return the elevated setup process.".into());
        }
        on_started();
        let _ = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Elevation::Exited(code as i32)
    }
}

fn finish(code: i32) {
    if let Ok(mut g) = RESULT_TX.lock() {
        if let Some(tx) = g.take() {
            let _ = tx.send(code);
        }
    }
    ui_hide();
}

pub fn run(args: &[String]) -> i32 {
    detach_console();
    let opts = parse_options(args);
    let mut elevated = opts.preview && !opts.no_admin;
    let mut elevate_error = String::new();
    if !opts.preview {
        elevated = is_elevated();
        if !elevated {
            let params = args
                .iter()
                .skip(1)
                .map(|a| quote_arg(a))
                .collect::<Vec<_>>()
                .join(" ");
            match run_elevated(&params, || {}) {
                Elevation::Exited(code) => return code,
                Elevation::Declined => {}
                Elevation::Failed(e) => {
                    crate::install::log_line(&format!("installer ui: elevate: {e}"));
                    elevate_error = e;
                }
            }
        }
    }
    *START.lock().unwrap_or_else(|e| e.into_inner()) = Some((opts, elevated, elevate_error));
    let (tx, rx) = mpsc::sync_channel(1);
    *RESULT_TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    {
        let Ok(mut guard) = slot.lock() else {
            crate::install::log_line("installer ui: window thread lock poisoned");
            return EXIT_NO_WINDOW;
        };
        if guard.is_none() {
            match desktop_window::spawn(InstallerApp) {
                Ok(t) => *guard = Some(t),
                Err(e) => {
                    crate::install::log_line(&format!("installer ui: spawn failed: {e}"));
                    return EXIT_NO_WINDOW;
                }
            }
        }
        if let Some(t) = guard.as_ref() {
            let _ = t.show(LPARAM(0));
        }
    }
    rx.recv().unwrap_or(EXIT_CANCELLED)
}

fn ui_show() {
    if let Some(hwnd) = UI.with(|u| u.borrow().as_ref().map(|u| u.hwnd)) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    let (opts, elevated, elevate_error) = START
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_else(|| (Options::new(Mode::Install), is_elevated(), String::new()));
    let mode = opts.mode;
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(instance) = GetModuleHandleW(None) else {
            finish(EXIT_NO_WINDOW);
            return;
        };
        let gfx = match Gfx::new() {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("installer ui: gfx: {e}"));
                finish(EXIT_NO_WINDOW);
                return;
            }
        };
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_APPWINDOW,
            CLASS,
            &HSTRING::from(window_title(mode)),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            HMENU::default(),
            windows::Win32::Foundation::HINSTANCE(instance.0),
            None,
        ) else {
            crate::install::log_line("installer ui: CreateWindowExW failed");
            finish(EXIT_NO_WINDOW);
            return;
        };
        let installed = Model::ALL.map(model_installed);
        let models_bytes = if mode == Mode::Uninstall {
            dir_bytes(&speech_dir())
        } else {
            0
        };
        let mut ui = Ui {
            hwnd,
            scale: 1.0,
            gfx: Some(gfx),
            elevated,
            elevate_error,
            step: mode.steps()[0],
            focus: Target::Next,
            kb: false,
            voice_on: true,
            model: preferred_model(&installed),
            installed,
            models_on: models_bytes > 0,
            models_bytes,
            job: job_snapshot(),
            hidden: false,
            scroll: 0.0,
            scroll_max: Cell::new(0.0),
            back_r: Cell::new(None),
            next_r: Cell::new(None),
            started: Instant::now(),
            pad: Pad::new(),
            opts,
        };
        if let (Mode::Install, Some(choice)) = (mode, ui.opts.install) {
            ui.voice_on = choice.is_some();
            if let Some(m) = choice {
                ui.model = m;
            }
            ui.step = Step::Install;
            start_install(&mut ui);
        } else if let (Mode::Uninstall, Some(models)) = (mode, ui.opts.remove) {
            ui.models_on = models && ui.models_bytes > 0;
            ui.step = Step::Removing;
            start_uninstall(&mut ui);
        } else if ui.opts.preview {
            if let Some(st) = ui.opts.step {
                ui.step = st;
                match st {
                    Step::Install => start_install(&mut ui),
                    Step::Removing => start_uninstall(&mut ui),
                    _ => {}
                }
            }
        }
        ensure_focus(&mut ui);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        with_ui(|ui| {
            place(ui, None);
            paint(ui);
        });
        let snap = with_ui_ret(|ui| ui.opts.snapshot.clone().map(|p| (p, save_snapshot(ui))));
        if let Some(Some((path, result))) = snap {
            if let Err(e) = result {
                crate::install::log_line(&format!("installer ui: snapshot {}: {e}", path.display()));
            }
            finish(0);
            return;
        }
        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOZORDER
                | SWP_SHOWWINDOW
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
        );
        let _ = SetForegroundWindow(hwnd);
    }
}

fn window_title(mode: Mode) -> &'static str {
    match mode {
        Mode::Install => "warmUP Companion Setup",
        Mode::Uninstall => "Uninstall warmUP Companion",
    }
}

fn ui_hide() {
    if let Some(ui) = UI.with(|u| u.try_borrow_mut().ok().and_then(|mut b| b.take())) {
        unsafe {
            let _ = KillTimer(ui.hwnd, TIMER_ID);
            let _ = DestroyWindow(ui.hwnd);
        }
    }
}

fn with_ui(f: impl FnOnce(&mut Ui)) {
    UI.with(|u| {
        if let Ok(mut b) = u.try_borrow_mut() {
            if let Some(ui) = b.as_mut() {
                f(ui);
            }
        }
    });
}

fn with_ui_ret<T>(f: impl FnOnce(&mut Ui) -> T) -> Option<T> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut b| b.as_mut().map(f)))
}

fn save_snapshot(ui: &Ui) -> Result<(), String> {
    let path = ui.opts.snapshot.as_ref().ok_or("no snapshot path")?;
    let g = ui.gfx.as_ref().ok_or("nothing rendered")?;
    let (w, h) = g.size;
    let mut bm = BITMAP::default();
    unsafe {
        let _ = GdiFlush();
        if GetObjectW(
            g.bmp,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut _),
        ) == 0
            || bm.bmBits.is_null()
        {
            return Err("surface bits unavailable".into());
        }
    }
    let len = (w * h * 4) as usize;
    let bgra = unsafe { std::slice::from_raw_parts(bm.bmBits as *const u8, len) };
    let rgba: Vec<u8> = bgra
        .chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            let over = |c: u8| (c as u32 + 0xED * (255 - a) / 255).min(255) as u8;
            [over(p[2]), over(p[1]), over(p[0]), 255]
        })
        .collect();
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgba))
        .map_err(|e| e.to_string())
}

fn surface_px(scale: f32) -> (i32, i32) {
    (
        ((WIN_W + 2.0 * PAD_X) * scale).ceil() as i32,
        ((WIN_H + PAD_TOP + PAD_BOTTOM) * scale).ceil() as i32,
    )
}

fn place(ui: &mut Ui, anchor: Option<(i32, i32)>) {
    unsafe {
        let monitor: HMONITOR = match anchor {
            Some((x, y)) => MonitorFromPoint(
                POINT {
                    x: x + (PAD_X * ui.scale) as i32,
                    y: y + (PAD_TOP * ui.scale) as i32,
                },
                MONITOR_DEFAULTTONEAREST,
            ),
            None if ui.hidden || ui.gfx.is_some() => {
                MonitorFromWindow(ui.hwnd, MONITOR_DEFAULTTONEAREST)
            }
            None => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
            }
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let ww = (work.right - work.left) as f32;
        let wh = (work.bottom - work.top) as f32;
        let fit = ((ww / (WIN_W + 2.0 * SCREEN_MARGIN)).min(wh / (WIN_H + 2.0 * SCREEN_MARGIN)))
            .max(0.5);
        let s = (dx.max(96) as f32 / 96.0).min(fit);
        if (s - ui.scale).abs() > 0.001 {
            ui.scale = s;
            if let Some(g) = ui.gfx.as_mut() {
                g.icons.clear();
            }
        }
        let card_w = (WIN_W * s).round() as i32;
        let card_h = (WIN_H * s).round() as i32;
        let (mut cx, mut cy) = match anchor {
            Some((x, y)) => (x + (PAD_X * s) as i32, y + (PAD_TOP * s) as i32),
            None => (
                work.left + (work.right - work.left - card_w) / 2,
                work.top + (work.bottom - work.top - card_h) / 2,
            ),
        };
        cx = cx.clamp(work.left, (work.right - card_w).max(work.left));
        cy = cy.clamp(work.top, (work.bottom - card_h).max(work.top));
        let (w, h) = surface_px(s);
        let _ = SetWindowPos(
            ui.hwnd,
            None,
            cx - (PAD_X * s).round() as i32,
            cy - (PAD_TOP * s).round() as i32,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn step_rect(i: usize) -> Rect {
    rect(RAIL_X, RAIL_Y + i as f32 * STEP_PITCH, RAIL_W, STEP_H)
}

fn voice_row() -> Rect {
    rect(CX, 230.0, CW, 59.0)
}

fn engine_list() -> Rect {
    rect(CX, 330.0, CW, 192.0)
}

fn engine_row(i: usize) -> Rect {
    rect(CX + 6.0, 336.0 + i as f32 * 36.0, CW - 12.0, 36.0)
}

fn link_row() -> Rect {
    rect(CX, 144.0, CW, 59.0)
}

fn back_label(ui: &Ui) -> Option<&'static str> {
    match ui.step {
        Step::Welcome => Some("Cancel"),
        Step::License | Step::Features => Some("Back"),
        Step::Install if ui.job.phase == Phase::Failed => Some("Back"),
        Step::Confirm => Some("Cancel"),
        Step::Removing if ui.job.phase == Phase::Failed => Some("Close"),
        _ => None,
    }
}

fn next_label(ui: &Ui) -> Option<&'static str> {
    match ui.step {
        Step::Welcome => Some("Next"),
        Step::License => Some("Accept"),
        Step::Features => Some("Install"),
        Step::Install => match ui.job.phase {
            Phase::Failed => Some("Retry"),
            Phase::Done => Some("Continue"),
            _ => None,
        },
        Step::Finish => Some("Finish"),
        Step::Confirm => Some("Uninstall"),
        Step::Removing => match ui.job.phase {
            Phase::Failed => Some("Retry"),
            Phase::Done => Some("Continue"),
            _ => None,
        },
        Step::Done => Some("Close"),
    }
}

fn has_models(ui: &Ui) -> bool {
    ui.models_bytes > 0
}

fn focus_order(ui: &Ui) -> Vec<Target> {
    let mut o = Vec::new();
    match ui.step {
        Step::Features => {
            o.push(Target::Voice);
            if ui.voice_on {
                o.extend((0..Model::ALL.len()).map(Target::Engine));
            }
        }
        Step::Finish => o.push(Target::Link),
        Step::Confirm if has_models(ui) => o.push(Target::Voice),
        _ => {}
    }
    if back_label(ui).is_some() {
        o.push(Target::Back);
    }
    if next_label(ui).is_some() {
        o.push(Target::Next);
    }
    o
}

fn ensure_focus(ui: &mut Ui) {
    let order = focus_order(ui);
    if !order.contains(&ui.focus) {
        ui.focus = if order.contains(&Target::Next) {
            Target::Next
        } else {
            order.last().copied().unwrap_or(Target::Next)
        };
    }
}

fn set_step(ui: &mut Ui, step: Step) {
    ui.step = step;
    ui.scroll = 0.0;
    ui.focus = Target::Next;
    ensure_focus(ui);
}

fn uninstall_close_code(ui: &Ui) -> Option<i32> {
    match ui.step {
        Step::Confirm => Some(EXIT_CANCELLED),
        Step::Done => Some(0),
        _ => match ui.job.phase {
            Phase::Done => Some(0),
            Phase::Running => None,
            Phase::Failed if ui.job.needs_admin => Some(EXIT_CANCELLED),
            Phase::Failed | Phase::Idle => Some(1),
        },
    }
}

fn close_code(ui: &Ui) -> Option<i32> {
    if ui.opts.mode == Mode::Uninstall {
        return uninstall_close_code(ui);
    }
    if ui.step == Step::Finish || ui.job.phase == Phase::Done {
        return Some(0);
    }
    if ui.job.phase == Phase::Running {
        if ui.job.service == Task::Done {
            return Some(0);
        }
        return None;
    }
    Some(2)
}

fn request_close(ui: &Ui) {
    if let Some(code) = close_code(ui) {
        finish(code);
    }
}

fn go_next(ui: &mut Ui) {
    match ui.step {
        Step::Welcome => set_step(ui, Step::License),
        Step::License => set_step(ui, Step::Features),
        Step::Features => {
            set_step(ui, Step::Install);
            start_install(ui);
        }
        Step::Install => match ui.job.phase {
            Phase::Failed => {
                if ui.job.needs_admin && !ui.opts.preview {
                    request_elevation(ui);
                } else {
                    start_install(ui);
                }
            }
            Phase::Done => set_step(ui, Step::Finish),
            _ => {}
        },
        Step::Finish | Step::Done => {
            finish(0);
            return;
        }
        Step::Confirm => {
            set_step(ui, Step::Removing);
            start_uninstall(ui);
        }
        Step::Removing => match ui.job.phase {
            Phase::Failed => {
                if ui.job.needs_admin && !ui.opts.preview {
                    request_elevation(ui);
                } else {
                    start_uninstall(ui);
                }
            }
            Phase::Done => set_step(ui, Step::Done),
            _ => {}
        },
    }
    ensure_focus(ui);
}

fn go_back(ui: &mut Ui) {
    match ui.step {
        Step::Welcome => finish(2),
        Step::License => set_step(ui, Step::Welcome),
        Step::Features => set_step(ui, Step::License),
        Step::Install if ui.job.phase == Phase::Failed => {
            job_update(|j| *j = JobState { serial: j.serial, ..JobState::new() });
            ui.job = job_snapshot();
            set_step(ui, Step::Features);
        }
        Step::Confirm => finish(EXIT_CANCELLED),
        Step::Removing if ui.job.phase == Phase::Failed => request_close(ui),
        _ => {}
    }
}

fn start_install(ui: &mut Ui) {
    if ui.job.phase == Phase::Running {
        return;
    }
    ui.scroll = 0.0;
    let voice = ui.voice_on;
    let model = ui.model;
    let installed = ui.installed[model.index()];
    if !ui.elevated {
        let msg = if ui.elevate_error.is_empty() {
            ADMIN_MSG.to_string()
        } else {
            format!("{ADMIN_MSG} {}", ui.elevate_error)
        };
        job_update(|j| {
            j.phase = Phase::Failed;
            j.service = Task::Pending;
            j.voice = Task::Pending;
            j.status = "Waiting for administrator permission".into();
            j.error = msg;
            j.note.clear();
            j.needs_admin = true;
        });
        ui.job = job_snapshot();
        ensure_focus(ui);
        return;
    }
    job_update(|j| {
        j.phase = Phase::Running;
        j.service = Task::Running;
        j.voice = Task::Pending;
        j.status = "Installing the Companion service\u{2026}".into();
        j.error.clear();
        j.note.clear();
        j.needs_admin = false;
    });
    ui.job = job_snapshot();
    let preview = ui.opts.preview;
    thread::spawn(move || {
        if preview {
            run_preview_job(voice, model, installed);
        } else {
            run_install_job(voice, model, installed);
        }
    });
    ensure_focus(ui);
}

fn start_uninstall(ui: &mut Ui) {
    if ui.job.phase == Phase::Running {
        return;
    }
    ui.scroll = 0.0;
    let models = ui.models_on && has_models(ui);
    if !ui.elevated {
        let msg = if ui.elevate_error.is_empty() {
            UN_ADMIN_MSG.to_string()
        } else {
            format!("{UN_ADMIN_MSG} {}", ui.elevate_error)
        };
        job_update(|j| {
            j.phase = Phase::Failed;
            j.service = Task::Pending;
            j.voice = Task::Pending;
            j.status = "Waiting for administrator permission".into();
            j.error = msg;
            j.note.clear();
            j.needs_admin = true;
        });
        ui.job = job_snapshot();
        ensure_focus(ui);
        return;
    }
    job_update(|j| {
        j.phase = Phase::Running;
        j.service = Task::Running;
        j.voice = Task::Pending;
        j.status = "Removing the Companion service\u{2026}".into();
        j.error.clear();
        j.note.clear();
        j.needs_admin = false;
    });
    ui.job = job_snapshot();
    let preview = ui.opts.preview;
    thread::spawn(move || {
        if preview {
            run_preview_uninstall(models);
        } else {
            run_uninstall_job(models);
        }
    });
    ensure_focus(ui);
}

fn run_preview_uninstall(models: bool) {
    thread::sleep(Duration::from_millis(1400));
    job_update(|j| {
        j.service = Task::Done;
        if models {
            j.voice = Task::Running;
            j.status = "Removing offline voice models\u{2026}".into();
        }
    });
    if models {
        thread::sleep(Duration::from_millis(900));
    }
    job_update(|j| {
        if models {
            j.voice = Task::Done;
        }
        j.phase = Phase::Done;
        j.status = "Preview only. Nothing was removed.".into();
    });
}

fn run_uninstall_job(models: bool) {
    if let Err(e) = crate::install::uninstall_service() {
        crate::install::log_line(&format!("uninstall ui: service removal failed: {e}"));
        job_update(|j| {
            j.phase = Phase::Failed;
            j.service = Task::Failed;
            j.status = "The Companion service could not be removed".into();
            j.error = format!(
                "{}\nDetails are logged in {}\\service.log.",
                e.trim(),
                crate::install::DATA_DIR
            );
        });
        return;
    }
    if !models {
        job_update(|j| {
            j.service = Task::Done;
            j.phase = Phase::Done;
            j.status = "warmUP Companion was removed.".into();
        });
        return;
    }
    job_update(|j| {
        j.service = Task::Done;
        j.voice = Task::Running;
        j.status = "Removing offline voice models\u{2026}".into();
    });
    let dir = speech_dir();
    let result = remove_dir_retry(&dir);
    if let Err(e) = &result {
        crate::install::log_line(&format!("uninstall ui: remove {}: {e}", dir.display()));
    }
    job_update(|j| {
        j.phase = Phase::Done;
        j.status = "warmUP Companion was removed.".into();
        match result {
            Ok(()) => j.voice = Task::Done,
            Err(e) => {
                j.voice = Task::Failed;
                j.note = format!(
                    "Some voice model files could not be removed ({e}). Delete {} by hand after a restart.",
                    dir.display()
                );
            }
        }
    });
}

fn remove_dir_retry(dir: &Path) -> std::io::Result<()> {
    let mut last = Ok(());
    for attempt in 0..5 {
        if !dir.exists() {
            return Ok(());
        }
        if attempt > 0 {
            thread::sleep(Duration::from_millis(400));
        }
        last = std::fs::remove_dir_all(dir);
        if last.is_ok() {
            return Ok(());
        }
    }
    last
}

fn request_elevation(ui: &mut Ui) {
    let params = if ui.opts.mode == Mode::Uninstall {
        let choice = if ui.models_on && has_models(ui) { "models" } else { "service" };
        format!("{UNINSTALL_CMD} --remove {choice}")
    } else {
        let choice = if ui.voice_on { ui.model.id() } else { "none" };
        format!("setup --install {choice}")
    };
    job_update(|j| {
        j.phase = Phase::Running;
        j.service = Task::Pending;
        j.status = "Waiting for administrator permission\u{2026}".into();
        j.error.clear();
        j.needs_admin = false;
    });
    ui.job = job_snapshot();
    ensure_focus(ui);
    thread::spawn(move || {
        let result = run_elevated(&params, || job_update(|j| j.hide = true));
        match result {
            Elevation::Exited(code) => job_update(|j| j.exit = Some(code)),
            Elevation::Declined => job_update(|j| {
                j.phase = Phase::Failed;
                j.status = "Waiting for administrator permission".into();
                j.error = ADMIN_MSG.into();
                j.needs_admin = true;
            }),
            Elevation::Failed(e) => job_update(|j| {
                j.phase = Phase::Failed;
                j.status = "Waiting for administrator permission".into();
                j.error = format!("{ADMIN_MSG} {e}");
                j.needs_admin = true;
            }),
        }
    });
}

fn run_preview_job(voice: bool, model: Model, installed: bool) {
    thread::sleep(Duration::from_millis(1400));
    job_update(|j| {
        j.service = Task::Done;
        if voice {
            j.voice = Task::Running;
            j.status = format!("Downloading {}\u{2026}", model.title());
        }
    });
    if voice && !installed {
        thread::sleep(Duration::from_millis(1800));
    }
    job_update(|j| {
        if voice {
            j.voice = Task::Done;
        }
        j.phase = Phase::Done;
        j.status = "Preview only. Nothing was installed.".into();
    });
}

fn powershell(script: &Path, args: &[&str], title: &str) -> std::io::Result<std::process::ExitStatus> {
    use std::io::BufRead;
    use std::os::windows::process::CommandExt;
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if let Some((pct, detail)) = parse_progress(&line) {
                job_update(|j| {
                    j.progress = Some(pct);
                    j.status = format!("Downloading {title} \u{00B7} {detail}");
                });
            }
        }
    }
    let status = child.wait();
    job_update(|j| j.progress = None);
    status
}

fn parse_progress(line: &str) -> Option<(f32, String)> {
    let (label, rest) = line.trim().split_once(": ")?;
    let (pct, sizes) = rest.split_once("% (")?;
    let pct: f32 = pct.trim().parse().ok()?;
    let sizes = sizes.trim_end_matches(')').replace('/', " / ");
    Some(((pct / 100.0).clamp(0.0, 1.0), format!("{label} {sizes}")))
}

fn run_install_job(voice: bool, model: Model, installed: bool) {
    if let Err(e) = crate::install::install_service(false) {
        crate::install::log_line(&format!("installer ui: service install failed: {e}"));
        job_update(|j| {
            j.phase = Phase::Failed;
            j.service = Task::Failed;
            j.status = "The Companion service could not be installed".into();
            j.error = format!(
                "{}\nDetails are logged in {}\\service.log.",
                e.trim(),
                crate::install::DATA_DIR
            );
        });
        return;
    }
    if !voice {
        job_update(|j| {
            j.service = Task::Done;
            j.phase = Phase::Done;
            j.status = "warmUP Companion is installed and running.".into();
        });
        return;
    }
    job_update(|j| {
        j.service = Task::Done;
        j.voice = Task::Running;
        j.status = if installed {
            format!("{} is already installed", model.title())
        } else {
            format!("Downloading {}\u{2026}", model.title())
        };
    });
    let dir = payload_dir();
    let speech = speech_dir();
    let note = if model == Model::Parakeet {
        if installed {
            let _ = std::fs::write(speech.join("engine.txt"), "parakeet");
            None
        } else {
            let script = dir.join("Get-WarmupParakeet.ps1");
            voice_script(&script, &[], model.title())
        }
    } else {
        let script = dir.join("Get-WarmupSpeech.ps1");
        let note = voice_script(&script, &["-Model", model.id()], model.title());
        if note.is_none() {
            let _ = std::fs::write(speech.join("engine.txt"), "whisper");
        }
        note
    };
    job_update(|j| {
        j.phase = Phase::Done;
        match note {
            Some(n) => {
                j.voice = Task::Failed;
                j.note = n;
                j.status = "warmUP Companion is installed and running.".into();
            }
            None => {
                j.voice = Task::Done;
                j.status = "warmUP Companion is installed and running.".into();
            }
        }
    });
}

fn voice_script(script: &Path, args: &[&str], title: &str) -> Option<String> {
    if !script.is_file() {
        return Some(format!(
            "Voice typing was not set up because {} is missing. Companion works fine without it.",
            script.display()
        ));
    }
    match powershell(script, args, title) {
        Ok(st) if st.success() => None,
        Ok(st) => Some(format!(
            "Voice typing could not be downloaded (exit {}). Companion works fine without it. Add it later by running {}.",
            st.code().unwrap_or(-1),
            script.display()
        )),
        Err(e) => Some(format!(
            "Voice typing could not be downloaded: {e}. Companion works fine without it. Add it later by running {}.",
            script.display()
        )),
    }
}

fn poll_job(ui: &mut Ui) -> bool {
    let snap = job_snapshot();
    if snap.serial == ui.job.serial {
        return false;
    }
    let was = ui.job.phase;
    ui.job = snap;
    if ui.job.hide && !ui.hidden {
        ui.hidden = true;
        unsafe {
            let _ = ShowWindow(ui.hwnd, SW_HIDE);
        }
    }
    if let Some(code) = ui.job.exit {
        finish(code);
        return false;
    }
    if was != ui.job.phase {
        ui.scroll = 0.0;
        ui.focus = Target::Next;
        ensure_focus(ui);
    }
    true
}

fn open_website() {
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(WEBSITE),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

fn click(ui: &mut Ui, x: f32, y: f32) {
    ui.kb = false;
    if CLOSE.inset(-7.0).contains(x, y) {
        request_close(ui);
        return;
    }
    if ui.step == Step::Finish && link_row().contains(x, y) {
        ui.focus = Target::Link;
        open_website();
        return;
    }
    if ui.step == Step::Confirm && has_models(ui) && voice_row().contains(x, y) {
        ui.models_on = !ui.models_on;
        ui.focus = Target::Voice;
        return;
    }
    if ui.step == Step::Features {
        if voice_row().contains(x, y) {
            ui.voice_on = !ui.voice_on;
            ui.focus = Target::Voice;
            ensure_focus(ui);
            return;
        }
        if ui.voice_on {
            for i in 0..Model::ALL.len() {
                if engine_row(i).contains(x, y) {
                    ui.model = Model::ALL[i];
                    ui.focus = Target::Engine(i);
                    return;
                }
            }
        }
    }
    if back_label(ui).is_some() && ui.back_r.get().is_some_and(|r| r.contains(x, y)) {
        go_back(ui);
        return;
    }
    if next_label(ui).is_some() && ui.next_r.get().is_some_and(|r| r.contains(x, y)) {
        go_next(ui);
    }
}

fn step_focus(ui: &mut Ui, forward: bool) {
    let order = focus_order(ui);
    if order.is_empty() {
        return;
    }
    let n = order.len();
    ui.focus = match order.iter().position(|t| *t == ui.focus) {
        Some(i) => order[if forward { (i + 1) % n } else { (i + n - 1) % n }],
        None => order[0],
    };
}

fn key_down(ui: &mut Ui, vk: u16) {
    ui.kb = true;
    match vk {
        v if v == VK_ESCAPE.0 => request_close(ui),
        v if v == VK_TAB.0 || v == VK_RIGHT.0 || v == VK_DOWN.0 => step_focus(ui, true),
        v if v == VK_LEFT.0 || v == VK_UP.0 => step_focus(ui, false),
        v if v == VK_NEXT.0 => ui.scroll += 120.0,
        v if v == VK_PRIOR.0 => ui.scroll -= 120.0,
        v if v == VK_RETURN.0 || v == VK_SPACE.0 => match ui.focus {
            Target::Back => go_back(ui),
            Target::Next => go_next(ui),
            Target::Voice if ui.step == Step::Confirm => ui.models_on = !ui.models_on,
            Target::Voice => {
                ui.voice_on = !ui.voice_on;
                ensure_focus(ui);
            }
            Target::Engine(i) => ui.model = Model::ALL[i],
            Target::Link => open_website(),
        },
        _ => {}
    }
}

/// Controller presses reuse the keyboard paths, so focus and step rules stay identical.
/// Back follows the visible Back / Cancel / Close button and does nothing while it is hidden
/// (installing, removing, finished).
fn pad_press(ui: &mut Ui, bit: u32) {
    match bit {
        PAD_CONFIRM => key_down(ui, VK_RETURN.0),
        PAD_BACK => {
            ui.kb = true;
            if back_label(ui).is_some() {
                go_back(ui);
            }
        }
        PAD_UP => key_down(ui, VK_UP.0),
        PAD_DOWN => key_down(ui, VK_DOWN.0),
        PAD_LEFT => key_down(ui, VK_LEFT.0),
        _ => key_down(ui, VK_RIGHT.0),
    }
}

fn design_point(ui: &Ui, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x / ui.scale - PAD_X, y / ui.scale - PAD_TOP)
}

fn paint(ui: &mut Ui) {
    if ui.hidden {
        return;
    }
    let mut gfx = match ui.gfx.take() {
        Some(g) => g,
        None => match unsafe { Gfx::new() } {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("installer ui: gfx: {e}"));
                return;
            }
        },
    };
    if let Err(e) = unsafe { render(ui, &mut gfx) } {
        crate::install::log_line(&format!("installer ui: render: {e}"));
        return;
    }
    ui.gfx = Some(gfx);
}

unsafe fn render(ui: &Ui, g: &mut Gfx) -> Result<(), String> {
    let s = ui.scale;
    g.begin(surface_px(s))?;
    g.draw_shadow(
        &[rect(PAD_X * s, (PAD_TOP + SHADOW_DY) * s, WIN_W * s, WIN_H * s)],
        WIN_R * s,
        SHADOW_SIGMA * s,
    );
    g.transform(s, PAD_X * s, PAD_TOP * s);
    draw(ui, g);
    g.present(ui.hwnd, None)
}

unsafe fn wrap_lines(g: &mut Gfx, text: &str, w: f32, px: f32, bold: bool) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let para = para.trim_end();
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if g.measure(&candidate, px, bold) <= w {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if g.measure(word, px, bold) <= w {
                line = word.to_string();
                continue;
            }
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if g.measure(&piece, px, bold) > w && piece.chars().count() > 1 {
                    piece.pop();
                    out.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            line = piece;
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
unsafe fn wrap_text(
    g: &mut Gfx,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    px: f32,
    lh: f32,
    c: u32,
    a: f32,
) -> f32 {
    let lines = wrap_lines(g, text, w, px, false);
    for (i, line) in lines.iter().enumerate() {
        g.text(
            line,
            rect(x, y + i as f32 * lh, w, lh),
            px,
            false,
            c,
            a,
            Align::Left,
        );
    }
    lines.len() as f32 * lh
}

unsafe fn focus_ring(ui: &Ui, g: &mut Gfx, target: Target, r: Rect, radius: f32) {
    if ui.kb && ui.focus == target {
        g.ring(r.inset(-3.0), radius + 3.0, 2.0, ACCENT, 0.9);
    }
}

unsafe fn line(g: &mut Gfx, r: Rect) {
    g.brush.SetColor(&color(WHITE, LINE_A));
    g.rt.FillRectangle(&r.d2d(), &g.brush);
}

unsafe fn draw(ui: &Ui, g: &mut Gfx) {
    let s = ui.scale;
    g.fill(rect(0.0, 0.0, WIN_W, WIN_H), WIN_R, BG, 1.0);
    g.ring(rect(0.0, 0.0, WIN_W, WIN_H), WIN_R, 1.0, WHITE, LINE_A);

    g.icon(Icon::Logo, rect(24.0, 25.0, 18.0, 18.0), WHITE, 1.0, s);
    g.text(
        window_title(ui.opts.mode),
        rect(52.0, 25.0, 360.0, 18.0),
        15.0,
        true,
        WHITE,
        1.0,
        Align::Left,
    );
    let ver = if ui.opts.preview {
        format!("v{} preview", env!("CARGO_PKG_VERSION"))
    } else {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    };
    let pw = g.measure(&ver, 12.0, false).ceil() + 24.0;
    let pill = rect(CLOSE.x - 14.0 - pw, 20.0, pw, 28.0);
    g.fill(pill, 14.0, CARD, 1.0);
    g.text(&ver, pill, 12.0, false, DIM, DIM_A, Align::Center);
    g.icon(Icon::Close, CLOSE, DIM, DIM_A, s);

    draw_rail(ui, g);
    match ui.step {
        Step::Welcome => draw_welcome(ui, g),
        Step::License => draw_license(ui, g),
        Step::Features => draw_features(ui, g),
        Step::Install => draw_install(ui, g),
        Step::Finish => draw_finish(ui, g),
        Step::Confirm => draw_confirm(ui, g),
        Step::Removing => draw_removing(ui, g),
        Step::Done => draw_done(ui, g),
    }
    draw_footer(ui, g);
}

unsafe fn draw_rail(ui: &Ui, g: &mut Gfx) {
    let s = ui.scale;
    let steps = ui.opts.mode.steps();
    let cur = ui.step.index(ui.opts.mode);
    let all_done = matches!(ui.step, Step::Finish | Step::Done);
    for (i, step) in steps.iter().enumerate() {
        let r = step_rect(i);
        let done = i < cur || all_done;
        let active = i == cur && !all_done;
        if active {
            g.fill(r, 12.0, HI, 1.0);
            g.ring(r, 12.0, 2.0, LAV, 1.0);
        }
        let m = rect(r.x + 14.0, r.y + 11.0, 22.0, 22.0);
        let num = format!("{}", i + 1);
        if done {
            g.fill(m, 11.0, LAV, 0x26 as f32 / 255.0);
            g.icon(Icon::Check, rect(m.x + 4.5, m.y + 4.5, 13.0, 13.0), ACCENT, 1.0, s);
        } else if active {
            g.fill(m, 11.0, LAV, 1.0);
            g.text(&num, m, 11.0, true, ON_ACCENT, 1.0, Align::Center);
        } else {
            g.ring(m, 11.0, 1.0, MARKER_OFF, 1.0);
            g.text(&num, m, 11.0, true, DIM, DIM_A, Align::Center);
        }
        let (c, a) = if active || done { (WHITE, 1.0) } else { (DIM, DIM_A) };
        g.text(
            step.label(),
            rect(r.x + 48.0, r.y + 13.5, r.w - 56.0, 17.0),
            14.0,
            active,
            c,
            a,
            Align::Left,
        );
    }
    g.icon(
        Icon::Svg(SHIELD_CHECK),
        rect(RAIL_X + 14.0, 498.0, 14.0, 14.0),
        DIM,
        DIM_A,
        s,
    );
    wrap_text(
        g,
        "Runs locally. Logs stay in ProgramData.",
        RAIL_X + 36.0,
        497.0,
        130.0,
        11.0,
        17.0,
        DIM,
        DIM_A,
    );
}

unsafe fn heading(g: &mut Gfx, title: &str, sub: &str) -> f32 {
    g.text(title, rect(CX, 72.0, CW, 27.0), 22.0, true, WHITE, 1.0, Align::Left);
    105.0 + wrap_text(g, sub, CX, 105.0, CW, 13.0, 19.0, DIM, DIM_A)
}

unsafe fn card_row(g: &mut Gfx, r: Rect, label: &str, caption: &str, text_w: f32, fill: u32) {
    g.fill(r, 12.0, fill, 1.0);
    g.text(
        label,
        rect(r.x + 16.0, r.y + 12.0, text_w, 17.0),
        14.0,
        false,
        WHITE,
        1.0,
        Align::Left,
    );
    g.text(
        caption,
        rect(r.x + 16.0, r.y + 32.0, text_w, 15.0),
        12.0,
        false,
        DIM,
        DIM_A,
        Align::Left,
    );
}

unsafe fn icon_row(ui: &Ui, g: &mut Gfx, y: f32, icon: Icon, label: &str, caption: &str) {
    let r = rect(CX, y, CW, 59.0);
    g.fill(r, 12.0, CARD, 1.0);
    let tile = rect(r.x + 14.0, r.y + 14.5, 30.0, 30.0);
    g.fill(tile, 9.0, LAV, 0.15);
    g.icon(icon, tile.inset(7.0), ACCENT, 1.0, ui.scale);
    g.text(
        label,
        rect(r.x + 58.0, r.y + 12.0, CW - 74.0, 17.0),
        14.0,
        false,
        WHITE,
        1.0,
        Align::Left,
    );
    g.text(
        caption,
        rect(r.x + 58.0, r.y + 32.0, CW - 74.0, 15.0),
        12.0,
        false,
        DIM,
        DIM_A,
        Align::Left,
    );
}

unsafe fn note_box(ui: &Ui, g: &mut Gfx, y: Option<f32>, icon: &'static str, text: &str) {
    let lines = wrap_lines(g, text, CW - 56.0, 12.0, false);
    let h = (lines.len() as f32 * 17.0 + 19.0).max(36.0);
    let r = rect(CX, y.unwrap_or(CONTENT_BOTTOM - h), CW, h);
    g.ring(r, 10.0, 1.0, WHITE, LINE_A);
    g.icon(Icon::Svg(icon), rect(r.x + 14.0, r.y + 10.0, 16.0, 16.0), DIM, DIM_A, ui.scale);
    for (i, l) in lines.iter().enumerate() {
        g.text(
            l,
            rect(r.x + 40.0, r.y + 10.0 + i as f32 * 17.0, CW - 56.0, 17.0),
            12.0,
            false,
            DIM,
            DIM_A,
            Align::Left,
        );
    }
}

unsafe fn draw_welcome(ui: &Ui, g: &mut Gfx) {
    heading(
        g,
        "Welcome to warmUP Companion",
        "Use your controller where Windows normally needs a keyboard.",
    );
    icon_row(
        ui,
        g,
        144.0,
        Icon::Svg(LOCK_KEYHOLE),
        "Sign-in and lock screen",
        "Pick your account and type your PIN with the controller",
    );
    icon_row(
        ui,
        g,
        211.0,
        Icon::Svg(SHIELD_CHECK),
        "Admin prompts",
        "Confirm Windows admin prompts without reaching for the mouse",
    );
    icon_row(
        ui,
        g,
        278.0,
        Icon::Keyboard,
        "On-screen keyboard",
        "Type anywhere with the gamepad keyboard",
    );
    icon_row(
        ui,
        g,
        345.0,
        Icon::Svg(MIC),
        "Offline voice typing",
        "Optional. Pick a voice engine on the Features step",
    );
    note_box(
        ui,
        g,
        None,
        INFO,
        "Companion runs as a Windows service and starts with Windows.",
    );
}

unsafe fn draw_license(ui: &Ui, g: &mut Gfx) {
    heading(
        g,
        "License",
        "warmUP Companion is open source under the MIT License.",
    );
    let card = rect(CX, 144.0, CW, CONTENT_BOTTOM - 144.0);
    g.fill(card, 12.0, CARD, 1.0);
    let text = LICENSE
        .trim()
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(|p| p.lines().map(str::trim).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n\n");
    let lines = wrap_lines(g, &text, CW - 40.0, 12.0, false);
    let lh = 18.0;
    let inner = card.inset(16.0);
    let total = lines.len() as f32 * lh;
    let max = (total - inner.h).max(0.0);
    ui.scroll_max.set(max);
    let off = ui.scroll.clamp(0.0, max);
    g.rt.PushAxisAlignedClip(&inner.d2d(), D2D1_ANTIALIAS_MODE_ALIASED);
    for (i, l) in lines.iter().enumerate() {
        let y = inner.y + i as f32 * lh - off;
        if y + lh < inner.y || y > inner.y + inner.h {
            continue;
        }
        g.text(l, rect(inner.x, y, inner.w - 8.0, lh), 12.0, false, WHITE, 0.85, Align::Left);
    }
    g.rt.PopAxisAlignedClip();
    draw_scrollbar(g, inner, total, off);
}

unsafe fn draw_scrollbar(g: &mut Gfx, area: Rect, total: f32, off: f32) {
    if total <= area.h {
        return;
    }
    let track = rect(area.x + area.w + 6.0, area.y, 4.0, area.h);
    g.fill(track, 2.0, WHITE, LINE_A);
    let th = (area.h * area.h / total).max(24.0);
    let ty = area.y + (area.h - th) * (off / (total - area.h));
    g.fill(rect(track.x, ty, 4.0, th), 2.0, DIM, DIM_A);
}

unsafe fn draw_toggle(g: &mut Gfx, r: Rect, on: bool) {
    g.fill(r, 14.0, if on { LAV } else { TOGGLE_OFF }, 1.0);
    let kx = if on { r.x + r.w - 14.0 } else { r.x + 14.0 };
    g.circle(kx, r.y + 14.0, 11.0, WHITE);
}

unsafe fn required_card(ui: &Ui, g: &mut Gfx, label: &str, caption: &str) {
    let req = rect(CX, 163.0, CW, 59.0);
    card_row(g, req, label, caption, 359.0, CARD);
    let pill = rect(req.x + 391.0, req.y + 17.0, 89.0, 26.0);
    g.fill(pill, 13.0, HI, 1.0);
    g.icon(
        Icon::Svg(LOCK),
        rect(pill.x + 10.0, pill.y + 7.0, 12.0, 12.0),
        DIM,
        DIM_A,
        ui.scale,
    );
    g.text(
        "Required",
        rect(pill.x + 28.0, pill.y + 5.5, 56.0, 15.0),
        12.0,
        false,
        DIM,
        DIM_A,
        Align::Left,
    );
}

unsafe fn toggle_card(ui: &Ui, g: &mut Gfx, label: &str, caption: &str, on: bool) {
    let r = voice_row();
    card_row(g, r, label, caption, 402.0, if on { HI } else { CARD });
    if on {
        g.ring(r, 12.0, 2.0, LAV, 1.0);
    }
    focus_ring(ui, g, Target::Voice, r, 12.0);
    draw_toggle(g, rect(r.x + 434.0, r.y + 16.0, 46.0, 28.0), on);
}

unsafe fn draw_confirm(ui: &Ui, g: &mut Gfx) {
    heading(
        g,
        "Remove warmUP Companion",
        &format!(
            "This stops and removes the Companion service. Logs and diagnostics stay in {} for troubleshooting.",
            crate::install::DATA_DIR
        ),
    );
    required_card(
        ui,
        g,
        "warmUP Companion service",
        "Service, tray helper and installed program files",
    );
    if has_models(ui) {
        toggle_card(
            ui,
            g,
            "Offline voice models",
            &format!(
                "{} on disk. Keep them if you plan to reinstall",
                format_bytes(ui.models_bytes)
            ),
            ui.models_on,
        );
    }
    note_box(
        ui,
        g,
        None,
        INFO,
        "Your controller stops working on the sign-in screen, the lock screen and admin prompts once the service is removed.",
    );
}

unsafe fn draw_removing(ui: &Ui, g: &mut Gfx) {
    let job = &ui.job;
    let (title, sub) = match job.phase {
        Phase::Done => (
            "warmUP Companion was removed",
            "The service is stopped and deleted.",
        ),
        Phase::Failed if job.needs_admin => (
            "Administrator permission needed",
            "Windows asks for permission before the service can be removed.",
        ),
        Phase::Failed => (
            "Uninstall could not finish",
            "Nothing else was changed. You can retry or close.",
        ),
        _ => (
            "Removing warmUP Companion",
            "This takes a few seconds.",
        ),
    };
    let mut rows = vec![("warmUP Companion service".to_string(), job.service)];
    if ui.models_on && has_models(ui) {
        rows.push((
            format!("Offline voice models \u{00B7} {}", format_bytes(ui.models_bytes)),
            job.voice,
        ));
    }
    draw_progress(ui, g, title, sub, &rows);
}

unsafe fn draw_done(ui: &Ui, g: &mut Gfx) {
    heading(
        g,
        "warmUP Companion was removed",
        "Your controller no longer works on the sign-in screen or admin prompts.",
    );
    let models_removed = ui.models_on && has_models(ui) && ui.job.voice != Task::Failed;
    icon_row(
        ui,
        g,
        144.0,
        Icon::Svg(CIRCLE_CHECK),
        "Removed: Companion service",
        "Service, tray helper and program files",
    );
    let mut y = 211.0;
    if has_models(ui) {
        let size = format_bytes(ui.models_bytes);
        if models_removed {
            icon_row(
                ui,
                g,
                y,
                Icon::Svg(CIRCLE_CHECK),
                "Removed: Offline voice models",
                &format!("{size} freed"),
            );
        } else {
            icon_row(
                ui,
                g,
                y,
                Icon::Svg(MIC),
                "Kept: Offline voice models",
                &format!("{size} in {}", speech_dir().display()),
            );
        }
        y += 67.0;
    }
    icon_row(
        ui,
        g,
        y,
        Icon::Svg(HARD_DRIVE),
        "Kept: Logs and diagnostics",
        crate::install::DATA_DIR,
    );
    if !ui.job.note.is_empty() {
        note_box(ui, g, Some(y + 79.0), ALERT, &ui.job.note);
    }
}

unsafe fn draw_features(ui: &Ui, g: &mut Gfx) {
    heading(
        g,
        "Choose features",
        "The Companion service is required. Offline voice typing runs on your PC and needs a one-time model download.",
    );

    required_card(
        ui,
        g,
        "warmUP Companion service",
        "Controller input on sign-in, lock and admin screens",
    );
    toggle_card(
        ui,
        g,
        "Offline voice typing",
        "Speech is recognised on your PC, never in the cloud",
        ui.voice_on,
    );

    let on = ui.voice_on;
    let fade = if on { 1.0 } else { 0.4 };
    g.text(
        "VOICE ENGINE",
        rect(CX + 4.0, 307.0, 200.0, 13.0),
        11.0,
        true,
        DIM,
        DIM_A * fade,
        Align::Left,
    );
    g.text(
        "Change later from the tray",
        rect(CX + CW - 4.0 - 240.0, 307.0, 240.0, 13.0),
        11.0,
        false,
        DIM,
        HINT_A * fade,
        Align::Right,
    );
    let list = engine_list();
    g.fill(list, 12.0, CARD, 1.0);
    for (i, model) in Model::ALL.iter().enumerate() {
        let r = engine_row(i);
        let sel = on && ui.model == *model;
        if sel {
            g.fill(r, 8.0, LAV, 0x1F as f32 / 255.0);
        }
        focus_ring(ui, g, Target::Engine(i), r, 8.0);
        let radio = rect(r.x + 10.0, r.y + 9.0, 18.0, 18.0);
        if sel {
            g.fill(radio, 9.0, WHITE, 1.0);
            g.ring(radio, 9.0, 5.0, LAV, 1.0);
        } else {
            g.ring(radio, 9.0, 1.5, RADIO_OFF, fade);
        }
        let mut x = r.x + 40.0;
        let tw = g.measure(model.title(), 13.0, sel);
        g.text(
            model.title(),
            rect(x, r.y + 10.0, tw + 4.0, 16.0),
            13.0,
            sel,
            WHITE,
            fade,
            Align::Left,
        );
        x += tw + 8.0;
        let cw = g.measure(model.caption(), 12.0, false);
        g.text(
            model.caption(),
            rect(x, r.y + 10.5, cw + 4.0, 15.0),
            12.0,
            false,
            DIM,
            DIM_A * fade,
            Align::Left,
        );
        x += cw + 8.0;
        if *model == Model::Parakeet {
            let bw = g.measure("RECOMMENDED", 9.0, true).ceil() + 14.0;
            let badge = rect(x, r.y + 10.5, bw, 15.0);
            g.fill(badge, 7.5, BADGE, fade);
            g.text("RECOMMENDED", badge, 9.0, true, WHITE, fade, Align::Center);
        }
        let sz = rect(r.x + r.w - 10.0 - 90.0, r.y + 10.5, 90.0, 15.0);
        if ui.installed[i] {
            g.text("Installed", sz, 12.0, false, ACCENT, fade, Align::Right);
        } else {
            g.text(model.size(), sz, 12.0, false, DIM, DIM_A * fade, Align::Right);
        }
    }
}

unsafe fn task_row(ui: &Ui, g: &mut Gfx, y: f32, label: &str, task: Task) {
    let s = ui.scale;
    let ir = rect(CX + 16.0, y, 18.0, 18.0);
    match task {
        Task::Done => g.icon(Icon::Svg(CIRCLE_CHECK), ir, LAV, 1.0, s),
        Task::Failed => g.icon(Icon::Svg(CIRCLE_X), ir, DANGER, 1.0, s),
        Task::Pending => g.icon(Icon::Svg(CIRCLE), ir, DIM, DIM_A, s),
        Task::Running => {
            let a = ui.started.elapsed().as_secs_f32() * std::f32::consts::TAU * 0.9;
            let (sn, cs) = a.sin_cos();
            let (cx, cy) = (ir.x + 9.0, ir.y + 9.0);
            g.rt.SetTransform(&Matrix3x2 {
                M11: s * cs,
                M12: s * sn,
                M21: -s * sn,
                M22: s * cs,
                M31: s * (cx - cs * cx + sn * cy) + PAD_X * s,
                M32: s * (cy - sn * cx - cs * cy) + PAD_TOP * s,
            });
            g.icon(Icon::Svg(LOADER), ir, LAV, 1.0, s);
            g.transform(s, PAD_X * s, PAD_TOP * s);
        }
    }
    let (c, a) = if task == Task::Pending { (DIM, DIM_A) } else { (WHITE, 1.0) };
    g.text(
        label,
        rect(CX + 46.0, y + 0.5, CW - 62.0, 17.0),
        14.0,
        task == Task::Running,
        c,
        a,
        Align::Left,
    );
}

unsafe fn draw_install(ui: &Ui, g: &mut Gfx) {
    let job = &ui.job;
    let mut rows = vec![("warmUP Companion service".to_string(), job.service)];
    if ui.voice_on {
        rows.push((
            format!("Offline voice typing \u{00B7} {}", ui.model.title()),
            job.voice,
        ));
    }
    let (title, sub) = match job.phase {
        Phase::Done => (
            "warmUP Companion is installed",
            "The service is running and starts with Windows.",
        ),
        Phase::Failed if job.needs_admin => (
            "Administrator permission needed",
            "Windows asks for permission before setup can install the service.",
        ),
        Phase::Failed => (
            "Setup could not finish",
            "Nothing else was changed. You can retry or go back.",
        ),
        _ => (
            "Installing warmUP Companion",
            "This takes about a minute. Voice models can take longer to download.",
        ),
    };
    draw_progress(ui, g, title, sub, &rows);
}

unsafe fn draw_progress(ui: &Ui, g: &mut Gfx, title: &str, sub: &str, rows: &[(String, Task)]) {
    let job = &ui.job;
    heading(g, title, sub);

    let status = if job.status.is_empty() {
        "Preparing\u{2026}"
    } else {
        job.status.as_str()
    };
    g.text(status, rect(CX, 148.0, CW, 16.0), 13.0, true, WHITE, 1.0, Align::Left);
    let track = rect(CX, 174.0, CW, 6.0);
    g.fill(track, 3.0, CARD, 1.0);
    match job.phase {
        Phase::Done => g.fill(track, 3.0, LAV, 1.0),
        Phase::Failed => {}
        _ if job.progress.is_some() => {
            let w = CW * job.progress.unwrap_or(0.0);
            if w > 0.0 {
                g.fill(rect(CX, track.y, w.max(6.0), 6.0), 3.0, LAV, 1.0);
            }
        }
        _ => {
            let t = (ui.started.elapsed().as_secs_f32() * 0.6).fract();
            let seg = CW * 0.3;
            let x0 = (CX - seg + t * (CW + seg)).max(CX);
            let x1 = (CX - seg + t * (CW + seg) + seg).min(CX + CW);
            if x1 > x0 {
                g.fill(rect(x0, track.y, x1 - x0, 6.0), 3.0, LAV, 1.0);
            }
        }
    }

    for (i, (label, task)) in rows.iter().enumerate() {
        task_row(ui, g, 210.0 + i as f32 * 32.0, label, *task);
    }

    let msg = if job.phase == Phase::Failed {
        Some((ALERT, job.error.as_str(), true))
    } else if !job.note.is_empty() {
        Some((INFO, job.note.as_str(), false))
    } else {
        None
    };
    if let Some((icon, text, danger)) = msg {
        let top = 290.0;
        let lines = wrap_lines(g, text, CW - 64.0, 13.0, false);
        let lh = 19.0;
        let total = lines.len() as f32 * lh;
        let h = (total + 28.0).min(CONTENT_BOTTOM - top);
        let r = rect(CX, top, CW, h);
        g.fill(r, 12.0, CARD, 1.0);
        if danger {
            g.ring(r, 12.0, 1.0, DANGER, 0.5);
        }
        g.icon(
            Icon::Svg(icon),
            rect(r.x + 16.0, r.y + 15.5, 16.0, 16.0),
            if danger { DANGER } else { DIM },
            if danger { 1.0 } else { DIM_A },
            ui.scale,
        );
        let inner = rect(r.x + 44.0, r.y + 14.0, CW - 64.0, h - 28.0);
        let max = (total - inner.h).max(0.0);
        ui.scroll_max.set(max);
        let off = ui.scroll.clamp(0.0, max);
        g.rt.PushAxisAlignedClip(&inner.d2d(), D2D1_ANTIALIAS_MODE_ALIASED);
        for (i, l) in lines.iter().enumerate() {
            g.text(
                l,
                rect(inner.x, inner.y + i as f32 * lh - off, inner.w, lh),
                13.0,
                false,
                WHITE,
                1.0,
                Align::Left,
            );
        }
        g.rt.PopAxisAlignedClip();
        draw_scrollbar(g, rect(inner.x, inner.y, inner.w - 2.0, inner.h), total, off);
    } else {
        ui.scroll_max.set(0.0);
    }
}

unsafe fn draw_finish(ui: &Ui, g: &mut Gfx) {
    heading(
        g,
        "warmUP Companion is ready",
        "Your controller now works on the sign-in screen, the lock screen and admin prompts.",
    );
    let r = link_row();
    g.fill(r, 12.0, CARD, 1.0);
    g.text(
        "Get the warmUP Game Launcher",
        rect(r.x + 16.0, r.y + 12.0, 402.0, 17.0),
        14.0,
        true,
        WHITE,
        1.0,
        Align::Left,
    );
    g.text(
        "All your PC games in one controller-friendly home",
        rect(r.x + 16.0, r.y + 32.0, 402.0, 15.0),
        12.0,
        false,
        DIM,
        DIM_A,
        Align::Left,
    );
    g.icon(
        Icon::ChevronRight,
        rect(r.x + r.w - 36.0, r.y + 20.5, 18.0, 18.0),
        ACCENT,
        1.0,
        ui.scale,
    );
    focus_ring(ui, g, Target::Link, r, 12.0);
    if !ui.job.note.is_empty() {
        note_box(ui, g, Some(219.0), ALERT, &ui.job.note);
    }
    note_box(
        ui,
        g,
        None,
        INFO,
        "Open the Companion from the tray to change the voice engine or controller settings.",
    );
}

unsafe fn footer_info(ui: &Ui) -> Option<(&'static str, String)> {
    let data = crate::install::DATA_DIR.to_string();
    match ui.step {
        Step::Welcome => Some((HARD_DRIVE, data)),
        Step::License => None,
        Step::Features => {
            let text = if !ui.voice_on {
                "No download".to_string()
            } else if ui.installed[ui.model.index()] {
                "Already installed".to_string()
            } else {
                ui.model.download().to_string()
            };
            Some((HARD_DRIVE, text))
        }
        Step::Install => Some((HARD_DRIVE, data)),
        Step::Finish => Some((CIRCLE_CHECK, format!("Installed in {data}"))),
        Step::Confirm if ui.models_on && has_models(ui) => Some((
            HARD_DRIVE,
            format!("Frees {}", format_bytes(ui.models_bytes)),
        )),
        Step::Confirm | Step::Removing => Some((HARD_DRIVE, data)),
        Step::Done => Some((CIRCLE_CHECK, format!("Logs kept in {data}"))),
    }
}

unsafe fn draw_footer(ui: &Ui, g: &mut Gfx) {
    let s = ui.scale;
    line(g, rect(0.0, FOOTER_Y, WIN_W, 1.0));
    if ui.pad.connected() {
        let mut hints = Vec::new();
        if next_label(ui).is_some() {
            hints.push((0, "Select"));
        }
        if let Some(label) = back_label(ui) {
            hints.push((1, label));
        }
        let mut x = 24.0;
        for (glyph, label) in hints {
            g.icon(Icon::Pad(ui.pad.playstation(), glyph), rect(x, 583.0, 22.0, 22.0), WHITE, 1.0, s);
            x += 28.0;
            let lw = g.measure(label, 13.0, false);
            g.text(label, rect(x, 583.0, lw + 4.0, 22.0), 13.0, false, DIM, DIM_A, Align::Left);
            x += lw + 16.0;
        }
    } else if let Some((icon, text)) = footer_info(ui) {
        g.icon(Icon::Svg(icon), rect(24.0, 586.0, 16.0, 16.0), DIM, DIM_A, s);
        g.text(&text, rect(48.0, 586.5, 420.0, 15.0), 12.0, false, DIM, DIM_A, Align::Left);
    }
    let mut right = CX + CW;
    ui.next_r.set(None);
    ui.back_r.set(None);
    if let Some(label) = next_label(ui) {
        let w = (g.measure(label, 15.0, true).ceil() + 52.0).max(88.0);
        let r = rect(right - w, BTN_Y, w, BTN_H);
        g.fill(r, 12.0, ACCENT, 1.0);
        focus_ring(ui, g, Target::Next, r, 12.0);
        g.text(label, r, 15.0, true, ON_ACCENT, 1.0, Align::Center);
        ui.next_r.set(Some(r));
        right = r.x - 10.0;
    }
    if let Some(label) = back_label(ui) {
        let w = (g.measure(label, 15.0, true).ceil() + 52.0).max(88.0);
        let r = rect(right - w, BTN_Y, w, BTN_H);
        g.fill(r, 12.0, CARD, 1.0);
        focus_ring(ui, g, Target::Back, r, 12.0);
        g.text(label, r, 15.0, true, WHITE, 1.0, Align::Center);
        ui.back_r.set(Some(r));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let sx = (lparam.0 & 0xffff) as i16 as i32 - wr.left;
            let sy = ((lparam.0 >> 16) & 0xffff) as i16 as i32 - wr.top;
            let mut caption = false;
            with_ui(|ui| {
                let x = sx as f32 / ui.scale - PAD_X;
                let y = sy as f32 / ui.scale - PAD_TOP;
                caption = (0.0..WIN_W).contains(&x)
                    && (0.0..TITLE_H).contains(&y)
                    && !CLOSE.inset(-7.0).contains(x, y);
            });
            LRESULT(if caption { HTCAPTION } else { HTCLIENT } as isize)
        }
        WM_LBUTTONDOWN => {
            with_ui(|ui| {
                let (x, y) = design_point(ui, lparam);
                click(ui, x, y);
                paint(ui);
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            with_ui(|ui| {
                let delta = ((wparam.0 >> 16) & 0xffff) as i16 as f32;
                ui.scroll = (ui.scroll - delta / 120.0 * 54.0).clamp(0.0, ui.scroll_max.get());
                paint(ui);
            });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            with_ui(|ui| {
                key_down(ui, wparam.0 as u16);
                ui.scroll = ui.scroll.clamp(0.0, ui.scroll_max.get());
                paint(ui);
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            with_ui(|ui| {
                let changed = poll_job(ui);
                let focused = GetForegroundWindow() == ui.hwnd;
                let (presses, ry, pad_changed) = ui.pad.poll(focused);
                for bit in &presses {
                    pad_press(ui, *bit);
                }
                if ry != 0.0 {
                    ui.scroll -= ry * 18.0;
                }
                ui.scroll = ui.scroll.clamp(0.0, ui.scroll_max.get());
                let animating = matches!(ui.step, Step::Install | Step::Removing)
                    && ui.job.phase == Phase::Running;
                if changed || animating || pad_changed || ry != 0.0 || !presses.is_empty() {
                    paint(ui);
                }
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            with_ui(|ui| {
                let suggested = &*(lparam.0 as *const RECT);
                if let Some(g) = ui.gfx.as_mut() {
                    g.icons.clear();
                }
                ui.scale = ((wparam.0 >> 16) & 0xffff).max(96) as f32 / 96.0;
                place(ui, Some((suggested.left, suggested.top)));
                paint(ui);
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            with_ui(|ui| request_close(ui));
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_ID);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
