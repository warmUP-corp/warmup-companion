//! UI Automation focus redirect for the Winlogon/secure desktop.
//!
//! On the lock/sign-in screen the foreground window is class `LogonUI Logon
//! Window`, hosting the credential UI as XAML *inside* it — there is no child
//! edit HWND, and the password box's focus is XAML-internal. Windows' built-in
//! CoreWindow gamepad navigation moves that internal focus off the password box
//! on every D-pad / stick press, so a plain `SendInput` lands on whatever button
//! the nav drifted to. The fix is to re-target the password element via UIA
//! `SetFocus` immediately before each key, regardless of where nav drifted.
//!
//! This is a Warmup-only path, separate from the baseline Win32 focus path.
//!
//! Step 1 (this module so far): a `dump_foreground_tree()` probe to confirm the
//! password element's identity (ControlType / IsPassword / Name) on a real
//! secure desktop before the finder conditions are trusted. COM + the
//! `IUIAutomation` instance live on the caller thread (the gamepad loop thread,
//! which is MTA and attached to Winlogon).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Subtree,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
#[cfg(feature = "gamepad")]
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

/// UIA control-type id for an editable field.
const CT_EDIT: i32 = 50004;

/// Set by the debug overlay (F9) on the input desktop; drained by the gamepad
/// loop thread, which owns the COM apartment, to run the dump there.
static DUMP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// True while the input desktop is Winlogon (published each poll by the gamepad
/// loop). Gates the per-keystroke focus redirect so userland is untouched.
static ON_WINLOGON: AtomicBool = AtomicBool::new(false);

/// Set whenever we stop the search service for the secure desktop (and `true`
/// at startup); cleared after we issue a userland `sc start`. Lets us re-start
/// the service exactly once per userland transition — including the orphan case
/// where this process started fresh in userland after a prior run stopped it —
/// without spawning `sc.exe` on every poll.
static NEED_SEARCH_START: AtomicBool = AtomicBool::new(true);

/// Thread id of the gamepad loop (the COM/UIA apartment + Winlogon-attached
/// thread). `focus_password_field` no-ops off this thread so the VK UI thread's
/// mouse path never inits COM on the wrong apartment.
static LOOP_TID: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// COM initialized (MTA) on this thread? One-shot.
    static COM_READY: RefCell<bool> = const { RefCell::new(false) };
    /// Cached automation client for this thread (apartment-bound).
    static AUTOMATION: RefCell<Option<IUIAutomation>> = const { RefCell::new(None) };
    /// Cached password element; refound when SetFocus fails or on winlogon exit.
    static PWD_ELEMENT: RefCell<Option<IUIAutomationElement>> = const { RefCell::new(None) };
    /// Last focus status logged, to avoid flooding the log per keystroke.
    static LAST_STATUS: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

/// Overlay hook: ask the loop thread to dump the foreground UIA subtree.
pub fn request_dump() {
    DUMP_REQUESTED.store(true, Ordering::SeqCst);
}

/// Loop thread: if a dump was requested, run it here (MTA, on Winlogon).
pub fn take_dump_request() -> bool {
    DUMP_REQUESTED.swap(false, Ordering::SeqCst)
}

fn log(msg: &str) {
    crate::install::log_line(msg);
}

/// Published each poll by the gamepad loop. When entering Winlogon it records
/// the loop thread id (the apartment that owns UIA); when leaving it drops the
/// cached password element so a later return refinds against the new tree.
pub fn set_active(on_winlogon: bool) {
    let was = ON_WINLOGON.swap(on_winlogon, Ordering::SeqCst);
    if on_winlogon {
        LOOP_TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        if !was {
            // Entered the secure desktop: stop the native touch keyboard being
            // summoned on credential-field focus (the hide loop only flashes it),
            // and stop the search service live so the gamepad keyboard can't be
            // summoned. `Start` is left untouched (always auto) so userland — and
            // any reboot — can bring it straight back.
            crate::win::native_keyboard::disable_auto_invoke();
            crate::win::native_keyboard::stop_search_service();
            NEED_SEARCH_START.store(true, Ordering::SeqCst);
        }
    } else {
        // Userland. Re-start the search service once per transition (and once at
        // startup) so Start-menu search works. Guarded by NEED_SEARCH_START so we
        // don't spawn `sc.exe` every poll; the startup default of `true` also
        // covers the orphan case (process started fresh in userland after a prior
        // run stopped the service, so there is no Winlogon->userland edge to ride).
        if NEED_SEARCH_START.swap(false, Ordering::SeqCst) {
            crate::win::native_keyboard::ensure_search_service_running();
        }
        if was {
            clear_cache();
            crate::win::native_keyboard::restore_auto_invoke();
        }
    }
}

