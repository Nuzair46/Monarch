use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{MenuBuilder, SubmenuBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::app::shortcuts;
use crate::app::state::MonarchAppState;
use crate::diagnostics;

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
                diagnostics::log("confirm_watchdog:rolled_back:timeout");
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
            Err(err) => {
                drop(guard);
                diagnostics::log(format!("confirm_watchdog:rollback_error:{err}"));
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

            let signature = match topology_watch_signature(&app) {
                Ok(signature) => signature,
                Err(_) => continue,
            };

            match &last_signature {
                None => {
                    last_signature = Some(signature);
                }
                Some(previous) if previous == &signature => {}
                Some(_) => {
                    last_signature = Some(signature);
                    let _ = shortcuts::sync_global_shortcuts(&app);
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

fn topology_watch_signature<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let state = app.state::<MonarchAppState>();
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    let displays = guard
        .manager
        .list_displays()
        .map_err(|err| err.to_string())?;

    let mut entries = displays
        .iter()
        .map(|display| {
            (
                display.id.adapter_luid,
                display.id.target_id,
                display.id.edid_hash,
                display.is_active,
                display.is_primary,
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut parts = Vec::with_capacity(displays.len());
    for (adapter_luid, target_id, edid_hash, is_active, is_primary) in entries {
        parts.push(format!(
            "{adapter_luid:016x}:{target_id}:{}:{}:{}",
            edid_hash
                .map(|value| format!("{value:016x}"))
                .unwrap_or_else(|| "-".to_string()),
            if is_active { 1 } else { 0 },
            if is_primary { 1 } else { 0 }
        ));
    }
    Ok(parts.join("|"))
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
                    show_main_window(&app);
                }
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }
    tray_builder.build(app)?;
    Ok(())
}

pub fn refresh_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    let _refresh_guard = match tray_menu_refresh_lock().try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            // Another refresh is in flight. Don't drop this one silently: schedule a single
            // deferred retry so the menu still converges on the latest state.
            diagnostics::log("tray_refresh:skip_busy");
            schedule_tray_refresh_retry(app);
            return;
        }
    };
    tray_refresh_retry_delay_ms().store(TRAY_REFRESH_RETRY_BASE_MS, Ordering::SeqCst);
    let Some(tray) = app.tray_by_id("monarch-tray") else {
        return;
    };
    if let Ok(menu) = build_tray_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
}

const TRAY_REFRESH_RETRY_BASE_MS: u64 = 300;
const TRAY_REFRESH_RETRY_MAX_MS: u64 = 5000;

fn tray_refresh_retry_pending() -> &'static AtomicBool {
    static PENDING: OnceLock<AtomicBool> = OnceLock::new();
    PENDING.get_or_init(|| AtomicBool::new(false))
}

fn tray_refresh_retry_delay_ms() -> &'static AtomicU64 {
    static DELAY: OnceLock<AtomicU64> = OnceLock::new();
    DELAY.get_or_init(|| AtomicU64::new(TRAY_REFRESH_RETRY_BASE_MS))
}

fn schedule_tray_refresh_retry<R: Runtime>(app: &AppHandle<R>) {
    if tray_refresh_retry_pending()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    // Exponential backoff (capped) so a long-blocked holder does not produce a thread+log
    // storm every 300ms; the delay resets once a refresh actually gets through.
    let delay_ms = tray_refresh_retry_delay_ms().load(Ordering::SeqCst);
    let next_delay_ms = (delay_ms * 2).min(TRAY_REFRESH_RETRY_MAX_MS);
    tray_refresh_retry_delay_ms().store(next_delay_ms, Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        tray_refresh_retry_pending().store(false, Ordering::SeqCst);
        refresh_tray_menu(&app);
    });
}

fn tray_action_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn tray_menu_refresh_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn spawn_serialized_action<R, F>(app: &AppHandle<R>, name: &'static str, action: F)
where
    R: Runtime,
    F: FnOnce(AppHandle<R>) + Send + 'static,
{
    let app = app.clone();
    std::thread::spawn(move || {
        diagnostics::log(format!("tray_action:start:{name}"));
        let guard = match tray_action_lock().lock() {
            Ok(guard) => guard,
            Err(_) => {
                diagnostics::log(format!("tray_action:lock_poisoned:{name}"));
                return;
            }
        };
        action(app.clone());
        drop(guard);
        diagnostics::log(format!("tray_action:end:{name}"));
    });
}

pub fn apply_profile_external_action_result<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
) -> Result<(), String> {
    let state = app.state::<MonarchAppState>();
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    diagnostics::log(format!("apply_profile:start:{name}"));
    guard
        .manager
        .apply_profile(name)
        .map_err(|err| err.to_string())?;
    let pending_timeout = guard.manager.pending_confirmation_remaining();
    let auto_confirmed =
        pending_timeout.is_some() && guard.manager.confirm_current_layout().is_ok();
    drop(guard);

    let _ = shortcuts::sync_global_shortcuts(app);
    refresh_tray_menu(app);
    emit_state_changed(app);

    if let Some(timeout) = pending_timeout.filter(|_| !auto_confirmed) {
        emit_confirmation(
            app,
            ConfirmationEvent::Applied {
                timeout_ms: timeout.as_millis() as u64,
            },
        );
        spawn_confirmation_watchdog(app.clone(), timeout);
    }
    diagnostics::log(format!(
        "apply_profile:done:{name}:auto_confirm={auto_confirmed}"
    ));
    Ok(())
}

