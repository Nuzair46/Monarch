use std::sync::Mutex;

use monarch::{DisplayId, FileConfigStore, MonarchDisplayManager};
use tauri::{Manager, WindowEvent};

use crate::app::{commands, events, shortcuts, startup};
use crate::backend::SystemDisplayBackend;

pub type MonarchManager = MonarchDisplayManager<SystemDisplayBackend, FileConfigStore>;

pub struct MonarchRuntimeState {
    pub manager: MonarchManager,
}

pub struct MonarchAppState(pub Mutex<MonarchRuntimeState>);

pub fn run_app() {
    let _single_instance_guard = match crate::app::single_instance::try_acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            if let Some(profile_name) = startup::requested_profile_name() {
                if let Err(err) = crate::app::ipc::send_apply_profile_request(&profile_name) {
                    eprintln!("Monarch is already running and IPC profile apply failed: {err}");
                }
            } else {
                if let Err(err) = crate::app::ipc::send_show_main_window_request() {
                    eprintln!("Monarch is already running and IPC show-main failed: {err}");
                }
            }
            return;
        }
        Err(err) => {
            eprintln!("Monarch single-instance check failed: {err}");
            return;
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let backend = SystemDisplayBackend::new().map_err(|err| err.to_string())?;
            let store = FileConfigStore::default();
            let mut manager =
                MonarchDisplayManager::new(backend, store).map_err(|err| err.to_string())?;
            let should_start_hidden = startup::should_start_hidden();
            let requested_profile_name = startup::requested_profile_name();
            let startup_profile_name = requested_profile_name
                .or_else(|| manager.settings().startup_profile_name.clone());

            if let Some(profile_name) = startup_profile_name {
                match manager.apply_profile(&profile_name) {
                    Ok(()) => {
                        if manager.has_pending_confirmation() {
                            if let Err(err) = manager.confirm_current_layout() {
                                eprintln!(
                                    "Monarch launch profile confirm failed for '{profile_name}': {err}"
                                );
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("Monarch launch profile apply failed for '{profile_name}': {err}");
                    }
                }
            }

            let startup_enabled = manager.settings().start_with_windows;
            let state = MonarchAppState(Mutex::new(MonarchRuntimeState { manager }));
            app.manage(state);

            if let Err(err) = startup::sync_start_with_windows(startup_enabled) {
                eprintln!("Monarch startup task sync failed: {err}");
            }
            if let Err(err) = shortcuts::sync_global_shortcuts(&app.handle()) {
                eprintln!("Monarch global shortcut sync failed: {err}");
            }

            events::build_tray(&app.handle()).map_err(|err| err.to_string())?;
            events::refresh_tray_menu(&app.handle());
            events::spawn_color_state_watchdog(app.handle().clone());
            events::spawn_topology_state_watchdog(app.handle().clone());
            crate::app::ipc::spawn_listener(app.handle().clone());

            if let Some(window) = app.get_webview_window("main") {
                if should_start_hidden {
                    let _ = window.minimize();
                    let _ = window.hide();
                }
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(win) = app_handle.get_webview_window("main") {
                            let _ = win.hide();
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_displays,
            commands::get_layout,
            commands::list_profiles,
            commands::get_snapshot,
            commands::toggle_display,
            commands::apply_layout,
            commands::save_profile,
            commands::apply_profile,
            commands::delete_profile,
            commands::restore_last_layout,
            commands::confirm_current_layout,
            commands::rollback_pending,
            commands::update_settings,
            commands::open_external_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub fn format_display_key(id: &DisplayId) -> String {
    let edid = id
        .edid_hash
        .map(|value| format!("{value:016x}"))
        .unwrap_or_else(|| "-".to_string());
    format!("{:016x}:{}:{edid}", id.adapter_luid, id.target_id)
}

pub fn parse_display_key(key: &str) -> Result<DisplayId, monarch::ManagerError> {
    let mut parts = key.split(':');
    let luid_hex = parts.next().ok_or_else(|| {
        monarch::ManagerError::Validation("invalid display key (adapter_luid)".to_string())
    })?;
    let target = parts.next().ok_or_else(|| {
        monarch::ManagerError::Validation("invalid display key (target_id)".to_string())
    })?;
    let edid = parts.next().unwrap_or("-");

    let adapter_luid = u64::from_str_radix(luid_hex, 16).map_err(|_| {
        monarch::ManagerError::Validation("invalid display key adapter_luid hex".to_string())
    })?;
    let target_id = target.parse::<u32>().map_err(|_| {
        monarch::ManagerError::Validation("invalid display key target_id".to_string())
    })?;
    let edid_hash = if edid == "-" {
        None
    } else {
        Some(u64::from_str_radix(edid, 16).map_err(|_| {
            monarch::ManagerError::Validation("invalid display key edid hash".to_string())
        })?)
    };

    Ok(DisplayId {
        adapter_luid,
        target_id,
        edid_hash,
    })
}