fn active() -> bool {
    ON_WINLOGON.load(Ordering::SeqCst)
}

/// True when the input desktop is Winlogon (the secure desktop). Userland inject
/// paths use this to gate Winlogon-only behavior / diagnostics.
pub fn is_active() -> bool {
    active()
}

/// Userland personal-dictionary gate (ADR 0001). `None` => conservative skip.
pub fn focused_is_password_field() -> Option<bool> {
    if active() {
        return Some(false);
    }
    if unsafe { GetCurrentThreadId() } != LOOP_TID.load(Ordering::SeqCst) {
        return None;
    }
    let auto = automation()?;
    let el = unsafe { auto.GetFocusedElement().ok()? };
    Some(unsafe { el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) })
}

/// Drop the cached password element (winlogon exit, VK close, account switch).
pub fn clear_cache() {
    let _ = PWD_ELEMENT.try_with(|s| *s.borrow_mut() = None);
    let _ = LAST_STATUS.try_with(|s| *s.borrow_mut() = None);
}

/// Log a focus status string only when it changes (one line per state, not per key).
fn log_status(status: &'static str) {
    LAST_STATUS.with(|last| {
        if *last.borrow() != Some(status) {
            *last.borrow_mut() = Some(status);
            log(&format!("logon focus: {status}"));
        }
    });
}

/// Find the credential password element under the foreground window: prefer
/// `IsPassword`, else the first keyboard-focusable `Edit`. Flat subtree scan
/// (TrueCondition) — robust to Win32-child vs XAML-island hosting. None if the
/// surface has no such field (e.g. a Yes/No UAC) — caller falls back to plain
/// `SendInput`.
unsafe fn find_password_element(auto: &IUIAutomation) -> Option<IUIAutomationElement> {
    let fg = GetForegroundWindow();
    if fg.0.is_null() {
        return None;
    }
    let root = auto.ElementFromHandle(fg).ok()?;
    let cond = auto.CreateTrueCondition().ok()?;
    let arr = root.FindAll(TreeScope_Subtree, &cond).ok()?;
    let n = arr.Length().unwrap_or(0);
    let mut fallback_edit: Option<IUIAutomationElement> = None;
    for i in 0..n {
        let Ok(el) = arr.GetElement(i) else { continue };
        if el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) {
            return Some(el);
        }
        if fallback_edit.is_none() {
            let ct = el.CurrentControlType().map(|c| c.0).unwrap_or(0);
            let focusable = el
                .CurrentIsKeyboardFocusable()
                .map(|b| b.as_bool())
                .unwrap_or(false);
            if ct == CT_EDIT && focusable {
                fallback_edit = Some(el);
            }
        }
    }
    fallback_edit
}

/// Re-target the credential password element via UIA `SetFocus` so the next
/// `SendInput` lands there regardless of where CoreWindow nav drifted. No-op
/// (returns false) off Winlogon or off the loop thread. Returns true only when
/// a field was focused; callers `SendInput` either way (false = plain fallback).
pub fn focus_password_field() -> bool {
    if !active() {
        return false;
    }
    if unsafe { GetCurrentThreadId() } != LOOP_TID.load(Ordering::SeqCst) {
        return false;
    }
    let Some(auto) = automation() else {
        return false;
    };

    // Foreground-juggle: the secure poll holds foreground so the pad stays
    // readable (XUSB is foreground-gated), but SendInput must reach LogonUI's PIN
    // field. Restore the credential window to foreground and suppress the anchor's
    // reclaim for this burst. Must precede find_password_element(), which reads
    // GetForegroundWindow() to locate the surface.
    #[cfg(feature = "gamepad")]
    if let Some(cred) = crate::xinput_backend::logon_credential_window() {
        crate::xinput_backend::begin_inject_hold();
        unsafe {
            let _ = SetForegroundWindow(cred);
        }
        crate::win::native_keyboard::suppress();
    }

    // Cached element first; SetFocus error => stale, drop and refind.
    let cached = PWD_ELEMENT.with(|s| s.borrow().clone());
    if let Some(el) = cached {
        if unsafe { el.SetFocus() }.is_ok() {
            crate::win::native_keyboard::suppress();
            log_status("focused (cached)");
            return true;
        }
        PWD_ELEMENT.with(|s| *s.borrow_mut() = None);
    }

    match unsafe { find_password_element(&auto) } {
        Some(el) => {
            let ok = unsafe { el.SetFocus() }.is_ok();
            PWD_ELEMENT.with(|s| *s.borrow_mut() = Some(el));
            if ok {
                crate::win::native_keyboard::suppress();
            }
            log_status(if ok {
                "focused"
            } else {
                "found, SetFocus failed"
            });
            ok
        }
        None => {
            log_status("no password element (fallback SendInput)");
            false
        }
    }
}

