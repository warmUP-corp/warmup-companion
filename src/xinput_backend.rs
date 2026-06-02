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
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::{w, PCSTR};
use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::Input::XboxController::{
    XINPUT_CAPABILITIES, XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK,
    XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT,
    XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_LEFT_THUMB,
    XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_RIGHT_THUMB, XINPUT_GAMEPAD_START,
    XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE,
};
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices,
    HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_INPUTSINK,
    RIDEV_PAGEONLY, RIDI_DEVICEINFO, RID_INPUT, RID_DEVICE_INFO, RIM_TYPEHID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, KillTimer, PostThreadMessageW,
    RegisterClassW, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer, TranslateMessage,
    HMENU, LWA_ALPHA, MSG, WM_DESTROY, WM_INPUT, WM_NULL, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::gamepad_backend::mapping_db_path;
use crate::gamepad_backend::Button;
use crate::gamepad_backend::ButtonChange;
use crate::gamepad_backend::GamepadBackend;
use crate::hid_gamepad::{self, PadSample};
use crate::xusb_ioctl::{XusbDevice, XusbReport};

const SLOTS: u32 = 4;
const ERROR_SUCCESS: u32 = 0;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const ERROR_EMPTY: u32 = 4306;

const LEFT_DEADZONE: i16 = 7849;
const RIGHT_DEADZONE: i16 = 8689;
const GUIDE_BUTTON_MASK: u16 = 0x0400;

