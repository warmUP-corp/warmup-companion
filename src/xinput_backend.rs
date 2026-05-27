//! XInput polling for Session-0 service / secure desktop (sign-in, UAC).
//!
//! Joyxoff insight: XInputGetState returns neutral (zeroed) state to processes
//! that have no foreground-eligible window on the input desktop. Mitigation: the
//! secure poll thread runs a real Win32 UI message pump and owns a tiny anchor
//! window on the Winlogon desktop. The XInputGetState call then happens on the
//! same thread that owns a window on the input desktop, matching Joyxoff's
//! `SetTimer(NULL, ..., FUN_0044e890)` pattern on a `JoyXoffMWindow` thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::{w, PCSTR};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidP_GetUsagesEx, HidP_Input, HIDP_STATUS_SUCCESS, PHIDP_PREPARSED_DATA, USAGE_AND_PAGE,
};
use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, HRAWINPUT, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RIDI_PREPARSEDDATA, RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEHID,
};
use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_DPAD_DOWN,
    XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP,
    XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_X,
    XINPUT_GAMEPAD_Y, XINPUT_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    PostThreadMessageW, RegisterClassW, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    HMENU, HWND_TOPMOST, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WM_DESTROY,
    WM_INPUT, WM_NULL, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

use crate::gamepad_backend::ButtonChange;
use crate::gamepad_backend::GamepadBackend;

const SLOTS: u32 = 4;
const ERROR_SUCCESS: u32 = 0;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const ERROR_EMPTY: u32 = 4306;

const LEFT_DEADZONE: i16 = 7849;
const RIGHT_DEADZONE: i16 = 8689;

type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;
type XInputGetKeystrokeFn = unsafe extern "system" fn(u32, u32, *mut XInputKeystroke) -> u32;

const XINPUT_KEYSTROKE_KEYDOWN: u16 = 0x0001;
const XINPUT_KEYSTROKE_KEYUP: u16 = 0x0002;

const VK_PAD_A: u16 = 0x5800;
const VK_PAD_B: u16 = 0x5801;
const VK_PAD_X: u16 = 0x5802;
const VK_PAD_Y: u16 = 0x5803;
const VK_PAD_LSHOULDER: u16 = 0x5804;
const VK_PAD_RSHOULDER: u16 = 0x5805;
const VK_PAD_DPAD_UP: u16 = 0x5810;
const VK_PAD_DPAD_DOWN: u16 = 0x5811;
const VK_PAD_DPAD_LEFT: u16 = 0x5812;
const VK_PAD_DPAD_RIGHT: u16 = 0x5813;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputKeystroke {
    virtual_key: u16,
    unicode: u16,
    flags: u16,
    user_index: u8,
    hid_code: u8,
}

