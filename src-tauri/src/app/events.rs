use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{MenuBuilder, SubmenuBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::app::commands::{snapshot_from_manager, AppSnapshotDto};
use crate::app::state::MonarchAppState;

pub const EVENT_STATE_CHANGED: &str = "monarch://state-changed";
pub const EVENT_CONFIRMATION: &str = "monarch://confirmation";

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfirmationEvent {
    Applied { timeout_ms: u64 },
    Confirmed,
    Reverted { reason: ConfirmationRevertReason },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationRevertReason {
    Manual,
    Timeout,
    Error,
}

pub fn emit_state_changed<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit(EVENT_STATE_CHANGED, ());
}

pub fn emit_confirmation<R: Runtime>(app: &AppHandle<R>, payload: ConfirmationEvent) {
    let _ = app.emit(EVENT_CONFIRMATION, payload);
}

pub fn spawn_confirmation_watchdog<R: Runtime>(app: AppHandle<R>, timeout: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        let state = app.state::<MonarchAppState>();
        let mut guard = match state.0.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        match guard.manager.rollback_if_confirmation_expired() {
            Ok(true) => {
                drop(guard);
                refresh_tray_menu(&app);
                emit_state_changed(&app);
                emit_confirmation(
                    &app,
                    ConfirmationEvent::Reverted {
                        reason: ConfirmationRevertReason::Timeout,
                    },
                );
            }
            Ok(false) => {}
            Err(_) => {
                drop(guard);
                emit_confirmation(
                    &app,
                    ConfirmationEvent::Reverted {
                        reason: ConfirmationRevertReason::Error,
                    },
                );
            }
        }
    });
}

pub fn spawn_color_state_watchdog<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let mut last_signature: Option<Option<String>> = None;

        loop {
            std::thread::sleep(Duration::from_millis(1200));

            let state = app.state::<MonarchAppState>();
            let mut emit_refresh = false;
            let mut consume_change = true;
            let next_signature = {
                let guard = match state.0.lock() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };

                let current_signature = match guard.manager.color_state_signature() {
                    Ok(signature) => signature,
                    Err(_) => continue,
                };

                if let Some(previous_signature) = &last_signature {
                    let changed = *previous_signature != current_signature;
                    if changed {
                        let should_auto_reapply = should_auto_reapply_calibration(
                            previous_signature.as_ref(),
                            current_signature.as_ref(),
                        );

                        if should_auto_reapply {
                            if !guard.manager.has_pending_confirmation() {
                                if guard.manager.reapply_color_calibration().is_ok() {
                                    emit_refresh = true;
                                } else {
                                    // Keep the previous signature so we retry on the next poll.
                                    consume_change = false;
                                }
                            } else {
                                // Defer consumption while a confirmation rollback is pending so a
                                // later poll can still react to the HDR/color-state change.
                                consume_change = false;
                            }
                        }
                    }
                }

                current_signature
            };

            if consume_change {
                last_signature = Some(next_signature);
            }

            if emit_refresh {
                emit_state_changed(&app);
            }
        }
    });
}

pub fn spawn_topology_state_watchdog<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let mut last_signature: Option<String> = None;

        loop {
            std::thread::sleep(Duration::from_millis(1800));

            let snapshot = match state_snapshot(&app) {
                Ok(snapshot) => snapshot,
                Err(_) => continue,
            };
            let signature = topology_signature(&snapshot);

            match &last_signature {
                None => {
                    last_signature = Some(signature);
                }
                Some(previous) if previous == &signature => {}
                Some(_) => {
                    last_signature = Some(signature);
                    refresh_tray_menu(&app);
                    emit_state_changed(&app);
                }
            }
        }
    });
}

fn should_auto_reapply_calibration(previous: Option<&String>, current: Option<&String>) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };

    let Some(previous_map) = parse_color_state_signature(previous) else {
        return false;
    };
    let Some(current_map) = parse_color_state_signature(current) else {
        return false;
    };

    // Only auto-reapply on HDR/advanced-color flag transitions for the same active display set.
    // Topology changes (detach/attach) already run their own post-apply calibration handling and
    // should not trigger this watcher path.
    if previous_map.len() != current_map.len() {
        return false;
    }
    if !previous_map.keys().all(|key| current_map.contains_key(key)) {
        return false;
    }

    previous_map.iter().any(|(key, previous_flag)| {
        let Some(current_flag) = current_map.get(key) else {
            return false;
        };
        *current_flag == '0' && *current_flag != *previous_flag
    })
}

fn parse_color_state_signature(signature: &str) -> Option<HashMap<String, char>> {
    let mut map = HashMap::new();
    if signature.is_empty() {
        return Some(map);
    }

    for entry in signature.split(';') {
        if entry.is_empty() {
            continue;
        }

        let mut parts = entry.split(':');
        let adapter_luid = parts.next()?;
        let target_id = parts.next()?;
        let flag_str = parts.next()?;
        if parts.next().is_some() {
            return None;
        }

        let mut chars = flag_str.chars();
        let flag = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        if !matches!(flag, '0' | '1' | 'x') {
            return None;
        }

        map.insert(format!("{adapter_luid}:{target_id}"), flag);
    }

    Some(map)
}

