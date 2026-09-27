use monarch::{FileConfigStore, MonarchDisplayManager};
use tauri::{Manager, WindowEvent};

use crate::app::{commands, events, shortcuts, startup};
use crate::backend::SystemDisplayBackend;

pub type MonarchManager = MonarchDisplayManager<SystemDisplayBackend, FileConfigStore>;

pub struct MonarchAppState {
    pub controller: super::coordinator::Controller,
}

pub fn run_app() {
    let _single_instance_guard = match crate::app::single_instance::try_acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            if let Some(profile_name) = startup::requested_profile_name() {
                if let Err(err) = crate::app::ipc::send_apply_profile_request(&profile_name) {
                    eprintln!("Monarch is already running and IPC profile apply failed: {err}");
                }
            } else if let Err(err) = crate::app::ipc::send_show_main_window_request() {
                eprintln!("Monarch is already running and IPC show-main failed: {err}");
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
            let manager =
                MonarchDisplayManager::new(backend, store).map_err(|err| err.to_string())?;
            let should_start_hidden = startup::should_start_hidden();
            // CLI argument wins over the saved startup profile setting.
            let requested_profile_name = startup::requested_profile_name();
            let startup_profile_name =
                requested_profile_name.or_else(|| manager.settings().startup_profile_name.clone());

            let startup_enabled = manager.settings().start_with_windows;
            let controller = super::coordinator::Controller::start(app.handle().clone(), manager)?;
            let state = MonarchAppState { controller };
            app.manage(state);

            if let Err(err) = startup::sync_start_with_windows(startup_enabled) {
                eprintln!("Monarch startup task sync failed: {err}");
            }
            if let Err(err) = shortcuts::sync_global_shortcuts(app.handle()) {
                eprintln!("Monarch global shortcut sync failed: {err}");
            }

            events::build_tray(app.handle()).map_err(|err| err.to_string())?;
            events::refresh_tray_menu(app.handle());
            events::spawn_system_event_listener(app.handle().clone());
            crate::app::ipc::spawn_listener(app.handle().clone());

            // Apply the startup profile on a worker thread AFTER tray/IPC exist: a wedged apply
            // at login must never leave an invisible, unkillable app. The external-action helper
            // keeps the auto-confirm semantics the old synchronous path had.
            if let Some(profile_name) = startup_profile_name {
                crate::diagnostics::log(format!("startup_profile:queued:{profile_name}"));
                events::handle_profile_apply_external_action(app.handle(), &profile_name);
            }

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
            commands::get_snapshot,
            commands::get_display_capabilities,
            commands::toggle_display,
            commands::apply_layout,
            commands::save_profile,
            commands::set_profile_audio,
            commands::apply_profile,
            commands::delete_profile,
            commands::restore_last_layout,
            commands::confirm_current_layout,
            commands::rollback_pending,
            commands::update_settings,
            commands::open_external_url,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_, _| {});
}

pub use monarch::identity::{display_key as format_display_key, parse_display_key};