fn button_masks() -> [(&'static str, u16); 10] {
    [
        ("UP", XINPUT_GAMEPAD_DPAD_UP.0),
        ("DOWN", XINPUT_GAMEPAD_DPAD_DOWN.0),
        ("LEFT", XINPUT_GAMEPAD_DPAD_LEFT.0),
        ("RIGHT", XINPUT_GAMEPAD_DPAD_RIGHT.0),
        ("A", XINPUT_GAMEPAD_A.0),
        ("B", XINPUT_GAMEPAD_B.0),
        ("X", XINPUT_GAMEPAD_X.0),
        ("Y", XINPUT_GAMEPAD_Y.0),
        ("LB", XINPUT_GAMEPAD_LEFT_SHOULDER.0),
        ("RB", XINPUT_GAMEPAD_RIGHT_SHOULDER.0),
    ]
}

pub struct XInputBackend {
    _module: Option<HMODULE>,
    get_state: Option<XInputGetStateFn>,
    get_keystroke: Option<XInputGetKeystrokeFn>,
    secure: Option<SecurePollThread>,
    active_slot: Option<u32>,
    prev_buttons: [u16; 4],
    slot_connected: [bool; 4],
    pending: Vec<ButtonChange>,
    axes: (f32, f32, f32, f32),
    last_status_log: Instant,
    last_no_pad_log: Instant,
    last_raw_log: Instant,
    last_secure_check: Instant,
    /// Consecutive input-desktop probes that were not Winlogon while helper runs.
    secure_leave_winlogon_streak: u8,
}

#[allow(dead_code)]
struct SecurePollThread {
    rx: mpsc::Receiver<SecureMsg>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    thread_id: u32,
}

#[allow(dead_code)]
const ANCHOR_CLASS: windows::core::PCWSTR = w!("WarmupXInputAnchorWindow");
#[allow(dead_code)]
const POLL_TIMER_ID: usize = 21;
#[allow(dead_code)]
const POLL_TIMER_MS: u32 = 8;

#[allow(dead_code)]
struct PollState {
    get_state: XInputGetStateFn,
    get_keystroke: Option<XInputGetKeystrokeFn>,
    tx: mpsc::Sender<SecureMsg>,
    prev_buttons: [u16; 4],
    active_slot: Option<u32>,
    connected_prev: [bool; 4],
    last_status: Instant,
    last_no_pad: Instant,
    last_probe_log: Instant,
    iter_count: u64,
    raw_preparsed: HashMap<usize, Vec<usize>>,
    raw_diag_count: u32,
    /// Latest button mask from raw HID (XUSB); merged when XInputGetState is neutral.
    last_raw_buttons: u16,
}

thread_local! {
    static POLL_STATE: RefCell<Option<PollState>> = const { RefCell::new(None) };
}

enum SecureMsg {
    Ready(String),
    Slots([bool; 4]),
    Buttons { slot: u32, prev: u16, cur: u16 },
    Axes((f32, f32, f32, f32)),
    NoController,
    Error(String),
}

impl XInputBackend {
    pub fn new() -> Self {
        let (module, get_state, get_keystroke) = load_xinput_api();
        Self {
            _module: module,
            get_state,
            get_keystroke,
            secure: None,
            active_slot: None,
            prev_buttons: [0; 4],
            slot_connected: [false; 4],
            pending: Vec::new(),
            axes: (0.0, 0.0, 0.0, 0.0),
            last_status_log: Instant::now() - Duration::from_secs(60),
            last_no_pad_log: Instant::now() - Duration::from_secs(60),
            last_raw_log: Instant::now() - Duration::from_secs(60),
            last_secure_check: Instant::now() - Duration::from_secs(60),
            secure_leave_winlogon_streak: 0,
        }
    }

    fn input_is_winlogon(&mut self) -> bool {
        let winlogon = match crate::win::input_desktop_name() {
            Ok(name) => name.eq_ignore_ascii_case("Winlogon"),
            Err(_) => false,
        };
        if winlogon {
            self.secure_leave_winlogon_streak = 0;
            return true;
        }
        self.secure_leave_winlogon_streak = self.secure_leave_winlogon_streak.saturating_add(1);
        self.secure.is_some() && self.secure_leave_winlogon_streak < 12
    }

    fn get_state(&self, slot: u32, state: &mut XINPUT_STATE) -> u32 {
        match self.get_state {
            Some(f) => unsafe { f(slot, state) },
            None => ERROR_DEVICE_NOT_CONNECTED,
        }
    }

    fn get_keystroke(&self, slot: u32, key: &mut XInputKeystroke) -> u32 {
        match self.get_keystroke {
            Some(f) => unsafe { f(slot, 0, key) },
            None => ERROR_EMPTY,
        }
    }

    fn log_slots_if_changed(&mut self, connected: [bool; 4]) {
        let changed = connected
            .iter()
            .zip(self.slot_connected.iter())
            .any(|(a, b)| a != b);
        if !changed && self.last_status_log.elapsed() < Duration::from_secs(30) {
            return;
        }
        if changed || self.last_status_log.elapsed() >= Duration::from_secs(30) {
            self.last_status_log = Instant::now();
            let summary: Vec<String> = (0..SLOTS)
                .map(|i| {
                    if connected[i as usize] {
                        format!("{i}:connected")
                    } else {
                        format!("{i}:empty")
                    }
                })
                .collect();
            service_log(&format!("XInput slots [{}]", summary.join(", ")));
        }
        self.slot_connected = connected;
    }

    fn log_no_controller(&mut self) {
        if self.last_no_pad_log.elapsed() >= Duration::from_secs(15) {
            self.last_no_pad_log = Instant::now();
            service_log("XInput: no controller connected (retrying)");
        }
    }

    fn pick_active_slot(&mut self, connected: &[bool; 4]) -> Option<u32> {
        if let Some(slot) = self.active_slot {
            if connected[slot as usize] {
                return Some(slot);
            }
            self.active_slot = None;
        }
        for i in 0..SLOTS {
            if connected[i as usize] {
                self.active_slot = Some(i);
                service_log(&format!("XInput: using slot {i}"));
                return Some(i);
            }
        }
        None
    }

    fn norm_thumb(value: i16, deadzone: i16) -> f32 {
        let v = value as f32;
        if v.abs() < deadzone as f32 {
            return 0.0;
        }
        (v / 32767.0).clamp(-1.0, 1.0)
    }

    fn edges(prev: u16, cur: u16) -> Vec<ButtonChange> {
        let mut out = Vec::new();
        for (name, mask) in button_masks() {
            let was = prev & mask != 0;
            let now = cur & mask != 0;
            if was != now {
                out.push(ButtonChange {
                    button_name: name,
                    pressed: now,
                });
            }
        }
        out
    }

    fn log_button_change(&mut self, slot: u32, prev: u16, cur: u16) {
        if prev == cur {
            return;
        }
        let names: Vec<&str> = button_masks()
            .iter()
            .filter_map(|(name, mask)| if cur & *mask != 0 { Some(*name) } else { None })
            .collect();
        service_log(&format!(
            "XInput buttons slot {slot}: 0x{prev:04x} -> 0x{cur:04x} [{}]",
            names.join("+")
        ));
        crate::debug_state::record_xinput_buttons(cur, &names.join("+"));
        self.last_raw_log = Instant::now();
    }

    fn poll_keystrokes(&mut self, slot: u32) {
        for _ in 0..16 {
            let mut key = XInputKeystroke::default();
            let err = self.get_keystroke(slot, &mut key);
            if err == ERROR_EMPTY || err == ERROR_DEVICE_NOT_CONNECTED {
                break;
            }
            if err != ERROR_SUCCESS {
                service_log(&format!("XInputGetKeystroke({slot}) error {err}"));
                break;
            }
            service_log(&format!(
                "XInput keystroke slot {slot}: vk=0x{:04x} flags=0x{:04x} user={} hid=0x{:02x}",
                key.virtual_key, key.flags, key.user_index, key.hid_code
            ));
            let Some(mask) = key_to_mask(key.virtual_key) else {
                continue;
            };
            let idx = slot as usize;
            let prev = self.prev_buttons[idx];
            let mut cur = prev;
            if key.flags & XINPUT_KEYSTROKE_KEYDOWN != 0 {
                cur |= mask;
            }
            if key.flags & XINPUT_KEYSTROKE_KEYUP != 0 {
                cur &= !mask;
            }
            if prev != cur {
                self.prev_buttons[idx] = cur;
                self.log_button_change(slot, prev, cur);
                self.pending.extend(Self::edges(prev, cur));
            }
        }
    }

    fn sync_secure_helper(&mut self) -> bool {
        if self.last_secure_check.elapsed() >= Duration::from_millis(250) {
            self.last_secure_check = Instant::now();
            let on_winlogon = self.input_is_winlogon();
            match (on_winlogon, self.secure.is_some()) {
                (true, false) => match SecurePollThread::spawn() {
                    Ok(thread) => {
                        service_log("XInput secure helper: starting on input desktop");
                        self.secure = Some(thread);
                    }
                    Err(e) => service_log(&format!("XInput secure helper: spawn failed: {e}")),
                },
                (false, true) => {
                    service_log("XInput secure helper: stopping");
                    self.secure.take();
                    self.secure_leave_winlogon_streak = 0;
                }
                _ => {}
            }
        }

        let Some(secure) = self.secure.as_ref() else {
            return false;
        };

        let mut messages = Vec::new();
        while let Ok(msg) = secure.rx.try_recv() {
            messages.push(msg);
        }
        let got_msg = !messages.is_empty();
        for msg in messages {
            match msg {
                SecureMsg::Ready(desktop) => {
                    service_log(&format!("XInput secure helper: thread on {desktop}"));
                }
                SecureMsg::Slots(connected) => self.log_slots_if_changed(connected),
                SecureMsg::Buttons { slot, prev, cur } => {
                    self.log_button_change(slot, prev, cur);
                    self.pending.extend(Self::edges(prev, cur));
                }
                SecureMsg::Axes(axes) => self.axes = axes,
                SecureMsg::NoController => {
                    self.axes = (0.0, 0.0, 0.0, 0.0);
                    self.log_no_controller();
                }
                SecureMsg::Error(e) => service_log(&format!("XInput secure helper: {e}")),
            }
        }
        got_msg || self.secure.is_some()
    }
}

impl SecurePollThread {
    fn spawn() -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (tid_tx, tid_rx) = mpsc::sync_channel::<u32>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("warmup-xinput-winlogon".into())
            .spawn(move || secure_poll_main(tx, worker_stop, tid_tx))
            .map_err(|e| format!("xinput secure thread: {e}"))?;
        let thread_id = tid_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|e| format!("xinput secure thread tid: {e}"))?;
        Ok(Self {
            rx,
            stop,
            join: Some(join),
            thread_id,
        })
    }
}

