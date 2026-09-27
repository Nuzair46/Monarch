#![cfg(windows)]
use std::mem::size_of;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub fn user_session() -> Result<(String, u32), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| e.to_string())?;
        let result = (|| -> windows::core::Result<String> {
            let mut required = 0;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut required);
            let mut buffer = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                required,
                &mut required,
            )?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
            let text = sid.to_string();
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
            Ok(text?)
        })();
        let _ = CloseHandle(token);
        let sid = result.map_err(|e| e.to_string())?;
        let mut session = 0;
        ProcessIdToSessionId(std::process::id(), &mut session).map_err(|e| e.to_string())?;
        Ok((sid, session))
    }
}
