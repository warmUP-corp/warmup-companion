//! Winlogon gamepad polling: vendor-agnostic HID (primary) + XInput (Xbox fast path).
//!
//! Joyxoff insight: XInputGetState returns neutral (zeroed) state to processes
//! that have no foreground-eligible window on the input desktop. Mitigation: the
//! secure poll thread runs a real Win32 UI message pump and owns a tiny anchor
//! window on the Winlogon desktop. PlayStation and Xbox pads are read via raw
//! HID + `hid_gamepad` (SDL `gamecontrollerdb.txt` for VID:PID hints); XInput
//! supplements when the driver exposes a real packet.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::{w, PCSTR};
use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK,
};
use windows::Win32::UI::Input::XboxController::{
    XINPUT_CAPABILITIES, XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B,
    XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT,
    XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_RIGHT_SHOULDER,
    XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, KillTimer, PostThreadMessageW,
    RegisterClassW, SetTimer, TranslateMessage, HMENU, MSG,
    WM_DESTROY, WM_INPUT, WM_NULL, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::gamepad_backend::Button;
use crate::gamepad_backend::ButtonChange;
use crate::gamepad_backend::GamepadBackend;
use crate::gamepad_backend::mapping_db_path;
use crate::hid_gamepad::{self, PadSample};
use crate::xusb_ioctl::{XusbDevice, XusbReport};

const SLOTS: u32 = 4;
const ERROR_SUCCESS: u32 = 0;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const ERROR_EMPTY: u32 = 4306;

const LEFT_DEADZONE: i16 = 7849;
const RIGHT_DEADZONE: i16 = 8689;

type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;
type XInputGetKeystrokeFn = unsafe extern "system" fn(u32, u32, *mut XInputKeystroke) -> u32;
type XInputGetCapabilitiesFn =
    unsafe extern "system" fn(u32, u32, *mut XINPUT_CAPABILITIES) -> u32;

/// One independently-loaded XInput DLL, used only by the winlogon diagnostic
/// probe (step C). We load `xinput1_4.dll` and `xinput1_3.dll` side-by-side and
/// poll ordinal 100 (`XInputGetStateEx`) from each on the same frame so the
/// service.log shows definitively which DLL — if either — returns real
/// `wButtons` on the secure desktop, and whether 1.4 background-zeroing is the
/// gate. Delete once the winning path is baked into the loader (step B).
struct ProbeDll {
    label: &'static str,
    get_state: XInputGetStateFn,
    get_caps: Option<XInputGetCapabilitiesFn>,
}

struct XInputProbe {
    dlls: Vec<ProbeDll>,
    /// OR of every DLL's slot-0 buttons last frame, for press/release edges.
    last_combined: u16,
    logged_caps: bool,
    last_heartbeat: Instant,
}

impl XInputProbe {
    fn load() -> Self {
        let mut dlls = Vec::new();
        for (label, raw) in [
            ("1_4", b"xinput1_4.dll\0".as_slice()),
            ("1_3", b"xinput1_3.dll\0".as_slice()),
        ] {
            unsafe {
                let Ok(module) = LoadLibraryA(PCSTR(raw.as_ptr())) else {
                    continue;
                };
                let Some(proc) = GetProcAddress(module, PCSTR(100usize as *const u8)) else {
                    continue;
                };
                let get_state: XInputGetStateFn = std::mem::transmute(proc);
                let get_caps = GetProcAddress(module, PCSTR(b"XInputGetCapabilities\0".as_ptr()))
                    .map(|p| std::mem::transmute::<_, XInputGetCapabilitiesFn>(p));
                dlls.push(ProbeDll {
                    label,
                    get_state,
                    get_caps,
                });
            }
        }
        Self {
            dlls,
            last_combined: 0,
            logged_caps: false,
            last_heartbeat: Instant::now() - Duration::from_secs(60),
        }
    }

    /// Poll every loaded DLL for `slot` and emit an `XPROBE` line whenever the
    /// combined button mask changes (press/release edge) or every 2s heartbeat.
    fn tick(&mut self, slot: u32, tx: &mpsc::Sender<SecureMsg>) {
        let mut combined = 0u16;
        let mut per_dll = Vec::with_capacity(self.dlls.len());
        for dll in &self.dlls {
            let mut s = XINPUT_STATE::default();
            let err = unsafe { (dll.get_state)(slot, &mut s) };
            let (btn, pkt) = if err == ERROR_SUCCESS {
                combined |= s.Gamepad.wButtons.0;
                (s.Gamepad.wButtons.0, s.dwPacketNumber)
            } else {
                (0, 0)
            };
            per_dll.push(format!(
                "{}:err={} btn=0x{:04x} pkt={} lt={} rt={} lx={} ly={}",
                dll.label, err, btn, pkt, s.Gamepad.bLeftTrigger, s.Gamepad.bRightTrigger,
                s.Gamepad.sThumbLX, s.Gamepad.sThumbLY,
            ));
        }

        let edge = combined != self.last_combined;
        let heartbeat = self.last_heartbeat.elapsed() >= Duration::from_secs(2);
        if !edge && !heartbeat {
            return;
        }
        self.last_combined = combined;
        if heartbeat {
            self.last_heartbeat = Instant::now();
        }

        let mut line = format!("XPROBE slot{slot} [{}] {}", per_dll.join(" | "), probe_foreground());

        // Capabilities once (subtype/flags identify the real device behind slot 0).
        if !self.logged_caps {
            if let Some(dll) = self.dlls.first() {
                if let Some(get_caps) = dll.get_caps {
                    let mut caps = XINPUT_CAPABILITIES::default();
                    let err = unsafe { get_caps(slot, 0, &mut caps) };
                    if err == ERROR_SUCCESS {
                        self.logged_caps = true;
                        line.push_str(&format!(
                            " caps[{}]:type={} subtype={} flags=0x{:04x}",
                            dll.label, caps.Type.0, caps.SubType.0, caps.Flags.0
                        ));
                    }
                }
            }
        }
        let _ = tx.send(SecureMsg::Error(line));
    }
}

/// One-shot identity of THIS worker process: session, token user SID, integrity
/// level RID, thread desktop. Joyxoff's worker reads the pad on the secure
/// desktop and ours does not despite byte-identical launch code — so the gate
/// must be a process attribute. This logs ours; compare against Joyxoff.exe in
/// Process Explorer (Session / User / Integrity). Integrity RIDs: System=0x4000,
/// High=0x3000, Medium=0x2000.
fn probe_self_identity() -> String {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TokenUser, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let pid = GetCurrentProcessId();
        let mut proc_sess = 0u32;
        let _ = ProcessIdToSessionId(pid, &mut proc_sess);

        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return format!("SELF pid={pid} proc_sess={proc_sess} token=open_err");
        }
        let read = |class| -> Vec<u8> {
            let mut len = 0u32;
            let _ = GetTokenInformation(tok, class, None, 0, &mut len);
            if len == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u8; len as usize];
            if GetTokenInformation(tok, class, Some(buf.as_mut_ptr().cast()), len, &mut len).is_err()
            {
                return Vec::new();
            }
            buf
        };

        let user_buf = read(TokenUser);
        let user = if user_buf.len() >= size_of::<TOKEN_USER>() {
            let tu = &*(user_buf.as_ptr() as *const TOKEN_USER);
            let mut s = PWSTR::null();
            if ConvertSidToStringSidW(tu.User.Sid, &mut s).is_ok() {
                let out = s.to_string().unwrap_or_default();
                let _ = LocalFree(HLOCAL(s.0 as _));
                out
            } else {
                "?".into()
            }
        } else {
            "?".into()
        };

        let integ_buf = read(TokenIntegrityLevel);
        let integ = if integ_buf.len() >= size_of::<TOKEN_MANDATORY_LABEL>() {
            let lab = &*(integ_buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let cnt = *GetSidSubAuthorityCount(lab.Label.Sid);
            let rid = *GetSidSubAuthority(lab.Label.Sid, (cnt - 1) as u32);
            format!("0x{rid:x}")
        } else {
            "?".into()
        };

        let _ = CloseHandle(tok);
        let desk = crate::win::current_desktop_name().unwrap_or_else(|| "?".into());
        format!("SELF pid={pid} proc_sess={proc_sess} user={user} integrity={integ} desktop={desk}")
    }
}