fn topology_signature(snapshot: &AppSnapshotDto) -> String {
    let mut displays = snapshot
        .displays
        .iter()
        .map(|display| {
            (
                display.id_key.as_str(),
                display.is_active,
                display.is_primary,
                display.resolution.width,
                display.resolution.height,
                display.refresh_rate_mhz,
            )
        })
        .collect::<Vec<_>>();
    displays.sort_by(|a, b| a.0.cmp(b.0));

    let mut parts = Vec::with_capacity(displays.len());
    for (id_key, is_active, is_primary, width, height, refresh_rate_mhz) in displays {
        parts.push(format!(
            "{id_key}:{}:{}:{width}x{height}:{refresh_rate_mhz}",
            if is_active { 1 } else { 0 },
            if is_primary { 1 } else { 0 }
        ));
    }
    parts.join("|")
}

pub fn build_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;
    let mut tray_builder = TrayIconBuilder::with_id("monarch-tray")
        .tooltip("Monarch")
        .menu(&menu)
        .on_menu_event({
            let app = app.clone();
            move |_tray, event| {
                handle_tray_menu_event(&app, event.id().as_ref());
            }
        })
        .on_tray_icon_event({
            let app = app.clone();
            move |_tray, event| {
                if let TrayIconEvent::DoubleClick { .. } = event {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        })
        ;
    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }
    tray_builder.build(app)?;
    Ok(())
}

pub fn refresh_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id("monarch-tray") else {
        return;
    };
    if let Ok(menu) = build_tray_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
}

pub fn spawn_deferred_tray_refresh<R: Runtime>(app: AppHandle<R>, delay: Duration) {
    std::thread::spawn(move || {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        refresh_tray_menu(&app);
    });
}

fn build_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    let snapshot = state_snapshot(app).ok();

    let mut profiles_menu = SubmenuBuilder::new(app, "Profiles");
    if let Some(snapshot) = &snapshot {
        if snapshot.profiles.is_empty() {
            profiles_menu = profiles_menu.text("profiles.none", "(No Profiles)");
        } else {
            for profile in &snapshot.profiles {
                profiles_menu =
                    profiles_menu.text(format!("profile::{}", profile.name), profile.name.clone());
            }
        }
    }

    let mut toggles_menu = SubmenuBuilder::new(app, "Toggle Monitor");
    if let Some(snapshot) = &snapshot {
        for display in &snapshot.displays {
            let label = if display.is_active {
                format!("Detach {}", display.friendly_name)
            } else {
                format!("Attach {}", display.friendly_name)
            };
            toggles_menu = toggles_menu.text(format!("toggle::{}", display.id_key), label);
        }
    }

    let menu = MenuBuilder::new(app)
        .item(&profiles_menu.build()?)
        .item(&toggles_menu.build()?)
        .separator()
        .text("restore_last_layout", "Restore Displays")
        .text("open_main", "Open App")
        .separator()
        .text("quit_app", "Quit")
        .build()?;

    Ok(menu)
}

fn state_snapshot<R: Runtime>(app: &AppHandle<R>) -> Result<AppSnapshotDto, String> {
    let state = app.state::<MonarchAppState>();
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    snapshot_from_manager(&guard.manager).map_err(|err| err.to_string())
}

fn handle_tray_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "open_main" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "quit_app" => {
            app.exit(0);
        }
        "restore_last_layout" => {
            handle_restore_last_layout(app);
        }
        id if id.strip_prefix("profile::").is_some() => {
            if let Some(name) = id.strip_prefix("profile::") {
                let state = app.state::<MonarchAppState>();
                let lock = state.0.lock();
                if let Ok(mut guard) = lock {
                    if guard.manager.apply_profile(name).is_ok() {
                        let pending_timeout = guard.manager.pending_confirmation_remaining();
                        let auto_confirmed = pending_timeout.is_some()
                            && guard.manager.confirm_current_layout().is_ok();
                        drop(guard);
                        refresh_tray_menu(app);
                        emit_state_changed(app);

                        // Tray actions are intended to be one-click operations and should not
                        // require confirming in the window UI. Fall back to the standard pending
                        // confirmation flow only if the immediate confirm fails unexpectedly.
                        if let Some(timeout) = pending_timeout.filter(|_| !auto_confirmed) {
                            emit_confirmation(
                                app,
                                ConfirmationEvent::Applied {
                                    timeout_ms: timeout.as_millis() as u64,
                                },
                            );
                            spawn_confirmation_watchdog(app.clone(), timeout);
                        }
                    }
                }
            }
        }
        id if id.strip_prefix("toggle::").is_some() => {
            if let Some(display_key) = id.strip_prefix("toggle::") {
                let state = app.state::<MonarchAppState>();
                let lock = state.0.lock();
                if let Ok(mut guard) = lock {
                    if let Ok(display_id) = crate::app::state::parse_display_key(display_key) {
                        if guard.manager.toggle_display(&display_id).is_ok() {
                            let timeout = guard
                                .manager
                                .pending_confirmation_remaining()
                                .unwrap_or_else(|| Duration::from_secs(10));
                            let auto_confirmed = guard.manager.confirm_current_layout().is_ok();
                            drop(guard);
                            refresh_tray_menu(app);
                            emit_state_changed(app);

                            // Tray actions are intended to be one-click operations and should not
                            // require confirming in the window UI. Fall back to the standard pending
                            // confirmation flow only if the immediate confirm fails unexpectedly.
                            if !auto_confirmed {
                                emit_confirmation(
                                    app,
                                    ConfirmationEvent::Applied {
                                        timeout_ms: timeout.as_millis() as u64,
                                    },
                                );
                                spawn_confirmation_watchdog(app.clone(), timeout);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn handle_restore_last_layout<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<MonarchAppState>();
    let lock = state.0.lock();
    if let Ok(mut guard) = lock {
        if guard.manager.restore_last_layout().is_ok() {
            drop(guard);
            refresh_tray_menu(app);
            emit_state_changed(app);
        }
    }
}
