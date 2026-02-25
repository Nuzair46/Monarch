#[cfg(target_os = "windows")]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;

    pub struct SingleInstanceGuard(HANDLE);

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    pub fn try_acquire() -> Result<Option<SingleInstanceGuard>, String> {
        unsafe {
            let handle = CreateMutexW(None, false, w!("Local\\MonarchSingleInstance"))
                .map_err(|err| format!("CreateMutexW failed: {err}"))?;

            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }

            Ok(Some(SingleInstanceGuard(handle)))
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub struct SingleInstanceGuard;

    pub fn try_acquire() -> Result<Option<SingleInstanceGuard>, String> {
        Ok(Some(SingleInstanceGuard))
    }
}

pub use imp::{try_acquire, SingleInstanceGuard};