type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;
type XInputGetKeystrokeFn = unsafe extern "system" fn(u32, u32, *mut XInputKeystroke) -> u32;
type XInputGetCapabilitiesFn = unsafe extern "system" fn(u32, u32, *mut XINPUT_CAPABILITIES) -> u32;

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
            last_heartbeat: crate::time_util::stale(Duration::from_secs(60)),
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
                dll.label,
                err,
                btn,
                pkt,
                s.Gamepad.bLeftTrigger,
                s.Gamepad.bRightTrigger,
                s.Gamepad.sThumbLX,
                s.Gamepad.sThumbLY,
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

        let mut line = format!(
            "XPROBE slot{slot} [{}] {}",
            per_dll.join(" | "),
            probe_foreground()
        );

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
/// level RID, thread desktop, PLUS the fields that actually distinguish
/// winlogon's duplicated token from our own service's LocalSystem token (both
/// are S-1-5-18, so the SID alone is useless): the token's logon-session LUID
/// (`authid`), its origin logon session (`origin`), token type / impersonation
/// level, and the parent process. If `authid`/`origin` don't match winlogon's
/// logon session, our `CreateProcessAsUserW` launch is not in winlogon's context
/// — the leading hypothesis for why our plain XInput read zeros while Joyxoff's
/// (identical recipe) reads live. Integrity RIDs: System=0x4000, High=0x3000.
fn probe_self_identity() -> String {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TokenOrigin, TokenStatistics, TokenUser, TOKEN_MANDATORY_LABEL, TOKEN_ORIGIN, TOKEN_QUERY,
        TOKEN_STATISTICS, TOKEN_USER,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Find `pid`'s parent PID and the parent's image name via a process snapshot.
    /// Returns `(0, "?")` if not found. Runs once at helper spawn, not per poll.
    unsafe fn parent_process_of(pid: u32) -> (u32, String) {
        let find = |want: u32| -> Option<PROCESSENTRY32W> {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
            let mut e = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut found = None;
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    if e.th32ProcessID == want {
                        found = Some(e);
                        break;
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
            found
        };
        let Some(me) = find(pid) else {
            return (0, "?".into());
        };
        let ppid = me.th32ParentProcessID;
        let name = find(ppid)
            .map(|e| {
                let len = e
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(e.szExeFile.len());
                String::from_utf16_lossy(&e.szExeFile[..len])
            })
            .unwrap_or_else(|| "?".into());
        (ppid, name)
    }

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
            if GetTokenInformation(tok, class, Some(buf.as_mut_ptr().cast()), len, &mut len)
                .is_err()
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

        // Logon-session LUID + token type/impersonation level. AuthenticationId is
        // the LUID of the logon session the token belongs to; winlogon's token
        // carries the SYSTEM logon session (0x3e7). If ours differs, the dup didn't
        // give us winlogon's context. TokenType: 1=Primary, 2=Impersonation.
        let stats_buf = read(TokenStatistics);
        let (auth_id, tok_type, imp_level) = if stats_buf.len() >= size_of::<TOKEN_STATISTICS>() {
            let st = &*(stats_buf.as_ptr() as *const TOKEN_STATISTICS);
            let luid =
                ((st.AuthenticationId.HighPart as u64) << 32) | st.AuthenticationId.LowPart as u64;
            (luid, st.TokenType.0, st.ImpersonationLevel.0)
        } else {
            (0u64, 0i32, 0i32)
        };

        // TokenOrigin.OriginatingLogonSession — the logon session that created the
        // token (CreateProcessAsUserW preserves the source token's origin).
        let origin_buf = read(TokenOrigin);
        let origin = if origin_buf.len() >= size_of::<TOKEN_ORIGIN>() {
            let o = &*(origin_buf.as_ptr() as *const TOKEN_ORIGIN);
            ((o.OriginatingLogonSession.HighPart as u64) << 32)
                | o.OriginatingLogonSession.LowPart as u64
        } else {
            0u64
        };

        let _ = CloseHandle(tok);

        // Parent process PID + image. Our worker's parent is our service; if the
        // driver gate keys on parent==winlogon (reparenting), this confirms whether
        // we'd need PROC_THREAD_ATTRIBUTE_PARENT_PROCESS.
        let (parent_pid, parent_name) = parent_process_of(pid);

        let desk = crate::win::current_desktop_name().unwrap_or_else(|| "?".into());
        format!(
            "SELF pid={pid} parent={parent_pid}({parent_name}) proc_sess={proc_sess} user={user} \
             integrity={integ} authid=0x{auth_id:x} origin=0x{origin:x} toktype={tok_type} \
             implevel={imp_level} desktop={desk}"
        )
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
        format!(
            "fg=0x{:x} pid={pid} ours={ours} class={class}",
            hwnd.0 as usize
        )
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
/// Analog trigger pressed threshold (0–255), matching SDL `TRIGGER_THRESHOLD` feel.
const TRIGGER_PRESS_THRESH: u8 = 30;

fn poll_trigger_edges(
    prev_lt: &mut bool,
    prev_rt: &mut bool,
    left: u8,
    right: u8,
    out: &mut Vec<ButtonChange>,
) {
    let lt = left > TRIGGER_PRESS_THRESH;
    let rt = right > TRIGGER_PRESS_THRESH;
    if lt != *prev_lt {
        *prev_lt = lt;
        out.push(ButtonChange {
            button: Button::Lt,
            pressed: lt,
        });
    }
    if rt != *prev_rt {
        *prev_rt = rt;
        out.push(ButtonChange {
            button: Button::Rt,
            pressed: rt,
        });
    }
}

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
    (Button::Select, XINPUT_GAMEPAD_BACK.0),
    (Button::Start, XINPUT_GAMEPAD_START.0),
    (Button::L3, XINPUT_GAMEPAD_LEFT_THUMB.0),
    (Button::R3, XINPUT_GAMEPAD_RIGHT_THUMB.0),
    (Button::Guide, GUIDE_BUTTON_MASK),
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
    active_secure_hid: bool,
    prev_buttons: [u16; 4],
    slot_connected: [bool; 4],
    pending: Vec<ButtonChange>,
    prev_trigger_left: bool,
    prev_trigger_right: bool,
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
/// Foreground-juggle (Winlogon). The XUSB driver gates GET_GAMEPAD_STATE on the
/// caller owning foreground, exactly like the XInput DLL — reading the IOCTL
/// directly does NOT bypass the focus gate on this stack (confirmed via crash
/// dump A/B: identical IOCTL, real button bytes only while our anchor held
/// foreground; empty payload once LogonUI took it). So the anchor MUST own
/// foreground to read the pad — but SendInput into the PIN field needs LogonUI
/// foreground. We time-multiplex: hold foreground to read/navigate, and let each
/// inject burst borrow it back briefly.
///
/// The credential window (LogonUI / UAC) the poll tick last saw in foreground —
/// what we steal from. The inject path restores it so SendInput lands there.
static LOGON_FG_HWND: AtomicIsize = AtomicIsize::new(0);
/// While > 0 the poll tick skips reclaiming foreground for the anchor, so an
/// inject burst (focus + SendInput on the loop thread) keeps LogonUI foreground
/// long enough for its keys to land. Decremented once per ~8ms poll tick.
static INJECT_HOLD_TICKS: AtomicU32 = AtomicU32::new(0);
static INJECT_HOLD_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Poll ticks to suppress the reclaim per committed key (~8ms each ⇒ ~48ms).
const INJECT_HOLD_WINDOW: u32 = 6;

/// The credential window (LogonUI / UAC) the secure poll last saw in foreground,
/// or None if not seen yet. The inject path foregrounds this so SendInput reaches
/// the PIN field.
pub fn logon_credential_window() -> Option<HWND> {
    let h = LOGON_FG_HWND.load(Ordering::Relaxed);
    (h != 0).then(|| HWND(h as *mut _))
}

/// Suppress the anchor's foreground reclaim for one inject burst so the loop
/// thread can foreground LogonUI and SendInput uninterrupted. Called from the
/// inject path; the poll tick reclaims foreground once the window elapses.
pub fn begin_inject_hold() {
    INJECT_HOLD_ACTIVE.store(true, Ordering::Relaxed);
    INJECT_HOLD_TICKS.store(INJECT_HOLD_WINDOW, Ordering::Relaxed);
}

/// Reliably take foreground for `hwnd` even against a window that keeps grabbing
/// it back (LogonUI). A bare `SetForegroundWindow` is throttled by Windows'
/// foreground-lock and loses the tug-of-war; attaching our input queue to the
/// current foreground thread lifts that restriction for the duration of the call.
/// Self-limiting: once we win, the next tick sees `cur == hwnd` and skips this.
///
/// Retained (dead) after the Joyxoff-style no-foreground switch: the inject path
/// may still want a one-shot foreground hand-off to LogonUI for keystroke sends.
#[allow(dead_code)]
unsafe fn force_foreground(hwnd: HWND, current_fg: HWND) {
    let our_tid = GetCurrentThreadId();
    let fg_tid = GetWindowThreadProcessId(current_fg, None);
    if fg_tid != 0 && fg_tid != our_tid {
        let attached = AttachThreadInput(fg_tid, our_tid, true).as_bool();
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(fg_tid, our_tid, false);
        }
    } else {
        let _ = SetForegroundWindow(hwnd);
    }
}

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
    /// Direct HID input-report readers (PlayStation / generic pads). Windowed
    /// Raw Input (`WM_INPUT`) is never delivered on the secure desktop, so these
    /// CreateFile/ReadFile handles are the only working source for vendor pads —
    /// the HID analogue of the `xusb` DeviceIoControl bypass for Xbox.
    hid_readers: Vec<crate::hid_reader::HidReader>,
    /// Throttle for re-enumerating HID readers (pad plugged in after spawn).
    last_hid_scan: Instant,
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
    hid_active_prev: bool,
    /// Throttle for re-enumerating XUSB pads: `open_all` runs once at startup, but a
    /// pad plugged in later (or present only after the secure desktop appears) must
    /// be picked up, else we have 0 devices and fall back to the foreground-gated DLL.
    last_xusb_scan: Instant,
    prev_trigger_left: bool,
    prev_trigger_right: bool,
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
    Trigger(ButtonChange),
    HidActive(bool),
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
            active_secure_hid: false,
            prev_buttons: [0; 4],
            slot_connected: [false; 4],
            pending: Vec::new(),
            prev_trigger_left: false,
            prev_trigger_right: false,
            axes: (0.0, 0.0, 0.0, 0.0),
            last_status_log: crate::time_util::stale(Duration::from_secs(60)),
            last_no_pad_log: crate::time_util::stale(Duration::from_secs(60)),
            last_raw_log: crate::time_util::stale(Duration::from_secs(60)),
            last_secure_check: crate::time_util::stale(Duration::from_secs(60)),
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
                    self.clear_secure_state();
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
                SecureMsg::Slots(connected) => {
                    self.log_slots_if_changed(connected);
                    // Reflect the helper's connection state in active_slot so
                    // controller_label()/is_connected() and the debug overlay see the
                    // pad the helper is reading on Winlogon.
                    let _ = self.pick_active_slot(&connected);
                    if self.active_slot != Some(0) {
                        self.active_secure_hid = false;
                    }
                }
                SecureMsg::Buttons { slot, prev, cur } => {
                    self.log_button_change(slot, prev, cur);
                    // The helper owns button state on Winlogon; record it so the live
                    // input summary (and controller_label) reflect the active pad.
                    if (slot as usize) < self.prev_buttons.len() {
                        self.active_slot = Some(slot);
                        self.prev_buttons[slot as usize] = cur;
                    }
                    self.pending.extend(Self::edges(prev, cur));
                }
                SecureMsg::Trigger(edge) => {
                    match edge.button {
                        Button::Lt => self.prev_trigger_left = edge.pressed,
                        Button::Rt => self.prev_trigger_right = edge.pressed,
                        _ => {}
                    }
                    self.pending.push(edge);
                }
                SecureMsg::HidActive(active) => {
                    self.active_secure_hid = active;
                    if active {
                        self.active_slot = Some(0);
                    }
                }
                SecureMsg::Axes(axes) => self.axes = axes,
                SecureMsg::NoController => {
                    self.active_slot = None;
                    self.clear_secure_state();
                    self.axes = (0.0, 0.0, 0.0, 0.0);
                    self.log_no_controller();
                }
                SecureMsg::Error(e) => service_log(&format!("XInput secure helper: {e}")),
            }
        }
        got_msg || self.secure.is_some()
    }

    fn clear_secure_state(&mut self) {
        self.active_secure_hid = false;
        self.prev_trigger_left = false;
        self.prev_trigger_right = false;
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
        poll_trigger_edges(
            &mut self.prev_trigger_left,
            &mut self.prev_trigger_right,
            pad.bLeftTrigger,
            pad.bRightTrigger,
            &mut self.pending,
        );
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
        if self.active_secure_hid {
            return "HID slot 0".to_string();
        }
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
        let mut pressed: Vec<&str> = BUTTON_MASKS
            .iter()
            .filter_map(|(b, m)| (mask & *m != 0).then_some(b.as_str()))
            .collect();
        if self.prev_trigger_left {
            pressed.push("LT");
        }
        if self.prev_trigger_right {
            pressed.push("RT");
        }
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
        // Joyxoff-style anchor (exstyle 0x08080088 = NOACTIVATE|TOOLWINDOW|LAYERED
        // |TOPMOST, WS_POPUP): a non-activating tool window that NEVER takes
        // foreground. Stealing foreground was confirmed harmful — it broke manual
        // PIN entry (yanked focus from LogonUI every tick) and no longer earned a
        // live pad read on this Windows build (gate denies even with fg=ours; see
        // service.log zero-path). Off-screen (-10000), 1x1, alpha 0 keeps it
        // invisible. The pad-read grant is being pursued via the focus-owner
        // mechanism instead (xusb22 FUN_140016af0), not foreground.
        match CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            ANCHOR_CLASS,
            w!("Warmup XInput Anchor"),
            WS_POPUP,
            -10000,
            -10000,
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

    // Fully transparent (alpha 0) so the off-screen 1x1 anchor is never visible.
    // The anchor never takes foreground (Joyxoff-style); the pad-read grant is
    // pursued via the focus-owner mechanism, not foreground ownership.
    unsafe {
        let _ =
            SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
    }
    let _ = tx.send(SecureMsg::Error(
        "anchor window created (NOACTIVATE; no foreground steal, Joyxoff-style)".into(),
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
            last_status: crate::time_util::stale(Duration::from_secs(60)),
            last_no_pad: crate::time_util::stale(Duration::from_secs(60)),
            last_probe_log: crate::time_util::stale(Duration::from_secs(60)),
            iter_count: 0,
            hid_devices: HashMap::new(),
            hid_readers: Vec::new(),
            last_hid_scan: crate::time_util::stale(Duration::from_secs(60)),
            last_hid: PadSample::default(),
            hid_diag_count: 0,
            suppress_until_zero: false,
            keystroke_events: 0,
            last_raw_report: Vec::new(),
            xusb: Vec::new(),
            last_xusb: None,
            hid_active_prev: false,
            last_xusb_scan: crate::time_util::stale(Duration::from_secs(60)),
            prev_trigger_left: false,
            prev_trigger_right: false,
            probe: XInputProbe::load(),
        });
    });

    // Open physical XUSB pads directly — the focus-gate bypass for Winlogon.
    let (xusb_devices, xusb_log) = XusbDevice::open_all();
    for line in xusb_log {
        let _ = tx.send(SecureMsg::Error(line));
    }
    // Open vendor HID pads (PlayStation / generic) for direct reads — windowed
    // Raw Input is dead on the secure desktop, so this is their only source.
    let (hid_readers, hid_log) = crate::hid_reader::HidReader::open_all();
    for line in hid_log {
        let _ = tx.send(SecureMsg::Error(line));
    }
    POLL_STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            state.xusb = xusb_devices;
            state.hid_readers = hid_readers;
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
        // Joyxoff-style: never touch foreground. We only record the credential
        // window LogonUI owns (for the inject path's SetFocus), then poll. Stealing
        // foreground broke manual entry and did not earn a live read on this build.
        unsafe {
            let cur = GetForegroundWindow();
            if cur != hwnd && !cur.0.is_null() {
                LOGON_FG_HWND.store(cur.0 as isize, Ordering::Relaxed);
            }
        }
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
            usUsage: 0x00,
            dwFlags: RIDEV_INPUTSINK | RIDEV_PAGEONLY,
            hwndTarget: hwnd,
        },
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
        RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x08,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        },
    ];
    match unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) } {
        Ok(()) => {
            let _ = tx.send(SecureMsg::Error(
                "HID: raw input sink registered (generic-desktop page + gamepad + joystick + multi-axis)".into(),
            ));
            log_raw_input_devices(tx);
        }
        Err(e) => {
            let _ = tx.send(SecureMsg::Error(format!("raw HID register failed: {e}")));
        }
    }
}

