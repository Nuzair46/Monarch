use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{MenuBuilder, SubmenuBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

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
    RollbackFailed { message: String },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationRevertReason {
    Manual,
    Timeout,
}

pub fn emit_state_changed<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit(EVENT_STATE_CHANGED, ());
}

pub fn emit_confirmation<R: Runtime>(app: &AppHandle<R>, payload: ConfirmationEvent) {
    let _ = app.emit(EVENT_CONFIRMATION, payload);
}

pub(super) fn should_auto_reapply_calibration(
    previous: Option<&String>,
    current: Option<&String>,
) -> bool {
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

fn tray_menu_refresh_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn submit<R: Runtime>(app: &AppHandle<R>, operation: super::coordinator::Operation) {
    let controller = app.state::<MonarchAppState>().controller.clone();
    match controller.submit(operation) {
        Ok(result) => {
            tauri::async_runtime::spawn(async move {
                if let Ok(Err(error)) = result.await {
                    diagnostics::log(format!("external_action:failed:{error}"));
                }
            });
        }
        Err(error) => diagnostics::log(error),
    }
}

pub fn handle_profile_apply_external_action<R: Runtime>(app: &AppHandle<R>, name: &str) {
    submit(
        app,
        super::coordinator::Operation::ApplyProfile(name.to_string(), true),
    );
}
pub fn handle_toggle_display_external_action<R: Runtime>(app: &AppHandle<R>, key: &str) {
    submit(
        app,
        super::coordinator::Operation::Toggle(key.to_string(), true),
    );
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
    let snapshot = state.controller.snapshot()?;
    let profiles = snapshot.profiles.into_iter().map(|p| p.name).collect();
    let displays = snapshot
        .displays
        .into_iter()
        .map(|d| TrayMenuDisplay {
            id_key: d.id_key,
            friendly_name: d.friendly_name,
            is_active: d.is_active,
        })
        .collect();

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
    submit(app, super::coordinator::Operation::Restore);
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
        app.state::<MonarchAppState>().controller.refresh(resume);
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
