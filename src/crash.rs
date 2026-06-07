//! Top-level SEH handler for the service worker.
//!
//! The worker dies on native access violations (`0xC0000005`) that the Rust
//! panic hook cannot see — `std::panic` only catches Rust panics, not Win32
//! structured exceptions. This installs `SetUnhandledExceptionFilter` so a crash
//! logs the exception code + faulting address + owning module (as `module+rva`)
//! to the service log AND writes a full minidump to `C:\ProgramData\WarmupVk`
//! for postmortem in WinDbg / Visual Studio.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWithFullMemory, MiniDumpWithHandleData, MiniDumpWithThreadInfo, MiniDumpWriteDump,
    SetUnhandledExceptionFilter, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

const GENERIC_WRITE: u32 = 0x4000_0000;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
const EXCEPTION_EXECUTE_HANDLER: i32 = 1;

/// Re-entrancy guard: a fault while handling a fault must not loop.
static HANDLING: AtomicBool = AtomicBool::new(false);

/// Install the top-level exception filter. Call once at worker start.
pub fn install() {
    unsafe {
        SetUnhandledExceptionFilter(Some(seh_filter));
    }
    crate::install::log_line("crash handler: SEH filter installed (minidump on AV)");
}

unsafe fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Resolve `addr` to `module_filename+0xRVA`. Best-effort; falls back to the raw
/// address if the owning module cannot be found.
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

    let mut exc = MINIDUMP_EXCEPTION_INFORMATION {
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
        Some(&mut exc),
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