/// Foreground window identity on the current (winlogon) desktop. Tells us
/// whether we own foreground (XInput 1.4 background-zeroing gate) or LogonUI
/// does. `ours=true` means GetForegroundWindow belongs to this process.
fn probe_foreground() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return "fg=none".into();
        }
        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let ours = pid == GetCurrentProcessId();
        let mut buf = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut buf);
        let class = if n > 0 {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            "?".into()
        };
        format!("fg=0x{:x} pid={pid} ours={ours} class={class}", hwnd.0 as usize)
    }
}

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

/// The one [`Button`] ↔ XInput mask table. HID and XInput both map through this;
/// nothing else hard-codes the bit for a button.
const BUTTON_MASKS: &[(Button, u16)] = &[
    (Button::Up, XINPUT_GAMEPAD_DPAD_UP.0),
    (Button::Down, XINPUT_GAMEPAD_DPAD_DOWN.0),
    (Button::Left, XINPUT_GAMEPAD_DPAD_LEFT.0),
    (Button::Right, XINPUT_GAMEPAD_DPAD_RIGHT.0),
    (Button::A, XINPUT_GAMEPAD_A.0),
    (Button::B, XINPUT_GAMEPAD_B.0),
    (Button::X, XINPUT_GAMEPAD_X.0),
    (Button::Y, XINPUT_GAMEPAD_Y.0),
    (Button::Lb, XINPUT_GAMEPAD_LEFT_SHOULDER.0),
    (Button::Rb, XINPUT_GAMEPAD_RIGHT_SHOULDER.0),
];