#[allow(dead_code)]
fn log_raw_input_devices(tx: &mpsc::Sender<SecureMsg>) {
    let mut count = 0u32;
    let item_size = size_of::<RAWINPUTDEVICELIST>() as u32;
    let first = unsafe { GetRawInputDeviceList(None, &mut count, item_size) };
    if first == u32::MAX || count == 0 {
        let _ = tx.send(SecureMsg::Error("HID: raw device list empty".into()));
        return;
    }

    let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
    let got = unsafe { GetRawInputDeviceList(Some(devices.as_mut_ptr()), &mut count, item_size) };
    if got == u32::MAX {
        let _ = tx.send(SecureMsg::Error("HID: raw device list failed".into()));
        return;
    }

    let mut logged = 0usize;
    for item in devices.into_iter().take(count as usize) {
        if item.dwType != RIM_TYPEHID {
            continue;
        }

        let mut info = RID_DEVICE_INFO {
            cbSize: size_of::<RID_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        let mut size = size_of::<RID_DEVICE_INFO>() as u32;
        let ok = unsafe {
            GetRawInputDeviceInfoW(
                item.hDevice,
                RIDI_DEVICEINFO,
                Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                &mut size,
            )
        };
        if ok == u32::MAX {
            continue;
        }

        let hid = unsafe { info.Anonymous.hid };
        let is_sony = hid.dwVendorId == 0x054c;
        if logged < 12 || is_sony {
            logged += 1;
            let _ = tx.send(SecureMsg::Error(format!(
                "HID: raw dev vid={:04x} pid={:04x} page=0x{:04x} usage=0x{:04x}",
                hid.dwVendorId, hid.dwProductId, hid.usUsagePage, hid.usUsage,
            )));
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
        service_log(&format!(
            "HID secure: held 0x{cur:04x} [{src}] raw={raw_hex} ({dev})"
        ));
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

    // Re-enumerate XUSB pads while we have none — a controller connected after the
    // worker started (common at the lock screen) was missed by the one-shot
    // `open_all`, leaving only the foreground-gated DLL (always zeroed here).
    // Throttled to ~1s; only log on success so an empty scan doesn't spam.
    if state.xusb.is_empty() && state.last_xusb_scan.elapsed() >= Duration::from_secs(1) {
        state.last_xusb_scan = Instant::now();
        let (devices, log) = XusbDevice::open_all();
        if !devices.is_empty() {
            state.xusb = devices;
            for line in log {
                let _ = state.tx.send(SecureMsg::Error(line));
            }
        }
    }

    // Drop unplugged HID readers, and re-enumerate while we have none (a pad
    // connected after spawn — common at the lock screen — was missed by the
    // one-shot open in secure_poll_main).
    state.hid_readers.retain(|r| !r.is_dead());
    if state.hid_readers.is_empty() && state.last_hid_scan.elapsed() >= Duration::from_secs(1) {
        state.last_hid_scan = Instant::now();
        let (readers, log) = crate::hid_reader::HidReader::open_all();
        if !readers.is_empty() {
            state.hid_readers = readers;
            for line in log {
                let _ = state.tx.send(SecureMsg::Error(line));
            }
        }
    }

    // Drain each reader's overlapped read; keep the freshest non-neutral sample
    // (a pad streaming neutral frames shouldn't blank a just-pressed button from
    // another). `last_hid` then feeds the hid_authoritative slot-0 path below.
    let mut hid_sample: Option<PadSample> = None;
    for reader in state.hid_readers.iter_mut() {
        if let Some(sample) = reader.poll() {
            let replace = match hid_sample {
                None => true,
                Some(prev) => prev.buttons == 0 && prev.lt == 0 && prev.rt == 0,
            };
            if replace {
                hid_sample = Some(sample);
            }
        }
    }
    if let Some(sample) = hid_sample {
        state.last_hid = sample;
    }
    state.hid_readers.retain(|r| !r.is_dead());

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
            let _ = state.tx.send(SecureMsg::Error(format!(
                "XInputGetState({slot}) error {err}"
            )));
        }
        // XInputGetKeystroke is foreground-gated like GetState. When physical XUSB
        // pads are open we read buttons directly from the driver below (no
        // foreground needed), so skip the gated path entirely — otherwise it would
        // fight `prev_buttons` with the XUSB edges.
        if state.xusb.is_empty() {
            if let Some(get_keystroke) = state.get_keystroke {
                state.keystroke_events +=
                    secure_poll_keystrokes(&state.tx, get_keystroke, slot, &mut state.prev_buttons);
            }
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
    // Keep the last good report: a connected pad occasionally returns no bytes for
    // a single poll; replacing with None would flash a neutral (all-released) frame
    // and emit spurious button-up edges.
    if xusb_rep.is_some() {
        state.last_xusb = xusb_rep;
    }
    // HID is authoritative for PlayStation/non-XUSB pads. Xbox controllers can
    // also surface Raw Input HID devices; do not let that shadow the direct XUSB
    // path, which is the reliable secure-desktop source for Xbox.
    let hid_authoritative = hid_is_authoritative(state.hid_readers.len(), state.xusb.len());
    if hid_authoritative {
        connected[0] = true;
    }
    if hid_authoritative != state.hid_active_prev {
        state.hid_active_prev = hid_authoritative;
        let _ = state.tx.send(SecureMsg::HidActive(hid_authoritative));
    }

    if INJECT_HOLD_ACTIVE.load(Ordering::Relaxed) {
        let ticks = INJECT_HOLD_TICKS.load(Ordering::Relaxed);
        if ticks > 0 {
            INJECT_HOLD_TICKS.store(ticks - 1, Ordering::Relaxed);
            let _ = state.tx.send(SecureMsg::Axes((0.0, 0.0, 0.0, 0.0)));
            return;
        }

        INJECT_HOLD_ACTIVE.store(false, Ordering::Relaxed);
        state.last_xusb = None;
        for (slot, prev) in state.prev_buttons.iter_mut().enumerate() {
            if *prev != 0 {
                let old = *prev;
                *prev = 0;
                let _ = state.tx.send(SecureMsg::Buttons {
                    slot: slot as u32,
                    prev: old,
                    cur: 0,
                });
            }
        }
        let _ = state.tx.send(SecureMsg::Axes((0.0, 0.0, 0.0, 0.0)));
        return;
    }

    // Surface the raw XUSB report in the debug overlay so byte offsets can be
    // confirmed against live presses on Winlogon (the parsed `buttons` offset is
    // provisional — see xusb_ioctl::parse_report).
    // Verify the foreground experiment: show XInput slot-0 buttons + whether our
    // process currently owns the foreground window. If `fg=ours` and `xinput` goes
    // non-zero on a press, the gate is defeated and the existing edge path delivers it.
    {
        let xin = states[0].map(|p| p.wButtons.0).unwrap_or(0);
        let fg = unsafe { GetForegroundWindow() };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
        let ours = pid == unsafe { GetCurrentProcessId() };
        // Live press-test signal: show BOTH sources + an XUSB raw fingerprint that
        // changes whenever any report byte moves. On a press one of these must move,
        // or the pad stream is not reaching us at all (desktop/foreground gate).
        // Fingerprint the INPUT bytes only — skip header (0..5) and the free-running
        // counter at bytes 5..7, which tick every report even with zero input (that
        // was the "weird xfp stream"). xfp now moves only on a real button/stick/
        // trigger change, so a frozen xfp during a press == no live input reaching us.
        let (xusb_btn, xfp) = match &state.last_xusb {
            Some(r) => (
                r.buttons,
                r.raw
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i >= 7)
                    .fold(0u32, |a, (_, &b)| a.wrapping_mul(31).wrapping_add(b as u32)),
            ),
            None => (0u16, 0u32),
        };
        let hid = state.last_hid;
        crate::debug_state::set_detail(format!(
            "xinput=0x{xin:04x} xusb=0x{xusb_btn:04x} xfp=0x{xfp:08x} hid=0x{:04x}/{}:{} L({:.2},{:.2}) R({:.2},{:.2}) fg={}",
            hid.buttons,
            hid.lt,
            hid.rt,
            hid.lx,
            hid.ly,
            hid.rx,
            hid.ry,
            if ours { "ours" } else { "LogonUI" }
        ));
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
        let xusb_str = match &state.last_xusb {
            Some(rep) => {
                let full: String = rep
                    .raw
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    " xusb:btn=0x{:04x} lt={} rt={} lx={} ly={} rx={} ry={} len={} raw=[{}]",
                    rep.buttons,
                    rep.left_trigger,
                    rep.right_trigger,
                    rep.thumb_lx,
                    rep.thumb_ly,
                    rep.thumb_rx,
                    rep.thumb_ry,
                    rep.raw.len(),
                    full,
                )
            }
            None => " xusb:none".into(),
        };
        let _ = state.tx.send(SecureMsg::Error(format!(
            "probe iter={} errs=[{}]{raw} keystroke_events={} hid:btn=0x{:04x} lt={} rt={} lx={:.2} ly={:.2} rx={:.2} ry={:.2} raw=[{}]{xusb_str}",
            state.iter_count,
            summary.join(","),
            state.keystroke_events,
            state.last_hid.buttons,
            state.last_hid.lt,
            state.last_hid.rt,
            state.last_hid.lx,
            state.last_hid.ly,
            state.last_hid.rx,
            state.last_hid.ry,
            // Prefer the direct-read bytes (the working secure-desktop path);
            // fall back to the WM_INPUT report, which never arrives on Winlogon.
            state
                .hid_readers
                .first()
                .map(|r| report_hex(r.last_report()))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| report_hex(&state.last_raw_report)),
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

    if hid_authoritative && slot == 0 {
        let prev = state.prev_buttons[0];
        let buttons = state.last_hid.buttons;
        if prev != buttons {
            state.prev_buttons[0] = buttons;
            let _ = state.tx.send(SecureMsg::Buttons {
                slot: 0,
                prev,
                cur: buttons,
            });
        }
        let mut trigger_edges = Vec::new();
        poll_trigger_edges(
            &mut state.prev_trigger_left,
            &mut state.prev_trigger_right,
            state.last_hid.lt,
            state.last_hid.rt,
            &mut trigger_edges,
        );
        for edge in trigger_edges {
            let _ = state.tx.send(SecureMsg::Trigger(edge));
        }
        let _ = state.tx.send(SecureMsg::Axes((
            state.last_hid.lx,
            state.last_hid.ly,
            state.last_hid.rx,
            state.last_hid.ry,
        )));
        return;
    }

    // The DLL state may be absent (slot connected only via the direct XUSB read,
    // which the foreground gate doesn't touch) — default to neutral, never panic.
    let pad = states[slot as usize].unwrap_or_default();

    // Authoritative input source. On the secure desktop the DLL `pad` reads
    // neutral (btn=0) even while our anchor holds foreground — the invisible anchor
    // does not actually grant LogonUI's input rights — so the XUSB-direct read is
    // the only source that carries live buttons there. Prefer XUSB whenever a pad
    // is open; fall back to the DLL only when no XUSB pad is present. (During an
    // inject burst the anchor yields foreground for ~48ms and XUSB may briefly
    // freeze, but the user is not navigating then, so that is harmless.)
    let xusb = state.last_xusb.clone().filter(|_| !state.xusb.is_empty());
    let (buttons, lx, ly, rx, ry, lt, rt) = match &xusb {
        Some(r) => (
            r.buttons,
            r.thumb_lx,
            r.thumb_ly,
            r.thumb_rx,
            r.thumb_ry,
            r.left_trigger,
            r.right_trigger,
        ),
        None => (
            pad.wButtons.0,
            pad.sThumbLX,
            pad.sThumbLY,
            pad.sThumbRX,
            pad.sThumbRY,
            pad.bLeftTrigger,
            pad.bRightTrigger,
        ),
    };

    let mut trigger_edges = Vec::new();
    poll_trigger_edges(
        &mut state.prev_trigger_left,
        &mut state.prev_trigger_right,
        lt,
        rt,
        &mut trigger_edges,
    );
    for edge in trigger_edges {
        let _ = state.tx.send(SecureMsg::Trigger(edge));
    }

    // Button edges from the authoritative source — handles press AND release, so it
    // owns `prev_buttons` outright (the foreground-gated keystroke path is skipped
    // above whenever an XUSB pad is open).
    let idx = slot as usize;
    let prev = state.prev_buttons[idx];
    if prev != buttons {
        state.prev_buttons[idx] = buttons;
        let _ = state.tx.send(SecureMsg::Buttons {
            slot,
            prev,
            cur: buttons,
        });
    }

    let stick_active = lx.abs() > LEFT_DEADZONE || ly.abs() > LEFT_DEADZONE;
    let axes = if stick_active {
        (
            XInputBackend::norm_thumb(lx, LEFT_DEADZONE),
            XInputBackend::norm_thumb(ly, LEFT_DEADZONE),
            XInputBackend::norm_thumb(rx, RIGHT_DEADZONE),
            XInputBackend::norm_thumb(ry, RIGHT_DEADZONE),
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

fn hid_is_authoritative(hid_device_count: usize, xusb_device_count: usize) -> bool {
    hid_device_count > 0 && xusb_device_count == 0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xinput_mask_edges_include_system_and_stick_buttons() {
        let cur = XINPUT_GAMEPAD_BACK.0
            | XINPUT_GAMEPAD_START.0
            | XINPUT_GAMEPAD_LEFT_THUMB.0
            | XINPUT_GAMEPAD_RIGHT_THUMB.0
            | GUIDE_BUTTON_MASK;
        let edges = XInputBackend::edges(0, cur);
        let pressed: Vec<Button> = edges
            .into_iter()
            .filter_map(|edge| edge.pressed.then_some(edge.button))
            .collect();

        assert_eq!(
            pressed,
            vec![
                Button::Select,
                Button::Start,
                Button::L3,
                Button::R3,
                Button::Guide,
            ]
        );
        assert_eq!(button_mask(Button::Select), Some(XINPUT_GAMEPAD_BACK.0));
        assert_eq!(button_mask(Button::Start), Some(XINPUT_GAMEPAD_START.0));
        assert_eq!(button_mask(Button::Guide), Some(GUIDE_BUTTON_MASK));
    }

    #[test]
    fn controller_label_identifies_secure_hid_source() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.active_secure_hid = true;

        assert_eq!(backend.controller_label(), "HID slot 0");
    }

    #[test]
    fn clear_secure_state_removes_hid_label() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.active_secure_hid = true;

        backend.clear_secure_state();

        assert_eq!(backend.controller_label(), "XInput slot 0");
    }

    #[test]
    fn live_input_summary_includes_held_triggers() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.prev_trigger_left = true;
        backend.prev_trigger_right = true;

        let summary = backend.live_input_summary();
        let parts: Vec<&str> = summary.split_whitespace().collect();
        assert!(parts.contains(&"LT"));
        assert!(parts.contains(&"RT"));
    }

    #[test]
    fn clear_secure_state_removes_trigger_summary_state() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.prev_trigger_left = true;
        backend.prev_trigger_right = true;

        backend.clear_secure_state();

        assert!(!backend.live_input_summary().contains("LT"));
        assert!(!backend.live_input_summary().contains("RT"));
    }

    #[test]
    fn xusb_presence_keeps_hid_from_shadowing_slot_zero() {
        assert!(hid_is_authoritative(1, 0));
        assert!(!hid_is_authoritative(1, 1));
        assert!(!hid_is_authoritative(0, 1));
    }
}
