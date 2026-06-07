//! Shared secure-desktop UI-thread band.
//!
//! Owns the part both the Xbox VK window and the debug overlay copied verbatim:
//! spawn a dedicated UI thread, set DPI awareness, register the window class,
//! run the `GetMessageW` pump, and route the show/hide/quit/repaint thread
//! messages. Thread messages have `hwnd == NULL`, so they are handled here in
//! the pump — never in the adapter's `wndproc`.
//!
//! An adapter implements [`DesktopApp`]: it supplies the class name, background,
//! `wndproc`, and the show/hide bodies. The WinEvent reattach-on-desktop-switch
//! logic lives behind [`DesktopApp::install_hooks`] (the VK window uses it; the
//! overlay leaves it default-empty), so the seam is real — two adapters, one band.

use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, LoadCursorW, PeekMessageW, PostQuitMessage, PostThreadMessageW,
    RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, MSG, PM_NOREMOVE, WM_USER,
    WNDCLASSW,
};

/// Thread messages the pump understands (posted with `hwnd == NULL`).
pub const WM_APP_SHOW: u32 = WM_USER;
pub const WM_APP_HIDE: u32 = WM_USER + 1;
pub const WM_APP_QUIT: u32 = WM_USER + 2;
pub const WM_APP_REPAINT: u32 = WM_USER + 3;

/// A paint-only adapter plugged into the shared UI-thread band.
///
/// Everything runs on the dedicated UI thread except the spawn handshake. The
/// adapter value is moved onto that thread and owns any per-thread state (e.g.
/// installed WinEvent hooks).
pub trait DesktopApp: Send + 'static {
    const THREAD_NAME: &'static str;
    const CLASS_NAME: PCWSTR;
    const BG_COLOR: u32;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

    /// Runs on the UI thread before class registration. `Err` aborts the spawn
    /// (the error is propagated to the caller of [`spawn`]).
    fn on_thread_start(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Runs once the queue exists and the thread id is known, before the spawn
    /// handshake completes.
    fn on_ready(&mut self, _thread_id: u32) {}
    /// Install WinEvent hooks (delivered on this thread during the pump).
    fn install_hooks(&mut self) {}
    /// Tear down anything `install_hooks` set up; runs after the pump exits.
    fn remove_hooks(&mut self) {}

    fn on_show(&mut self, lparam: LPARAM);
    fn on_hide(&mut self);
    fn on_repaint(&mut self) {}
}

fn run<A: DesktopApp>(ready: SyncSender<Result<u32, String>>, mut app: A) {
    if let Err(e) = app.on_thread_start() {
        let _ = ready.send(Err(e));
        return;
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).expect("module handle");
        let bg = CreateSolidBrush(COLORREF(A::BG_COLOR));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(A::WNDPROC),
            hInstance: instance.into(),
            lpszClassName: A::CLASS_NAME,
            hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
            hbrBackground: bg,
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&wc);
        // Queue must exist before the main thread posts WM_APP_SHOW.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    }

    let thread_id = unsafe { GetCurrentThreadId() };
    app.on_ready(thread_id);
    let _ = ready.send(Ok(thread_id));
    app.install_hooks();

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        match msg.message {
            WM_APP_SHOW => app.on_show(msg.lParam),
            WM_APP_HIDE => app.on_hide(),
            WM_APP_REPAINT => app.on_repaint(),
            WM_APP_QUIT => unsafe {
                PostQuitMessage(0);
            },
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }

    app.remove_hooks();
}

/// Handle to a running UI-thread band. Drop posts `WM_APP_QUIT` and joins.
pub struct DesktopWindowThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

/// Spawn the UI thread for `app` and block until it reports its thread id.
pub fn spawn<A: DesktopApp>(app: A) -> Result<DesktopWindowThread, String> {
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);
    let join = thread::Builder::new()
        .name(A::THREAD_NAME.into())
        .spawn(move || run(ready_tx, app))
        .map_err(|e| format!("{}: {e}", A::THREAD_NAME))?;
    let thread_id = ready_rx
        .recv()
        .map_err(|_| format!("{} exited before ready", A::THREAD_NAME))??;
    Ok(DesktopWindowThread {
        thread_id,
        join: Some(join),
    })
}

impl DesktopWindowThread {
    pub fn post(&self, msg: u32, lparam: LPARAM) -> Result<(), String> {
        unsafe {
            PostThreadMessageW(self.thread_id, msg, WPARAM(0), lparam)
                .map_err(|e| format!("PostThreadMessageW({msg}): {e}"))
        }
    }

    pub fn show(&self, lparam: LPARAM) -> Result<(), String> {
        self.post(WM_APP_SHOW, lparam)
    }

    pub fn hide(&self) -> Result<(), String> {
        self.post(WM_APP_HIDE, LPARAM(0))
    }
}

impl Drop for DesktopWindowThread {
    fn drop(&mut self) {
        let _ = self.post(WM_APP_QUIT, LPARAM(0));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
