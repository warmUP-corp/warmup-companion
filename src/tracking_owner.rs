//! Interactive-user identity for playtime / library-watch scoping (session + SID).

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TrackingOwner {
    pub user_sid: String,
    pub session_id: u32,
}

#[cfg(windows)]
pub(crate) fn tracking_owner_from_process(
    process: windows::Win32::Foundation::HANDLE,
    pid: u32,
) -> Option<TrackingOwner> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::OpenProcessToken;

    let mut session_id = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut session_id) }.ok()?;
    if session_id == 0 {
        return None;
    }

    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let sid = unsafe {
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        if len == 0 {
            None
        } else {
            let mut buf = vec![0u8; len as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            )
            .is_err()
            {
                None
            } else {
                let token_user = std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_USER>());
                let mut sid_string = PWSTR::null();
                if ConvertSidToStringSidW(token_user.User.Sid, &mut sid_string).is_err() {
                    None
                } else {
                    let value = sid_string.to_string().ok();
                    let _ = LocalFree(HLOCAL(sid_string.0.cast()));
                    value
                }
            }
        }
    };
    unsafe {
        let _ = CloseHandle(token);
    }
    let user_sid = sid?;
    if user_sid.eq_ignore_ascii_case("S-1-5-18") {
        return None;
    }
    Some(TrackingOwner {
        user_sid,
        session_id,
    })
}