impl Drop for SecurePollThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe {
            // Wake the message pump so it observes `stop` and exits GetMessage.
            let _ = PostThreadMessageW(self.thread_id, WM_NULL, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl GamepadBackend for XInputBackend {
    fn poll(&mut self) -> Result<(), String> {
        self.pending.clear();
        // Keep the Winlogon-attached helper alive for diagnostics/fallback, but
        // still poll XInput from the service worker's Default desktop. Joyxoff's
        // controller loop runs from its normal UI/render thread; attaching the
        // polling thread to Winlogon can produce packet changes with neutral
        // buttons on some systems.
        let _ = self.sync_secure_helper();

        // Winlogon helper owns real button state; primary GetState on Default returns
        // neutral masks and would clobber secure edges if we merged them here.
        if self.secure.is_some() {
            for slot in 0..SLOTS {
                self.poll_keystrokes(slot);
            }
            return Ok(());
        }

        let mut connected = [false; 4];
        let mut states: [Option<XINPUT_GAMEPAD>; 4] = [None; 4];

        for slot in 0..SLOTS {
            let mut state = XINPUT_STATE::default();
            let err = self.get_state(slot, &mut state);
            if err == ERROR_SUCCESS {
                connected[slot as usize] = true;
                states[slot as usize] = Some(state.Gamepad);
            } else if err != ERROR_DEVICE_NOT_CONNECTED {
                service_log(&format!("XInputGetState({slot}) error {err}"));
            }
        }

        self.log_slots_if_changed(connected);

        let Some(slot) = self.pick_active_slot(&connected) else {
            self.axes = (0.0, 0.0, 0.0, 0.0);
            self.log_no_controller();
            return Ok(());
        };

        let pad = states[slot as usize].expect("connected slot has state");
        let idx = slot as usize;
        let prev = self.prev_buttons[idx];
        let cur = pad.wButtons.0;
        self.prev_buttons[idx] = cur;
        self.log_button_change(slot, prev, cur);
        self.pending.extend(Self::edges(prev, cur));
        self.poll_keystrokes(slot);
        self.axes = (
            Self::norm_thumb(pad.sThumbLX, LEFT_DEADZONE),
            Self::norm_thumb(pad.sThumbLY, LEFT_DEADZONE),
            Self::norm_thumb(pad.sThumbRX, RIGHT_DEADZONE),
            Self::norm_thumb(pad.sThumbRY, RIGHT_DEADZONE),
        );
        Ok(())
    }

    fn button_changes(&mut self) -> Vec<ButtonChange> {
        std::mem::take(&mut self.pending)
    }

    fn axes(&self) -> (f32, f32, f32, f32) {
        self.axes
    }

    fn controller_label(&self) -> String {
        match self.active_slot {
            Some(i) => format!("XInput slot {i}"),
            None => "none".to_string(),
        }
    }
}

fn key_to_mask(vk: u16) -> Option<u16> {
    Some(match vk {
        VK_PAD_DPAD_UP => XINPUT_GAMEPAD_DPAD_UP.0,
        VK_PAD_DPAD_DOWN => XINPUT_GAMEPAD_DPAD_DOWN.0,
        VK_PAD_DPAD_LEFT => XINPUT_GAMEPAD_DPAD_LEFT.0,
        VK_PAD_DPAD_RIGHT => XINPUT_GAMEPAD_DPAD_RIGHT.0,
        VK_PAD_A => XINPUT_GAMEPAD_A.0,
        VK_PAD_B => XINPUT_GAMEPAD_B.0,
        VK_PAD_X => XINPUT_GAMEPAD_X.0,
        VK_PAD_Y => XINPUT_GAMEPAD_Y.0,
        VK_PAD_LSHOULDER => XINPUT_GAMEPAD_LEFT_SHOULDER.0,
        VK_PAD_RSHOULDER => XINPUT_GAMEPAD_RIGHT_SHOULDER.0,
        _ => return None,
    })
}

#[allow(dead_code)]
fn hid_usage_to_mask(page: u16, usage: u16) -> Option<u16> {
    Some(match (page, usage) {
        (0x09, 1) => XINPUT_GAMEPAD_A.0,
        (0x09, 2) => XINPUT_GAMEPAD_B.0,
        (0x09, 3) => XINPUT_GAMEPAD_X.0,
        (0x09, 4) => XINPUT_GAMEPAD_Y.0,
        (0x09, 5) => XINPUT_GAMEPAD_LEFT_SHOULDER.0,
        (0x09, 6) => XINPUT_GAMEPAD_RIGHT_SHOULDER.0,
        (0x09, 12) => XINPUT_GAMEPAD_DPAD_UP.0,
        (0x09, 13) => XINPUT_GAMEPAD_DPAD_DOWN.0,
        (0x09, 14) => XINPUT_GAMEPAD_DPAD_LEFT.0,
        (0x09, 15) => XINPUT_GAMEPAD_DPAD_RIGHT.0,
        _ => return None,
    })
}

/// Standard XUSB 16-bit button field (dpad, shoulders, face buttons).
const XUSB_BUTTON_MASK: u16 = 0xF3FF;

/// Xbox 360 / XUSB gamepad input reports expose a 16-bit button field at bytes 2–3.
fn xusb_buttons_from_report(report: &[u8]) -> Option<u16> {
    if report.len() < 4 {
        return None;
    }
    for buttons in [
        u16::from_be_bytes([report[2], report[3]]),
        u16::from_le_bytes([report[2], report[3]]),
    ] {
        let masked = buttons & XUSB_BUTTON_MASK;
        if masked != 0 {
            return Some(masked);
        }
    }
    None
}

fn load_xinput_api() -> (
    Option<HMODULE>,
    Option<XInputGetStateFn>,
    Option<XInputGetKeystrokeFn>,
) {
    unsafe {
        for (name, dll) in [
            ("xinput1_4.dll", b"xinput1_4.dll\0".as_ptr()),
            ("xinput1_3.dll", b"xinput1_3.dll\0".as_ptr()),
        ] {
            let Ok(module) = LoadLibraryA(PCSTR(dll)) else {
                continue;
            };
            let proc = GetProcAddress(module, PCSTR(100usize as *const u8))
                .or_else(|| GetProcAddress(module, PCSTR(b"XInputGetState\0".as_ptr())));
            let Some(proc) = proc else {
                continue;
            };
            let get_state: XInputGetStateFn = std::mem::transmute(proc);
            let get_keystroke = GetProcAddress(module, PCSTR(b"XInputGetKeystroke\0".as_ptr()))
                .map(|p| std::mem::transmute::<_, XInputGetKeystrokeFn>(p));
            let label = format!("{name} ordinal 100/GetState");
            crate::debug_state::set_xinput_loader(label.clone());
            service_log(&format!(
                "XInput loader: {label}; keystroke={}",
                get_keystroke.is_some()
            ));
            return (Some(module), Some(get_state), get_keystroke);
        }
    }
    crate::debug_state::set_xinput_loader("failed");
    service_log("XInput loader: failed");
    (None, None, None)
}

#[allow(dead_code)]
fn secure_poll_main(
    tx: mpsc::Sender<SecureMsg>,
    stop: Arc<AtomicBool>,
    tid_tx: mpsc::SyncSender<u32>,
) {
    // 1. Attach thread to the Winlogon desktop *before* loading xinput so the
    //    DLL's process/desktop association is bound to Winlogon from the start.
    match crate::win::attach_input() {
        Ok(()) => {
            let desktop = crate::win::current_desktop_name().unwrap_or_else(|| "?".into());
            let _ = tx.send(SecureMsg::Ready(desktop));
        }
        Err(e) => {
            let _ = tx.send(SecureMsg::Error(format!("desktop attach failed: {e}")));
            let _ = tid_tx.send(unsafe { GetCurrentThreadId() });
            return;
        }
    }

    // 2. Register class + create the anchor window on the Winlogon desktop first.
    //    Joyxoff polls XInput on the window-owning thread under the Winlogon worker
    //    token — not via ImpersonateLoggedOnUser on the timer path (see NOTES.md).
    //    This is the key bypass: XInput delivers real packets to processes that
    //    own a window on the input desktop, and the poll runs on this thread.
    let hwnd = unsafe {
        let instance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(SecureMsg::Error(format!("GetModuleHandleW: {e}")));
                let _ = tid_tx.send(GetCurrentThreadId());
                POLL_STATE.with(|s| *s.borrow_mut() = None);
                return;
            }
        };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(anchor_wndproc),
            hInstance: instance.into(),
            lpszClassName: ANCHOR_CLASS,
            ..Default::default()
        };
        // RegisterClassW returns 0 if class already exists; ignore.
        RegisterClassW(&wc);
        match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            ANCHOR_CLASS,
            w!("Warmup XInput Anchor"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            HMENU::default(),
            HINSTANCE(instance.0),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(SecureMsg::Error(format!("CreateWindowExW anchor: {e}")));
                let _ = tid_tx.send(GetCurrentThreadId());
                POLL_STATE.with(|s| *s.borrow_mut() = None);
                return;
            }
        }
    };

    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            1,
            1,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = tx.send(SecureMsg::Error(
            "anchor window shown on Winlogon for XInput focus eligibility".into(),
        ));
    }

    // 3. Load XInput after the anchor HWND exists on Winlogon.
    let (_module, get_state, get_keystroke) = load_xinput_api();
    let Some(get_state) = get_state else {
        let _ = tx.send(SecureMsg::Error("loader failed".into()));
        let _ = tid_tx.send(unsafe { GetCurrentThreadId() });
        return;
    };

    POLL_STATE.with(|s| {
        *s.borrow_mut() = Some(PollState {
            get_state,
            get_keystroke,
            tx: tx.clone(),
            prev_buttons: [0; 4],
            active_slot: None,
            connected_prev: [false; 4],
            last_status: Instant::now() - Duration::from_secs(60),
            last_no_pad: Instant::now() - Duration::from_secs(60),
            last_probe_log: Instant::now() - Duration::from_secs(60),
            iter_count: 0,
            raw_preparsed: HashMap::new(),
            raw_diag_count: 0,
            last_raw_buttons: 0,
        });
    });

    unsafe {
        register_raw_gamepad(hwnd, &tx);
        if SetTimer(hwnd, POLL_TIMER_ID, POLL_TIMER_MS, None) == 0 {
            let _ = tx.send(SecureMsg::Error("SetTimer failed for anchor poll".into()));
        }
    }

    // 4. Publish our thread id so Drop can wake us via PostThreadMessageW.
    let _ = tid_tx.send(unsafe { GetCurrentThreadId() });

    // 5. Pump messages. WM_TIMER fires xinput poll inside anchor_wndproc.
    let mut msg = MSG::default();
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let ok = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !ok.as_bool() {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = KillTimer(hwnd, POLL_TIMER_ID);
        let _ = DestroyWindow(hwnd);
    }
    POLL_STATE.with(|s| *s.borrow_mut() = None);
}