/// XInput mask bit for a button, if it has one.
pub(crate) fn button_mask(button: Button) -> Option<u16> {
    BUTTON_MASKS
        .iter()
        .find_map(|&(b, mask)| (b == button).then_some(mask))
}

fn secure_hid_combo_mask(mask: u16) -> bool {
    const NON_DPAD: u16 = XINPUT_GAMEPAD_A.0
        | XINPUT_GAMEPAD_B.0
        | XINPUT_GAMEPAD_X.0
        | XINPUT_GAMEPAD_Y.0
        | XINPUT_GAMEPAD_LEFT_SHOULDER.0
        | XINPUT_GAMEPAD_RIGHT_SHOULDER.0;
    (mask & NON_DPAD).count_ones() > 1
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
    hid_devices: HashMap<usize, hid_gamepad::DeviceState>,
    last_hid: PadSample,
    hid_diag_count: u32,
    suppress_until_zero: bool,
    /// Diagnostics: count of XInputGetKeystroke events seen since spawn, and the
    /// most recent raw HID report. Surfaced in the 2s probe so the next deploy
    /// shows definitively whether Y arrives via keystroke or which HID byte moves.
    keystroke_events: u64,
    last_raw_report: Vec<u8>,
    /// Physical XUSB pads opened via direct DeviceIoControl. These bypass the
    /// XInput foreground focus gate that zeroes `get_state` on Winlogon.
    xusb: Vec<XusbDevice>,
    /// Most recent XUSB report (for the probe dump + offset verification).
    last_xusb: Option<XusbReport>,
    /// Step-C diagnostic: side-by-side xinput1_3 vs xinput1_4 GetStateEx.
    probe: XInputProbe,
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
        for &(button, mask) in BUTTON_MASKS {
            let was = prev & mask != 0;
            let now = cur & mask != 0;
            if was != now {
                out.push(ButtonChange {
                    button,
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
        let names: Vec<&str> = BUTTON_MASKS
            .iter()
            .filter_map(|(b, mask)| (cur & *mask != 0).then(|| b.as_str()))
            .collect();
        service_log(&format!(
            "XInput buttons slot {slot}: 0x{prev:04x} -> 0x{cur:04x} [{}]",
            names.join("+")
        ));
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

    fn live_input_summary(&self) -> String {
        let Some(slot) = self.active_slot else {
            return String::new();
        };
        let mask = self.prev_buttons[slot as usize];
        let pressed: Vec<&str> = BUTTON_MASKS
            .iter()
            .filter_map(|(b, m)| (mask & *m != 0).then_some(b.as_str()))
            .collect();
        warmup_gamepad::live_input_format(&pressed, self.axes)
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
            service_log(&format!(
                "XInput loader: {label}; keystroke={}",
                get_keystroke.is_some()
            ));
            return (Some(module), Some(get_state), get_keystroke);
        }
    }
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
        // Joyxoff parity: ex_style 0x8080088 (TOPMOST|TOOLWINDOW|NOACTIVATE|LAYERED),
        // style WS_POPUP only — NOT WS_VISIBLE. Joyxoff's JoyXoffMWindow is created
        // invisible (style 0x80000000). A *visible* TOPMOST window on the Winlogon
        // desktop becomes a focus/nav target for the shell's gamepad navigation, so
        // D-pad presses move LogonUI focus off the password box. XInput's foreground
        // gate is irrelevant here (LogonUI is always foreground; we read pads via the
        // direct XUSB path), so visibility buys nothing and only causes the defocus.
        match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            ANCHOR_CLASS,
            w!("Warmup XInput Anchor"),
            WS_POPUP,
            0,
            0,
            32,
            32,
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

    // Anchor stays invisible (no WS_VISIBLE, no alpha) so it is not a gamepad-nav
    // focus target on the Winlogon desktop. Pads are read via the XUSB path below.
    let _ = tx.send(SecureMsg::Error(
        "anchor window created invisible on Winlogon (joyxoff parity, no nav focus)".into(),
    ));
    let _ = tx.send(SecureMsg::Error(probe_self_identity()));

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
            hid_devices: HashMap::new(),
            last_hid: PadSample::default(),
            hid_diag_count: 0,
            suppress_until_zero: false,
            keystroke_events: 0,
            last_raw_report: Vec::new(),
            xusb: Vec::new(),
            last_xusb: None,
            probe: XInputProbe::load(),
        });
    });

    // Open physical XUSB pads directly — the focus-gate bypass for Winlogon.
    let (xusb_devices, xusb_log) = XusbDevice::open_all();
    for line in xusb_log {
        let _ = tx.send(SecureMsg::Error(line));
    }
    POLL_STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            state.xusb = xusb_devices;
        }
    });

    let gcdb = mapping_db_path();
    let gcdb_n = hid_gamepad::init_from_gcdb(&gcdb);
    if gcdb_n > 0 {
        let _ = tx.send(SecureMsg::Error(format!(
            "HID: loaded {gcdb_n} gamecontrollerdb VID:PID hints from {}",
            gcdb.display()
        )));
    }

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
                "HID: raw input sink registered (gamepad + joystick)".into(),
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
    let capacity = words * size_of::<usize>();
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
    // `read` is the byte count GetRawInputData wrote; never trust it past the
    // allocation. process_raw_input clamps the HID payload to these bytes.
    let raw_bytes = (read as usize).min(capacity);
    if raw_bytes < size_of::<RAWINPUTHEADER>() {
        return;
    }

    let raw = unsafe { &*(storage.as_ptr() as *const RAWINPUT) };
    let Some((_key, sample, src, dev, report)) =
        hid_gamepad::process_raw_input(&mut state.hid_devices, raw, raw_bytes)
    else {
        return;
    };
    let raw_hex = report_hex(&report);
    state.last_raw_report = report.clone();
    if src == "open" {
        service_log(&format!("HID secure: {dev} raw={raw_hex}"));
        state.prev_buttons[0] = 0;
        state.hid_diag_count = 0;
        state.suppress_until_zero = false;
    }
    state.last_hid = sample;
    let cur = sample.buttons;
    let prev = state.prev_buttons[0];
    if prev != cur {
        if state.suppress_until_zero {
            if cur == 0 {
                state.suppress_until_zero = false;
                state.prev_buttons[0] = 0;
            }
            service_log(&format!(
                "HID secure: suppress combo tail 0x{prev:04x} -> 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
            ));
            return;
        }
        if secure_hid_combo_mask(prev) || secure_hid_combo_mask(cur) {
            state.suppress_until_zero = cur != 0;
            service_log(&format!(
                "HID secure: suppress noisy combo 0x{prev:04x} -> 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
            ));
            return;
        }
        state.prev_buttons[0] = cur;
        service_log(&format!(
            "HID secure: buttons 0x{prev:04x} -> 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
        ));
        let _ = state.tx.send(SecureMsg::Buttons { slot: 0, prev, cur });
    } else if cur != 0 && state.hid_diag_count < 4 {
        state.hid_diag_count = state.hid_diag_count.saturating_add(1);
        service_log(&format!("HID secure: held 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"));
    }
}