/// Lazily init COM (MTA) once on the calling thread. Tolerates an already-init
/// apartment (S_FALSE) but logs a real mismatch (RPC_E_CHANGED_MODE).
fn ensure_com() -> bool {
    COM_READY.with(|ready| {
        if *ready.borrow() {
            return true;
        }
        // SAFETY: one-shot per thread; the loop thread has no other apartment.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            log(&format!(
                "logon focus: CoInitializeEx failed hr=0x{:08x}",
                hr.0
            ));
            return false;
        }
        *ready.borrow_mut() = true;
        true
    })
}

/// Get (or lazily create) the thread's `IUIAutomation` client.
fn automation() -> Option<IUIAutomation> {
    if !ensure_com() {
        return None;
    }
    AUTOMATION.with(|slot| {
        if let Some(a) = slot.borrow().as_ref() {
            return Some(a.clone());
        }
        // SAFETY: CUIAutomation is a registered in-proc COM server.
        let created: windows::core::Result<IUIAutomation> =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) };
        match created {
            Ok(a) => {
                *slot.borrow_mut() = Some(a.clone());
                Some(a)
            }
            Err(e) => {
                log(&format!(
                    "logon focus: CoCreateInstance(CUIAutomation) failed: {e}"
                ));
                None
            }
        }
    })
}

/// Map a UIA control-type id to a short label for the dump (full list is large;
/// only the ones relevant to a credential surface are named).
fn control_type_label(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50004 => "Edit",
        50020 => "Text",
        50026 => "Group",
        50032 => "Window",
        50033 => "Pane",
        _ => "?",
    }
}

/// Dump the foreground window's UIA subtree to the service log: ControlType,
/// IsPassword, IsKeyboardFocusable, Name, AutomationId, ClassName for each
/// element. Flat (TreeScope_Subtree + TrueCondition) — enough to identify the
/// password element before the finder is built. Bounded to avoid log flooding.
pub fn dump_foreground_tree() {
    let Some(auto) = automation() else {
        return;
    };
    // SAFETY: all calls below are on COM objects valid for this apartment.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            log("logon focus: dump — no foreground window");
            return;
        }
        let root: IUIAutomationElement = match auto.ElementFromHandle(fg) {
            Ok(e) => e,
            Err(e) => {
                log(&format!("logon focus: ElementFromHandle failed: {e}"));
                return;
            }
        };
        let root_name = root
            .CurrentName()
            .map(|b| b.to_string())
            .unwrap_or_default();
        let root_class = root
            .CurrentClassName()
            .map(|b| b.to_string())
            .unwrap_or_default();
        log(&format!(
            "logon focus: dump foreground hwnd=0x{:x} class='{root_class}' name='{root_name}'",
            fg.0 as usize
        ));

        let cond = match auto.CreateTrueCondition() {
            Ok(c) => c,
            Err(e) => {
                log(&format!("logon focus: CreateTrueCondition failed: {e}"));
                return;
            }
        };
        let arr = match root.FindAll(TreeScope_Subtree, &cond) {
            Ok(a) => a,
            Err(e) => {
                log(&format!("logon focus: FindAll failed: {e}"));
                return;
            }
        };
        let count = arr.Length().unwrap_or(0);
        log(&format!("logon focus: subtree has {count} element(s)"));
        let cap = count.min(200);
        for i in 0..cap {
            let Ok(el) = arr.GetElement(i) else { continue };
            let ct = el.CurrentControlType().map(|c| c.0).unwrap_or(0);
            let is_pwd = el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false);
            let focusable = el
                .CurrentIsKeyboardFocusable()
                .map(|b| b.as_bool())
                .unwrap_or(false);
            let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
            let autoid = el
                .CurrentAutomationId()
                .map(|b| b.to_string())
                .unwrap_or_default();
            let class = el
                .CurrentClassName()
                .map(|b| b.to_string())
                .unwrap_or_default();
            log(&format!(
                "  [{i}] ct={ct}({}) pwd={is_pwd} focusable={focusable} name='{name}' autoid='{autoid}' class='{class}'",
                control_type_label(ct)
            ));
        }
    }
}