#[allow(dead_code)]
unsafe extern "system" fn anchor_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TIMER && wparam.0 == POLL_TIMER_ID {
        POLL_STATE.with(|s| {
            if let Some(state) = s.borrow_mut().as_mut() {
                poll_xinput_tick(state);
            }
        });
        return LRESULT(0);
    }
    if msg == WM_INPUT {
        POLL_STATE.with(|s| {
            if let Some(state) = s.borrow_mut().as_mut() {
                poll_raw_hid_input(state, HRAWINPUT(lparam.0 as _));
            }
        });
        return LRESULT(0);
    }
    if msg == WM_DESTROY {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[allow(dead_code)]
fn register_raw_gamepad(hwnd: HWND, tx: &mpsc::Sender<SecureMsg>) {
    let devices = [
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x05,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x04,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
    ];
    match unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) } {
        Ok(()) => {
            let _ = tx.send(SecureMsg::Error(
                "raw HID fallback registered for gamepad/joystick".into(),
            ));
        }
        Err(e) => {
            let _ = tx.send(SecureMsg::Error(format!("raw HID register failed: {e}")));
        }
    }
}

#[allow(dead_code)]
fn poll_raw_hid_input(state: &mut PollState, raw_handle: HRAWINPUT) {
    let mut size = 0u32;
    unsafe {
        GetRawInputData(
            raw_handle,
            RID_INPUT,
            None,
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
    }
    if size == 0 {
        return;
    }

    let words = (size as usize + size_of::<usize>() - 1) / size_of::<usize>();
    let mut storage = vec![0usize; words];
    let read = unsafe {
        GetRawInputData(
            raw_handle,
            RID_INPUT,
            Some(storage.as_mut_ptr().cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if read == u32::MAX || read == 0 {
        return;
    }

    let raw = unsafe { &*(storage.as_ptr() as *const RAWINPUT) };
    if raw.header.dwType != RIM_TYPEHID.0 {
        return;
    }

    let Some(preparsed) = raw_preparsed_data(state, raw) else {
        return;
    };
    let hid = unsafe { raw.data.hid };
    let report_size = hid.dwSizeHid as usize;
    let report_count = hid.dwCount as usize;
    if report_size == 0 || report_count == 0 {
        return;
    }

    let raw_start = unsafe { slice::from_raw_parts(hid.bRawData.as_ptr(), report_size * report_count) };
    for report in raw_start.chunks(report_size) {
        let mut usages = [USAGE_AND_PAGE::default(); 64];
        let mut usage_len = usages.len() as u32;
        let status = unsafe {
            HidP_GetUsagesEx(
                HidP_Input,
                0,
                usages.as_mut_ptr(),
                &mut usage_len,
                preparsed,
                report,
            )
        };

        let mut cur = 0u16;
        let mut seen = Vec::new();
        if status == HIDP_STATUS_SUCCESS {
            for usage in usages.iter().take(usage_len as usize) {
                seen.push(format!("{:02x}:{:02x}", usage.UsagePage, usage.Usage));
                if let Some(mask) = hid_usage_to_mask(usage.UsagePage, usage.Usage) {
                    cur |= mask;
                }
            }
        }
        if cur == 0 {
            cur = xusb_buttons_from_report(report).unwrap_or(0);
        }
        state.last_raw_buttons = cur;

        let prev = state.prev_buttons[0];
        if prev != cur {
            state.prev_buttons[0] = cur;
            service_log(&format!(
                "XInput secure helper: raw HID buttons 0x{prev:04x} -> 0x{cur:04x} [{}]",
                if seen.is_empty() {
                    "xusb".to_string()
                } else {
                    seen.join(",")
                }
            ));
            let _ = state.tx.send(SecureMsg::Buttons { slot: 0, prev, cur });
        } else if cur != 0 && state.raw_diag_count < 4 {
            state.raw_diag_count = state.raw_diag_count.saturating_add(1);
            service_log(&format!(
                "XInput secure helper: raw HID held 0x{cur:04x} usages=[{}]",
                seen.join(",")
            ));
        }
    }
}

#[allow(dead_code)]
fn raw_preparsed_data(state: &mut PollState, raw: &RAWINPUT) -> Option<PHIDP_PREPARSED_DATA> {
    let key = raw.header.hDevice.0 as usize;
    if !state.raw_preparsed.contains_key(&key) {
        let mut bytes = 0u32;
        unsafe {
            GetRawInputDeviceInfoW(raw.header.hDevice, RIDI_PREPARSEDDATA, None, &mut bytes);
        }
        if bytes == 0 {
            return None;
        }
        let words = (bytes as usize + size_of::<usize>() - 1) / size_of::<usize>();
        let mut storage = vec![0usize; words];
        let got = unsafe {
            GetRawInputDeviceInfoW(
                raw.header.hDevice,
                RIDI_PREPARSEDDATA,
                Some(storage.as_mut_ptr().cast()),
                &mut bytes,
            )
        };
        if got == u32::MAX || got == 0 {
            let _ = state
                .tx
                .send(SecureMsg::Error("raw HID preparsed data unavailable".into()));
            return None;
        }
        state.raw_preparsed.insert(key, storage);
        let _ = state.tx.send(SecureMsg::Error(format!(
            "raw HID preparsed cached for device 0x{key:x} ({bytes} bytes)"
        )));
    }
    state
        .raw_preparsed
        .get_mut(&key)
        .map(|buf| PHIDP_PREPARSED_DATA(buf.as_mut_ptr() as isize))
}

#[allow(dead_code)]
fn poll_xinput_tick(state: &mut PollState) {
    let mut connected = [false; 4];
    let mut states: [Option<XINPUT_GAMEPAD>; 4] = [None; 4];
    let mut errs = [0u32; 4];
    let mut packets = [0u32; 4];
    for slot in 0..SLOTS {
        let mut s = XINPUT_STATE::default();
        let err = unsafe { (state.get_state)(slot, &mut s) };
        errs[slot as usize] = err;
        if err == ERROR_SUCCESS {
            connected[slot as usize] = true;
            states[slot as usize] = Some(s.Gamepad);
            packets[slot as usize] = s.dwPacketNumber;
        } else if err != ERROR_DEVICE_NOT_CONNECTED {
            let _ = state
                .tx
                .send(SecureMsg::Error(format!("XInputGetState({slot}) error {err}")));
        }
        if let Some(get_keystroke) = state.get_keystroke {
            secure_poll_keystrokes(&state.tx, get_keystroke, slot, &mut state.prev_buttons);
        }
    }

    state.iter_count += 1;
    let probe_due = state.last_probe_log.elapsed() >= Duration::from_secs(2);
    if state.iter_count == 1 || probe_due {
        state.last_probe_log = Instant::now();
        let summary: Vec<String> = (0..SLOTS as usize)
            .map(|i| format!("{}:{}", i, errs[i]))
            .collect();
        let mut raw = String::new();
        for i in 0..SLOTS as usize {
            if let Some(pad) = states[i] {
                raw.push_str(&format!(
                    " s{i}:pkt={} btn=0x{:04x} lt={} rt={} lx={} ly={} rx={} ry={}",
                    packets[i],
                    pad.wButtons.0,
                    pad.bLeftTrigger,
                    pad.bRightTrigger,
                    pad.sThumbLX,
                    pad.sThumbLY,
                    pad.sThumbRX,
                    pad.sThumbRY,
                ));
            }
        }
        let _ = state.tx.send(SecureMsg::Error(format!(
            "probe iter={} errs=[{}]{raw}",
            state.iter_count,
            summary.join(",")
        )));
    }

    let slots_changed = connected
        .iter()
        .zip(state.connected_prev.iter())
        .any(|(a, b)| a != b);
    if slots_changed || state.last_status.elapsed() >= Duration::from_secs(30) {
        state.last_status = Instant::now();
        state.connected_prev = connected;
        let _ = state.tx.send(SecureMsg::Slots(connected));
    }

    let slot = match state.active_slot {
        Some(slot) if connected[slot as usize] => Some(slot),
        _ => {
            state.active_slot = (0..SLOTS).find(|i| connected[*i as usize]);
            state.active_slot
        }
    };

    let Some(slot) = slot else {
        if state.last_no_pad.elapsed() >= Duration::from_secs(15) {
            state.last_no_pad = Instant::now();
            let _ = state.tx.send(SecureMsg::NoController);
        }
        return;
    };

    let pad = states[slot as usize].expect("connected slot has state");
    let idx = slot as usize;
    let prev = state.prev_buttons[idx];
    let mut cur = pad.wButtons.0;
    if slot == 0 {
        cur |= state.last_raw_buttons;
    }
    state.prev_buttons[idx] = cur;
    if prev != cur {
        let _ = state.tx.send(SecureMsg::Buttons { slot, prev, cur });
    }
    let _ = state.tx.send(SecureMsg::Axes((
        XInputBackend::norm_thumb(pad.sThumbLX, LEFT_DEADZONE),
        XInputBackend::norm_thumb(pad.sThumbLY, LEFT_DEADZONE),
        XInputBackend::norm_thumb(pad.sThumbRX, RIGHT_DEADZONE),
        XInputBackend::norm_thumb(pad.sThumbRY, RIGHT_DEADZONE),
    )));
}

#[allow(dead_code)]
fn secure_poll_keystrokes(
    tx: &mpsc::Sender<SecureMsg>,
    get_keystroke: XInputGetKeystrokeFn,
    slot: u32,
    prev_buttons: &mut [u16; 4],
) {
    for _ in 0..16 {
        let mut key = XInputKeystroke::default();
        let err = unsafe { get_keystroke(slot, 0, &mut key) };
        if err == ERROR_EMPTY || err == ERROR_DEVICE_NOT_CONNECTED {
            break;
        }
        if err != ERROR_SUCCESS {
            let _ = tx.send(SecureMsg::Error(format!(
                "XInputGetKeystroke({slot}) error {err}"
            )));
            break;
        }
        let _ = tx.send(SecureMsg::Error(format!(
            "keystroke slot {slot}: vk=0x{:04x} flags=0x{:04x} user={} hid=0x{:02x}",
            key.virtual_key, key.flags, key.user_index, key.hid_code
        )));
        let Some(mask) = key_to_mask(key.virtual_key) else {
            continue;
        };
        let idx = slot as usize;
        let prev = prev_buttons[idx];
        let mut cur = prev;
        if key.flags & XINPUT_KEYSTROKE_KEYDOWN != 0 {
            cur |= mask;
        }
        if key.flags & XINPUT_KEYSTROKE_KEYUP != 0 {
            cur &= !mask;
        }
        if prev != cur {
            prev_buttons[idx] = cur;
            let _ = tx.send(SecureMsg::Buttons { slot, prev, cur });
        }
    }
}

fn service_log(msg: &str) {
    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
        crate::install::log_line(msg);
    }
}