fn report_hex(report: &[u8]) -> String {
    let mut out = String::new();
    for (i, b) in report.iter().take(16).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02x}"));
    }
    if report.len() > 16 {
        out.push_str(" ...");
    }
    out
}

#[allow(dead_code)]
fn poll_xinput_tick(state: &mut PollState) {
    // Step-C diagnostic: compare 1_3 vs 1_4 GetStateEx on the live winlogon
    // thread. Disjoint field borrows (probe is &mut, tx is &) are allowed.
    state.probe.tick(0, &state.tx);

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
            state.keystroke_events +=
                secure_poll_keystrokes(&state.tx, get_keystroke, slot, &mut state.prev_buttons);
        }
    }

    // Direct XUSB read — authoritative on Winlogon, where DLL get_state is
    // gated to a neutral state by the foreground focus check. Pick the first
    // device that responds; mark its slot connected even if the DLL did not.
    let mut xusb_rep = None;
    let mut xusb_idx = None;
    for (i, dev) in state.xusb.iter().enumerate() {
        if let Some(rep) = dev.poll() {
            xusb_rep = Some(rep);
            xusb_idx = Some(i);
            break;
        }
    }
    if let Some(i) = xusb_idx {
        if i < SLOTS as usize {
            connected[i] = true;
        }
    }
    state.last_xusb = xusb_rep;

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
        let xusb_str = match &state.last_xusb {
            Some(rep) => format!(
                " xusb:btn=0x{:04x} lt={} rt={} lx={} ly={} rx={} ry={} raw=[{}]",
                rep.buttons,
                rep.left_trigger,
                rep.right_trigger,
                rep.thumb_lx,
                rep.thumb_ly,
                rep.thumb_rx,
                rep.thumb_ry,
                report_hex(&rep.raw),
            ),
            None => " xusb:none".into(),
        };
        let _ = state.tx.send(SecureMsg::Error(format!(
            "probe iter={} errs=[{}]{raw} keystroke_events={} hid_raw=[{}]{xusb_str}",
            state.iter_count,
            summary.join(","),
            state.keystroke_events,
            report_hex(&state.last_raw_report),
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
    let xinput_stick = pad.sThumbLX.abs() > LEFT_DEADZONE || pad.sThumbLY.abs() > LEFT_DEADZONE;
    // Button edges come from WM_INPUT (HID) and XInputGetKeystroke only. Merging
    // last_hid into GetState when wButtons==0 stuck false masks (HidP 0xb200).
    let cur = pad.wButtons.0;
    if cur != 0 {
        let idx = slot as usize;
        let prev = state.prev_buttons[idx];
        if prev != cur {
            state.prev_buttons[idx] = cur;
            let _ = state.tx.send(SecureMsg::Buttons { slot, prev, cur });
        }
    }
    let axes = if xinput_stick {
        (
            XInputBackend::norm_thumb(pad.sThumbLX, LEFT_DEADZONE),
            XInputBackend::norm_thumb(pad.sThumbLY, LEFT_DEADZONE),
            XInputBackend::norm_thumb(pad.sThumbRX, RIGHT_DEADZONE),
            XInputBackend::norm_thumb(pad.sThumbRY, RIGHT_DEADZONE),
        )
    } else {
        (
            state.last_hid.lx,
            state.last_hid.ly,
            state.last_hid.rx,
            state.last_hid.ry,
        )
    };
    let _ = state.tx.send(SecureMsg::Axes(axes));
}

#[allow(dead_code)]
fn secure_poll_keystrokes(
    tx: &mpsc::Sender<SecureMsg>,
    get_keystroke: XInputGetKeystrokeFn,
    slot: u32,
    prev_buttons: &mut [u16; 4],
) -> u64 {
    let mut events = 0u64;
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
        events += 1;
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
            service_log(&format!(
                "HID secure: keystroke slot {slot} vk=0x{:04x} -> 0x{cur:04x}",
                key.virtual_key
            ));
            let _ = tx.send(SecureMsg::Buttons { slot, prev, cur });
        }
    }
    events
}

fn service_log(msg: &str) {
    if crate::config::service_mode() {
        crate::install::log_line(msg);
    }
}