pub fn handle_profile_apply_external_action<R: Runtime>(app: &AppHandle<R>, name: &str) {
    let name = name.to_string();
    spawn_serialized_action(app, "apply_profile", move |app| {
        if let Err(err) = apply_profile_external_action_result(&app, &name) {
            diagnostics::log(format!("apply_profile:error:{name}:{err}"));
        }
    });
}

pub fn handle_toggle_display_external_action<R: Runtime>(app: &AppHandle<R>, display_key: &str) {
    let display_key = display_key.to_string();
    spawn_serialized_action(app, "toggle_display", move |app| {
        toggle_display_external_action_inner(&app, &display_key);
    });
}

fn toggle_display_external_action_inner<R: Runtime>(app: &AppHandle<R>, display_key: &str) {
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
                let _ = shortcuts::sync_global_shortcuts(app);
                refresh_tray_menu(app);
                emit_state_changed(app);

                if !auto_confirmed {
                    emit_confirmation(
                        app,
                        ConfirmationEvent::Applied {
                            timeout_ms: timeout.as_millis() as u64,
                        },
                    );
                    spawn_confirmation_watchdog(app.clone(), timeout);
                }
            } else {
                diagnostics::log(format!(
                    "toggle_display:error:manager_toggle_failed:{display_key}"
                ));
            }
        } else {
            diagnostics::log(format!("toggle_display:error:invalid_key:{display_key}"));
        }
    } else {
        diagnostics::log("toggle_display:error:state_lock_failed");
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

#[derive(Clone)]
struct TrayMenuDisplay {
    id_key: String,
    friendly_name: String,
    is_active: bool,
}

#[derive(Clone)]
struct TrayMenuSnapshot {
    profiles: Vec<String>,
    displays: Vec<TrayMenuDisplay>,
}

fn tray_menu_snapshot<R: Runtime>(app: &AppHandle<R>) -> Result<TrayMenuSnapshot, String> {
    let state = app.state::<MonarchAppState>();
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;

    let profiles = guard
        .manager
        .list_profiles()
        .into_iter()
        .map(|profile| profile.name)
        .collect::<Vec<_>>();
    let displays = guard
        .manager
        .list_displays()
        .map_err(|err| err.to_string())?
        .into_iter()
        .map(|display| TrayMenuDisplay {
            id_key: crate::app::state::format_display_key(&display.id),
            friendly_name: display.friendly_name,
            is_active: display.is_active,
        })
        .collect::<Vec<_>>();

    Ok(TrayMenuSnapshot { profiles, displays })
}

fn build_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    let snapshot = tray_menu_snapshot(app).ok();

    let mut profiles_menu = SubmenuBuilder::new(app, "Profiles");
    if let Some(snapshot) = &snapshot {
        if snapshot.profiles.is_empty() {
            profiles_menu = profiles_menu.text("profiles.none", "(No Profiles)");
        } else {
            for profile in &snapshot.profiles {
                profiles_menu = profiles_menu.text(format!("profile::{profile}"), profile.clone());
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

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn handle_tray_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "open_main" => {
            show_main_window(app);
        }
        "quit_app" => {
            app.exit(0);
        }
        "restore_last_layout" => {
            handle_restore_last_layout(app);
        }
        id if id.strip_prefix("profile::").is_some() => {
            if let Some(name) = id.strip_prefix("profile::") {
                handle_profile_apply_external_action(app, name);
            }
        }
        id if id.strip_prefix("toggle::").is_some() => {
            if let Some(display_key) = id.strip_prefix("toggle::") {
                handle_toggle_display_external_action(app, display_key);
            }
        }
        _ => {}
    }
}

fn handle_restore_last_layout<R: Runtime>(app: &AppHandle<R>) {
    spawn_serialized_action(app, "restore_last_layout", move |app| {
        let state = app.state::<MonarchAppState>();
        let lock = state.0.lock();
        if let Ok(mut guard) = lock {
            if guard.manager.restore_last_layout().is_ok() {
                drop(guard);
                refresh_tray_menu(&app);
                emit_state_changed(&app);
            } else {
                diagnostics::log("restore_last_layout:error:manager_restore_failed");
            }
        } else {
            diagnostics::log("restore_last_layout:error:state_lock_failed");
        }
    });
}

/// Subscribe to system power-resume and display-change broadcasts so the backend cache is
/// invalidated and the tray/UI refreshed right after a sleep-wake cycle, instead of waiting for
/// the polling watchdogs to notice (or never noticing a stale cache at all).
pub fn spawn_system_event_listener<R: Runtime>(app: AppHandle<R>) {
    #[cfg(target_os = "windows")]
    system_events::spawn(app);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
    }
}

#[cfg(target_os = "windows")]
mod system_events {
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use tauri::{AppHandle, Manager, Runtime};
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, WM_DISPLAYCHANGE,
        WM_POWERBROADCAST, WNDCLASSW,
    };

    use crate::app::state::MonarchAppState;
    use crate::diagnostics;

    // Let the display stack settle after resume before rebuilding state.
    const RESUME_DEBOUNCE: Duration = Duration::from_millis(2000);
    const DISPLAY_CHANGE_DEBOUNCE: Duration = Duration::from_millis(500);

    #[derive(Clone, Copy, PartialEq)]
    enum SystemEvent {
        Resume,
        DisplayChange,
    }

    fn event_sender_slot() -> &'static OnceLock<Mutex<Sender<SystemEvent>>> {
        static SENDER: OnceLock<Mutex<Sender<SystemEvent>>> = OnceLock::new();
        &SENDER
    }

    pub fn spawn<R: Runtime>(app: AppHandle<R>) {
        let (sender, receiver) = mpsc::channel::<SystemEvent>();
        if event_sender_slot().set(Mutex::new(sender)).is_err() {
            diagnostics::log("system_events:error:already_spawned");
            return;
        }

        std::thread::spawn(run_message_pump);
        std::thread::spawn(move || consume_events(app, receiver));
    }

    fn notify(event: SystemEvent) {
        let Some(sender) = event_sender_slot().get() else {
            return;
        };
        let Ok(sender) = sender.lock() else {
            return;
        };
        let _ = sender.send(event);
    }

    /// Debounce loop: absorb bursts of resume/display-change notifications, then run one refresh
    /// per settled burst. Never touches any lock on the message-pump thread.
    fn consume_events<R: Runtime>(app: AppHandle<R>, receiver: mpsc::Receiver<SystemEvent>) {
        loop {
            let first = match receiver.recv() {
                Ok(event) => event,
                Err(_) => return,
            };
            let mut saw_resume = first == SystemEvent::Resume;

            loop {
                let settle = if saw_resume {
                    RESUME_DEBOUNCE
                } else {
                    DISPLAY_CHANGE_DEBOUNCE
                };
                match receiver.recv_timeout(settle) {
                    Ok(SystemEvent::Resume) => saw_resume = true,
                    Ok(SystemEvent::DisplayChange) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }

            diagnostics::log(format!("system_event:settled:resume={saw_resume}"));
            handle_settled_event(&app, saw_resume);
        }
    }

    fn handle_settled_event<R: Runtime>(app: &AppHandle<R>, resume: bool) {
        let action_name = if resume {
            "system_resume_refresh"
        } else {
            "display_change_refresh"
        };
        super::spawn_serialized_action(app, action_name, move |app| {
            if resume {
                // Only a resume invalidates the backend cache: adapter LUIDs may have changed
                // and transient EDID failures may have polluted it. Plain WM_DISPLAYCHANGE must
                // NOT invalidate, or the detached-display cache would be lost on every apply.
                let state = app.state::<MonarchAppState>();
                match state.0.lock() {
                    Ok(guard) => {
                        if let Err(err) = guard.manager.invalidate_backend_cache() {
                            diagnostics::log(format!("system_resume:invalidate_cache_error:{err}"));
                        }
                    }
                    Err(_) => {
                        diagnostics::log("system_resume:error:state_lock_failed");
                        return;
                    }
                };
            }
            let _ = crate::app::shortcuts::sync_global_shortcuts(&app);
            super::refresh_tray_menu(&app);
            super::emit_state_changed(&app);
        });
    }

    /// NOTE: deliberately NOT a message-only window (HWND_MESSAGE parent): message-only windows
    /// never receive broadcast messages such as WM_POWERBROADCAST/WM_DISPLAYCHANGE. A hidden
    /// top-level window does.
    fn run_message_pump() {
        unsafe {
            let instance = match GetModuleHandleW(None) {
                Ok(instance) => instance,
                Err(err) => {
                    diagnostics::log(format!("system_events:error:get_module_handle:{err}"));
                    return;
                }
            };

            let class_name = w!("MonarchSystemEventWindow");
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(system_event_wndproc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            if RegisterClassW(&window_class) == 0 {
                diagnostics::log("system_events:error:register_class_failed");
                return;
            }

            let window = match CreateWindowExW(
                Default::default(),
                class_name,
                w!("Monarch System Events"),
                Default::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            ) {
                Ok(window) => window,
                Err(err) => {
                    diagnostics::log(format!("system_events:error:create_window:{err}"));
                    return;
                }
            };
            let _ = window;

            diagnostics::log("system_events:listener_started");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            // Covers both WM_QUIT (0) and the -1 error return: if the pump dies, resume
            // protection is off — make that visible in the diagnostics log.
            diagnostics::log("system_events:error:message_pump_exited");
        }
    }

    unsafe extern "system" fn system_event_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_POWERBROADCAST => {
                let power_event = wparam.0 as u32;
                if power_event == PBT_APMRESUMEAUTOMATIC || power_event == PBT_APMRESUMESUSPEND {
                    notify(SystemEvent::Resume);
                }
                LRESULT(1)
            }
            WM_DISPLAYCHANGE => {
                notify(SystemEvent::DisplayChange);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
