//! Top-level SEH handler for the service worker.
//!
//! The worker dies on native access violations (`0xC0000005`) that the Rust
//! panic hook cannot see — `std::panic` only catches Rust panics, not Win32
//! structured exceptions. This installs `SetUnhandledExceptionFilter` so a crash
//! logs the exception code + faulting address + owning module (as `module+rva`)
//! to the service log AND writes a full minidump to `C:\ProgramData\WarmupVk`
//! for postmortem in WinDbg / Visual Studio.

#![cfg(any(windows, target_os = "linux"))]

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
#[cfg(windows)]
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWithFullMemory, MiniDumpWithHandleData, MiniDumpWithThreadInfo, MiniDumpWriteDump,
    SetUnhandledExceptionFilter, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
};
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x4000_0000;
#[cfg(windows)]
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;
#[cfg(windows)]
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
#[cfg(windows)]
const EXCEPTION_EXECUTE_HANDLER: i32 = 1;

/// Re-entrancy guard: a fault while handling a fault must not loop.
static HANDLING: AtomicBool = AtomicBool::new(false);

/// Install the top-level exception filter. Call once at worker start.
#[cfg(windows)]
pub fn install() {
    unsafe {
        SetUnhandledExceptionFilter(Some(seh_filter));
    }
    crate::install::log_line("crash handler: SEH filter installed (minidump on AV)");
}

#[cfg(windows)]
unsafe fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Resolve `addr` to `module_filename+0xRVA`. Best-effort; falls back to the raw
/// address if the owning module cannot be found.
#[cfg(windows)]
unsafe fn module_for(addr: usize) -> String {
    let mut hmod = HMODULE::default();
    let ok = GetModuleHandleExW(
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        PCWSTR(addr as *const u16),
        &mut hmod,
    )
    .is_ok();
    if !ok || hmod.is_invalid() {
        return format!("0x{addr:016x} (module unknown)");
    }
    let mut buf = [0u16; 260];
    let len = GetModuleFileNameW(hmod, &mut buf) as usize;
    let name = if len > 0 {
        let full = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
        full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
    } else {
        "?".to_string()
    };
    let rva = addr.wrapping_sub(hmod.0 as usize);
    format!("{name}+0x{rva:x} (base 0x{:016x})", hmod.0 as usize)
}

