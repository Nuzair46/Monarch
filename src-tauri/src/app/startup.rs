#[cfg(target_os = "windows")]
mod imp {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::*;
    const START_HIDDEN_ARG: &str = "--start-hidden";
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub fn should_start_hidden() -> bool {
        std::env::args_os().any(|a| a == START_HIDDEN_ARG)
    }
    pub fn requested_profile_name() -> Option<String> {
        super::parse_profile_name_from_args(
            std::env::args_os()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned()),
        )
    }
    pub fn sync_start_with_windows(enabled: bool) -> Result<(), String> {
        let command = if enabled {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let executable = executable.to_string_lossy().replace('\'', "''");
            Some(format!("powershell.exe -NoProfile -WindowStyle Hidden -Command \"Start-Sleep -Seconds 10; & '{executable}' --start-hidden\""))
        } else {
            None
        };
        update_registration(
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            command.as_deref(),
        )
    }

    fn update_registration(path: PCWSTR, command: Option<&str>) -> Result<(), String> {
        unsafe {
            let mut handle = HKEY::default();
            let status = if command.is_some() {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path,
                    None,
                    None,
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    None,
                    &mut handle,
                    None,
                )
            } else {
                RegOpenKeyExW(HKEY_CURRENT_USER, path, None, KEY_SET_VALUE, &mut handle)
            };
            if command.is_none() && status == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            status
                .ok()
                .map_err(|e| format!("cannot open startup registry key: {e}"))?;
            let key = Key(handle);
            let status = if let Some(command) = command {
                let bytes: Vec<u8> = command
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect();
                RegSetValueExW(key.0, w!("Monarch"), None, REG_SZ, Some(&bytes))
            } else {
                RegDeleteValueW(key.0, w!("Monarch"))
            };
            if status == ERROR_SUCCESS || (command.is_none() && status == ERROR_FILE_NOT_FOUND) {
                Ok(())
            } else {
                Err(format!(
                    "cannot update startup registry value (Windows error {})",
                    status.0
                ))
            }
        }
    }

    #[cfg(test)]
    mod registry_tests {
        use super::*;
        // Isolated fixture key: this never changes the user's actual Run registration.
        struct Fixture(Vec<u16>);
        impl Drop for Fixture {
            fn drop(&mut self) {
                unsafe {
                    let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(self.0.as_ptr()));
                }
            }
        }

        #[test]
        fn deleting_missing_startup_value_is_idempotent_in_every_windows_locale() {
            let path = format!("Software\\Monarch\\Tests\\startup-{}", std::process::id());
            let fixture = Fixture(path.encode_utf16().chain(Some(0)).collect());
            let path = PCWSTR(fixture.0.as_ptr());
            update_registration(path, None).unwrap();
            update_registration(path, Some("пример.exe --start-hidden")).unwrap();
            update_registration(path, None).unwrap();
            update_registration(path, None).unwrap();
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub fn should_start_hidden() -> bool {
        false
    }

    pub fn requested_profile_name() -> Option<String> {
        super::parse_profile_name_from_args(std::env::args().skip(1))
    }

    pub fn sync_start_with_windows(_enabled: bool) -> Result<(), String> {
        Ok(())
    }
}

pub use imp::{requested_profile_name, should_start_hidden, sync_start_with_windows};

fn parse_profile_name_from_args<I>(args: I) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_profile_flag(trimmed) {
            let next = args.next()?;
            let profile_name = next.trim();
            if profile_name.is_empty() {
                return None;
            }
            return Some(profile_name.to_string());
        }

        if let Some(profile_name) = parse_profile_equals_flag(trimmed) {
            return Some(profile_name);
        }
    }

    None
}

fn is_profile_flag(value: &str) -> bool {
    value.eq_ignore_ascii_case("-profile")
        || value.eq_ignore_ascii_case("--profile")
        || value.eq_ignore_ascii_case("/profile")
}

fn parse_profile_equals_flag(value: &str) -> Option<String> {
    if !value.to_ascii_lowercase().starts_with("--profile=")
        && !value.to_ascii_lowercase().starts_with("-profile=")
        && !value.to_ascii_lowercase().starts_with("/profile=")
    {
        return None;
    }
    let (_, profile_name) = value.split_once('=')?;
    let profile_name = profile_name.trim();
    if profile_name.is_empty() {
        return None;
    }
    Some(profile_name.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_profile_name_from_args;

    #[test]
    fn parses_profile_from_short_flag() {
        let args = vec!["-profile".to_string(), "Game Mode".to_string()];
        assert_eq!(
            parse_profile_name_from_args(args),
            Some("Game Mode".to_string())
        );
    }

    #[test]
    fn parses_profile_from_long_equals_flag() {
        let args = vec!["--profile=Work".to_string()];
        assert_eq!(parse_profile_name_from_args(args), Some("Work".to_string()));
    }

    #[test]
    fn returns_none_when_profile_flag_missing_value() {
        let args = vec!["-profile".to_string()];
        assert_eq!(parse_profile_name_from_args(args), None);
    }
}
