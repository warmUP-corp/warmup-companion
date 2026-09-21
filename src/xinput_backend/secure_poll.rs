//! Winlogon secure poll thread, anchor window, and message pump.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{
    BOOL, HINSTANCE, HWND, LPARAM, LRESULT, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::UI::Input::HRAWINPUT;
use crate::xusb_ioctl::XusbDevice;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    KillTimer, MsgWaitForMultipleObjects, PeekMessageW, PostThreadMessageW, RegisterClassW,
    SetLayeredWindowAttributes, SetTimer, TranslateMessage, HMENU, LWA_ALPHA, MSG, PM_REMOVE,
    QS_ALLINPUT, WM_DESTROY, WM_INPUT, WM_NULL, WM_POWERBROADCAST, WM_QUIT, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::gamepad_backend::mapping_db_path;
use crate::hid_gamepad::{self, PadSample};

use super::identity::probe_self_identity;
use super::raw_hid::{poll_raw_hid_input, register_raw_gamepad};
use super::types::{PollState, SecureMsg};
use super::xinput_poll::{load_xinput_api, poll_xinput_tick};
use super::{LOGON_FG_HWND, service_log};

// WM_POWERBROADCAST wParam values (Win32_System_Power feature not enabled; these
// are stable platform constants). Resume-from-suspend / resume-automatic.
const PBT_APMRESUMESUSPEND: usize = 0x0007;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;

thread_local! {
    static POLL_STATE: RefCell<Option<PollState>> = const { RefCell::new(None) };
}

#[allow(dead_code)]
const ANCHOR_CLASS: windows::core::PCWSTR = w!("WarmupXInputAnchorWindow");
#[allow(dead_code)]
const POLL_TIMER_ID: usize = 21;
#[allow(dead_code)]
const POLL_TIMER_MS: u32 = 8;
pub(crate) struct SecurePollThread {
    pub(crate) rx: mpsc::Receiver<SecureMsg>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    thread_id: u32,
}
impl SecurePollThread {
    pub(crate) fn spawn() -> Result<Self, String> {
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
    //    Poll XInput on the window-owning thread under the Winlogon worker
    //    token, not via ImpersonateLoggedOnUser on the timer path (see NOTES.md).
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
        // Anchor (exstyle 0x08080088 = NOACTIVATE|TOOLWINDOW|LAYERED
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
    // The anchor never takes foreground; the pad-read grant is
    // pursued via the focus-owner mechanism, not foreground ownership.
    unsafe {
        let _ =
            SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
    }
    let _ = tx.send(SecureMsg::Error(
        "anchor window created (NOACTIVATE; no foreground steal)".into(),
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
            last_slot_err_log: crate::time_util::stale(Duration::from_secs(60)),
            prev_trigger_left: false,
            prev_trigger_right: false,
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
    //    We wait with a 250ms timeout instead of blocking forever in GetMessageW:
    //    a wedged/dead message queue (lost input desktop, killed timer) can no
    //    longer hang the pump while ignoring `stop`. The poll timer fires every
    //    ~8ms, so a full 250ms of silence is abnormal; after ~750ms we exit and
    //    let sync_secure_helper respawn a fresh thread on the current desktop.
    let mut msg = MSG::default();
    let mut idle_streak: u8 = 0;
    'pump: loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let wait = unsafe { MsgWaitForMultipleObjects(None, BOOL(0), 250, QS_ALLINPUT) };
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if wait == WAIT_TIMEOUT {
            idle_streak = idle_streak.saturating_add(1);
            if idle_streak >= 3 {
                let _ = tx.send(SecureMsg::Error(
                    "secure pump silent >750ms (wedged); exiting for respawn".into(),
                ));
                break;
            }
            continue;
        }
        idle_streak = 0;
        // Drain everything pending without blocking; WM_QUIT ends the pump.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                break 'pump;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe {
        let _ = KillTimer(hwnd, POLL_TIMER_ID);
        let _ = DestroyWindow(hwnd);
    }
    POLL_STATE.with(|s| *s.borrow_mut() = None);
}

thread_local! {
    /// Throttle for the recovered-panic log so a tick that panics every frame
    /// can't itself spam the service log (it would reintroduce the disk-fill).
    static LAST_POLL_PANIC_LOG: std::cell::Cell<Option<Instant>> =
        const { std::cell::Cell::new(None) };
}

/// Record that a poll tick panicked and was caught at the FFI boundary. Logs at
/// most once per second; the pump continues either way.
fn note_poll_panic(kind: &str) {
    let now = Instant::now();
    let should_log = LAST_POLL_PANIC_LOG.with(|c| match c.get() {
        Some(t) if now.duration_since(t) < Duration::from_secs(1) => false,
        _ => {
            c.set(Some(now));
            true
        }
    });
    if should_log {
        service_log(&format!(
            "secure poll tick panicked in {kind}; recovered, pump alive"
        ));
    }
}
#[allow(dead_code)]
unsafe extern "system" fn anchor_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TIMER && wparam.0 == POLL_TIMER_ID {
        // Never touch foreground. We only record the credential
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
                // A panic must NOT unwind out of this `extern "system"` callback —
                // that aborts the whole process (Rust >= 1.81). Catch it here so a
                // bad poll tick is recovered and the message pump stays alive.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    poll_xinput_tick(state)
                }))
                .is_err()
                {
                    note_poll_panic("xinput_tick");
                }
            }
        });
        return LRESULT(0);
    }
    if msg == WM_INPUT {
        POLL_STATE.with(|s| {
            if let Some(state) = s.borrow_mut().as_mut() {
                let raw = HRAWINPUT(lparam.0 as _);
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    poll_raw_hid_input(state, raw)
                }))
                .is_err()
                {
                    note_poll_panic("raw_hid_input");
                }
            }
        });
        return LRESULT(0);
    }
    if msg == WM_POWERBROADCAST {
        // On resume, XUSB handles / HID overlapped reads / the XInput DLL's internal
        // slot state can all be stale. Flush the device handles and force immediate
        // re-enumeration on the next poll tick so input recovers without a replug.
        if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize || wparam.0 == PBT_APMRESUMESUSPEND as usize
        {
            POLL_STATE.with(|s| {
                if let Some(state) = s.borrow_mut().as_mut() {
                    state.xusb.clear();
                    state.hid_readers.clear();
                    state.last_xusb_scan = crate::time_util::stale(Duration::from_secs(60));
                    state.last_hid_scan = crate::time_util::stale(Duration::from_secs(60));
                    let _ = state.tx.send(SecureMsg::Error(
                        "resume: flushing pad handles for re-enum".into(),
                    ));
                }
            });
        }
        return LRESULT(1);
    }
    if msg == WM_DESTROY {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