#[cfg(windows)]
unsafe fn write_minidump(info: *const EXCEPTION_POINTERS) {
    let pid = GetCurrentProcessId();
    let path = wide(&format!(r"C:\ProgramData\WarmupVk\worker-crash-{pid}.dmp"));
    let file = match CreateFileW(
        PCWSTR(path.as_ptr()),
        GENERIC_WRITE,
        FILE_SHARE_NONE,
        None,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        HANDLE::default(),
    ) {
        Ok(h) if h != INVALID_HANDLE_VALUE => h,
        _ => {
            crate::install::log_line("crash handler: could not open dump file");
            return;
        }
    };

    let exc = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: GetCurrentThreadId(),
        ExceptionPointers: info as *mut EXCEPTION_POINTERS,
        ClientPointers: false.into(),
    };
    let dump_type = MINIDUMP_TYPE(
        MiniDumpWithFullMemory.0 | MiniDumpWithHandleData.0 | MiniDumpWithThreadInfo.0,
    );
    let res = MiniDumpWriteDump(
        GetCurrentProcess(),
        pid,
        file,
        dump_type,
        Some(&exc),
        None,
        None,
    );
    let _ = CloseHandle(file);
    match res {
        Ok(()) => crate::install::log_line(&format!(
            r"crash handler: minidump written -> C:\ProgramData\WarmupVk\worker-crash-{pid}.dmp"
        )),
        Err(e) => {
            crate::install::log_line(&format!("crash handler: MiniDumpWriteDump failed: {e}"))
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn seh_filter(info: *const EXCEPTION_POINTERS) -> i32 {
    if HANDLING.swap(true, Ordering::SeqCst) {
        // Already handling a crash; bail to the OS to avoid an infinite loop.
        return EXCEPTION_EXECUTE_HANDLER;
    }
    if info.is_null() {
        crate::install::log_line("WORKER SEH: null EXCEPTION_POINTERS");
        return EXCEPTION_EXECUTE_HANDLER;
    }
    let rec = (*info).ExceptionRecord;
    if rec.is_null() {
        crate::install::log_line("WORKER SEH: null ExceptionRecord");
        return EXCEPTION_EXECUTE_HANDLER;
    }
    let code = (*rec).ExceptionCode.0 as u32;
    let fault_pc = (*rec).ExceptionAddress as usize;
    let loc = module_for(fault_pc);

    // For access violations, ExceptionInformation[0] = 0 read / 1 write / 8 DEP,
    // ExceptionInformation[1] = the inaccessible virtual address.
    let extra = if code == 0xC000_0005 && (*rec).NumberParameters >= 2 {
        let op = match (*rec).ExceptionInformation[0] {
            0 => "read",
            1 => "write",
            8 => "execute(DEP)",
            _ => "access?",
        };
        let va = (*rec).ExceptionInformation[1];
        format!(" ({op} @ 0x{va:016x})")
    } else {
        String::new()
    };

    let summary = format!(
        "WORKER SEH: 0x{code:08X} at {loc}{extra} tid={}",
        GetCurrentThreadId()
    );
    crate::install::log_line(&summary);
    crate::sentry_telemetry::capture_native_crash(summary);
    write_minidump(info);

    // Terminate after we have logged + dumped; the launcher will relaunch.
    EXCEPTION_EXECUTE_HANDLER
}

#[cfg(target_os = "linux")]
const CRASH_SIGNALS: [libc::c_int; 5] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGABRT,
    libc::SIGILL,
    libc::SIGFPE,
];

#[cfg(target_os = "linux")]
static CRASH_TID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[cfg(target_os = "linux")]
static REPORT_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

#[cfg(target_os = "linux")]
static REPORT_PATHS: std::sync::OnceLock<(std::ffi::CString, std::ffi::CString)> =
    std::sync::OnceLock::new();

#[cfg(target_os = "linux")]
static PREVIOUS_ACTIONS: std::sync::OnceLock<Vec<libc::sigaction>> = std::sync::OnceLock::new();

#[cfg(target_os = "linux")]
pub fn report_file() -> std::path::PathBuf {
    crate::paths::state_dir().join("crash-report.txt")
}

#[cfg(target_os = "linux")]
pub fn last_report_file() -> std::path::PathBuf {
    crate::paths::state_dir().join("crash-report.last.txt")
}

#[cfg(target_os = "linux")]
fn pending_report_file() -> std::path::PathBuf {
    crate::paths::state_dir().join("crash-report.tmp")
}

#[cfg(target_os = "linux")]
pub fn install() {
    use std::os::unix::ffi::OsStrExt;

    report_previous_crash();
    let pending = pending_report_file();
    let finished = report_file();
    if let Some(parent) = pending.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (Ok(pending_c), Ok(finished_c)) = (
        std::ffi::CString::new(pending.as_os_str().as_bytes()),
        std::ffi::CString::new(finished.as_os_str().as_bytes()),
    ) else {
        return;
    };
    let fd = unsafe {
        libc::open(
            pending_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        crate::install::log_line("crash handler: could not open the crash report file");
        return;
    }
    if REPORT_PATHS.set((pending_c, finished_c)).is_err() {
        unsafe { libc::close(fd) };
        return;
    }
    REPORT_FD.store(fd, Ordering::SeqCst);

    #[cfg(target_env = "gnu")]
    unsafe {
        let mut frames = [std::ptr::null_mut(); 4];
        libc::backtrace(frames.as_mut_ptr(), frames.len() as libc::c_int);
    }

    let mut previous = Vec::with_capacity(CRASH_SIGNALS.len());
    for signal in CRASH_SIGNALS {
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        unsafe { libc::sigaction(signal, std::ptr::null(), &mut old) };
        previous.push(old);
    }
    if PREVIOUS_ACTIONS.set(previous).is_err() {
        return;
    }
    for signal in CRASH_SIGNALS {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = signal_handler as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
    crate::install::log_line(&format!(
        "crash handler: signal handler installed (report -> {})",
        report_file().display()
    ));
}

#[cfg(target_os = "linux")]
fn report_previous_crash() {
    let path = report_file();
    let Ok(report) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::rename(&path, last_report_file());
    let report = report.trim_end();
    if report.is_empty() {
        return;
    }
    crate::install::log_line(&format!("crash handler: the previous run crashed\n{report}"));
    let summary = report.lines().next().unwrap_or_default().to_string();
    sentry::configure_scope(|scope| scope.set_extra("report", report.into()));
    crate::sentry_telemetry::capture_native_crash(summary);
}

#[cfg(target_os = "linux")]
fn signal_name(signal: libc::c_int) -> &'static [u8] {
    match signal {
        libc::SIGSEGV => b"SIGSEGV",
        libc::SIGBUS => b"SIGBUS",
        libc::SIGABRT => b"SIGABRT",
        libc::SIGILL => b"SIGILL",
        libc::SIGFPE => b"SIGFPE",
        _ => b"signal",
    }
}

#[cfg(target_os = "linux")]
fn write_all(fd: libc::c_int, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
        if n <= 0 {
            return;
        }
        rest = &rest[n as usize..];
    }
}

#[cfg(target_os = "linux")]
fn write_decimal(fd: libc::c_int, value: i64) {
    let mut buf = [0u8; 24];
    let mut at = buf.len();
    let negative = value < 0;
    let mut rest = value.unsigned_abs();
    loop {
        at -= 1;
        buf[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if negative {
        at -= 1;
        buf[at] = b'-';
    }
    write_all(fd, &buf[at..]);
}

#[cfg(target_os = "linux")]
fn write_hex(fd: libc::c_int, value: usize) {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for index in 0..16 {
        let nibble = (value >> ((15 - index) * 4)) & 0xf;
        buf[2 + index] = b"0123456789abcdef"[nibble];
    }
    write_all(fd, &buf);
}

#[cfg(target_os = "linux")]
fn write_summary(fd: libc::c_int, signal: libc::c_int, info: *const libc::siginfo_t) {
    let code = if info.is_null() {
        0
    } else {
        unsafe { (*info).si_code }
    };
    write_all(fd, b"SERVICE SIGNAL: ");
    write_all(fd, signal_name(signal));
    write_all(fd, b" (code ");
    write_decimal(fd, code as i64);
    write_all(fd, b")");
    if !info.is_null() && matches!(signal, libc::SIGSEGV | libc::SIGBUS) && code > 0 {
        write_all(fd, b" at ");
        write_hex(fd, unsafe { (*info).si_addr() } as usize);
    }
    write_all(fd, b" tid=");
    write_decimal(fd, unsafe { libc::gettid() } as i64);
    write_all(fd, b"\n");
}

#[cfg(target_os = "linux")]
extern "C" fn signal_handler(
    signal: libc::c_int,
    info: *mut libc::siginfo_t,
    _context: *mut libc::c_void,
) {
    let tid = unsafe { libc::gettid() };
    let sent = info.is_null() || unsafe { (*info).si_code } <= 0;
    if let Err(owner) = CRASH_TID.compare_exchange(0, tid, Ordering::SeqCst, Ordering::SeqCst) {
        if owner != tid {
            loop {
                unsafe { libc::pause() };
            }
        }
        unsafe { libc::signal(signal, libc::SIG_DFL) };
        if sent {
            unsafe { libc::raise(signal) };
        }
        return;
    }
    unsafe { libc::alarm(5) };
    if !HANDLING.swap(true, Ordering::SeqCst) {
        write_summary(libc::STDERR_FILENO, signal, info);
        let fd = REPORT_FD.swap(-1, Ordering::SeqCst);
        if fd >= 0 {
            write_summary(fd, signal, info);
            write_all(fd, b"version ");
            write_all(fd, env!("CARGO_PKG_VERSION").as_bytes());
            write_all(fd, b"\n");
            #[cfg(target_env = "gnu")]
            unsafe {
                let mut frames = [std::ptr::null_mut(); 64];
                let count = libc::backtrace(frames.as_mut_ptr(), frames.len() as libc::c_int);
                write_all(fd, b"backtrace:\n");
                libc::backtrace_symbols_fd(frames.as_ptr(), count, fd);
            }
            unsafe {
                libc::fsync(fd);
                libc::close(fd);
            }
            if let Some((pending, finished)) = REPORT_PATHS.get() {
                unsafe { libc::rename(pending.as_ptr(), finished.as_ptr()) };
            }
        }
    }
    restore_previous_action(signal);
    if sent {
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }
}

#[cfg(target_os = "linux")]
fn restore_previous_action(signal: libc::c_int) {
    let index = CRASH_SIGNALS.iter().position(|candidate| *candidate == signal);
    match (index, PREVIOUS_ACTIONS.get()) {
        (Some(index), Some(previous)) => unsafe {
            libc::sigaction(signal, &previous[index], std::ptr::null_mut());
        },
        _ => unsafe {
            libc::signal(signal, libc::SIG_DFL);
        },
    }
}
