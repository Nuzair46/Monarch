#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Output};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const START_HIDDEN_ARG: &str = "--start-hidden";
    const RUN_KEY_PATH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_KEY_VALUE_NAME: &str = "Monarch";
    const START_DELAY_SECONDS: u64 = 10;

    pub fn should_start_hidden() -> bool {
        std::env::args_os().any(|arg| arg == START_HIDDEN_ARG)
    }

    pub fn sync_start_with_windows(enabled: bool) -> Result<(), String> {
        if enabled {
            create_run_key_entry()
        } else {
            delete_run_key_entry()
        }
    }

    fn create_run_key_entry() -> Result<(), String> {
        let exe_path = std::env::current_exe()
            .map_err(|err| format!("could not resolve current executable path: {err}"))?;
        let exe_for_ps = powershell_single_quoted(&exe_path.to_string_lossy());
        let launch_command = format!(
            "powershell.exe -NoProfile -WindowStyle Hidden -Command \"Start-Sleep -Seconds {START_DELAY_SECONDS}; & '{exe_for_ps}' {START_HIDDEN_ARG}\""
        );

        run_reg(&[
            "add".to_string(),
            RUN_KEY_PATH.to_string(),
            "/v".to_string(),
            RUN_KEY_VALUE_NAME.to_string(),
            "/t".to_string(),
            "REG_SZ".to_string(),
            "/d".to_string(),
            launch_command,
            "/f".to_string(),
        ])
        .map(|_| ())
    }

    fn delete_run_key_entry() -> Result<(), String> {
        match run_reg(&[
            "delete".to_string(),
            RUN_KEY_PATH.to_string(),
            "/v".to_string(),
            RUN_KEY_VALUE_NAME.to_string(),
            "/f".to_string(),
        ]) {
            Ok(_) => Ok(()),
            Err(err) if is_registry_value_not_found_error(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn run_reg(args: &[String]) -> Result<Output, String> {
        let output = Command::new("reg.exe")
            .creation_flags(CREATE_NO_WINDOW)
            .args(args)
            .output()
            .map_err(|err| format!("failed to run reg.exe: {err}"))?;

        if output.status.success() {
            Ok(output)
        } else {
            Err(render_failure("reg.exe", &output))
        }
    }

    fn render_failure(binary: &str, output: &Output) -> String {
        let mut rendered = String::new();
        if !output.stdout.is_empty() {
            rendered.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !rendered.is_empty() {
                rendered.push(' ');
            }
            rendered.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        let rendered = rendered.trim();

        if rendered.is_empty() {
            format!("{binary} failed with exit code {:?}", output.status.code())
        } else {
            format!(
                "{binary} failed with exit code {:?}: {rendered}",
                output.status.code()
            )
        }
    }

    fn is_registry_value_not_found_error(message: &str) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains("unable to find") || lower.contains("cannot find")
    }

    fn powershell_single_quoted(value: &str) -> String {
        value.replace('\'', "''")
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub fn should_start_hidden() -> bool {
        false
    }

    pub fn sync_start_with_windows(_enabled: bool) -> Result<(), String> {
        Ok(())
    }
}

pub use imp::{should_start_hidden, sync_start_with_windows};
