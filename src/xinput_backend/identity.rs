//! Process/token identity probe for secure-poll diagnostics.

use std::mem::size_of;

pub(crate) fn probe_self_identity() -> String {
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
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
    };

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
