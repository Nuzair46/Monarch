use std::collections::HashMap;
use std::sync::Mutex;

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
    let snapshot = tray_menu_snapshot(app).ok();
    let menu = build_tray_menu(app, snapshot.as_ref())?;
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
    app.manage(TrayMenuState {
        installed: Mutex::new(snapshot),
    });
    Ok(())
}

pub fn refresh_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    // Serialize native menu work on the UI thread. Read the latest snapshot there so
    // queued refreshes converge without replacing the same menu repeatedly.
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || refresh_tray_menu_on_main_thread(&handle)) {
        diagnostics::log(format!("tray_refresh:dispatch_failed:{error}"));
    }
}

fn refresh_tray_menu_on_main_thread<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id("monarch-tray") else {
        return;
    };
    let Some(state) = app.try_state::<TrayMenuState>() else {
        return;
    };
    let snapshot = match tray_menu_snapshot(app) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            diagnostics::log(format!("tray_refresh:snapshot_failed:{error}"));
            return;
        }
    };
    let Ok(mut installed) = state.installed.lock() else {
        diagnostics::log("tray_refresh:state_lock_poisoned");
        return;
    };
    if let Err(error) = update_tray_menu(&mut installed, snapshot, |snapshot| {
        let menu = build_tray_menu(app, Some(snapshot))?;
        tray.set_menu(Some(menu))
    }) {
        // Keep the last successfully installed snapshot so the next poll retries.
        diagnostics::log(format!("tray_refresh:failed:{error}"));
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrayMenuDisplay {
    id_key: String,
    friendly_name: String,
    is_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrayMenuSnapshot {
    profiles: Vec<String>,
    displays: Vec<TrayMenuDisplay>,
}

struct TrayMenuState {
    installed: Mutex<Option<TrayMenuSnapshot>>,
}

impl From<super::commands::AppSnapshotDto> for TrayMenuSnapshot {
    fn from(snapshot: super::commands::AppSnapshotDto) -> Self {
        Self {
            profiles: snapshot.profiles.into_iter().map(|p| p.name).collect(),
            displays: snapshot
                .displays
                .into_iter()
                .map(|d| TrayMenuDisplay {
                    id_key: d.id_key,
                    friendly_name: d.friendly_name,
                    is_active: d.is_active,
                })
                .collect(),
        }
    }
}

fn tray_menu_snapshot<R: Runtime>(app: &AppHandle<R>) -> Result<TrayMenuSnapshot, String> {
    let state = app.state::<MonarchAppState>();
    state.controller.snapshot().map(TrayMenuSnapshot::from)
}

fn update_tray_menu(
    installed: &mut Option<TrayMenuSnapshot>,
    snapshot: TrayMenuSnapshot,
    replace: impl FnOnce(&TrayMenuSnapshot) -> tauri::Result<()>,
) -> tauri::Result<()> {
    // Replacing the native Windows menu dismisses it if it is open. Background
    // polling must leave it intact when labels, actions and ordering are unchanged.
    if installed.as_ref() != Some(&snapshot) {
        replace(&snapshot)?;
        *installed = Some(snapshot);
    }
    Ok(())
}

fn build_tray_menu<R: Runtime>(
    app: &AppHandle<R>,
    snapshot: Option<&TrayMenuSnapshot>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    let mut profiles_menu = SubmenuBuilder::new(app, "Profiles");
    if let Some(snapshot) = snapshot {
        if snapshot.profiles.is_empty() {
            profiles_menu = profiles_menu.text("profiles.none", "(No Profiles)");
        } else {
            for profile in &snapshot.profiles {
                profiles_menu = profiles_menu.text(format!("profile::{profile}"), profile.clone());
            }
        }
    }

    let mut toggles_menu = SubmenuBuilder::new(app, "Toggle Monitor");
    if let Some(snapshot) = snapshot {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::commands::{
        AppSnapshotDto, DisplayInfoDto, LayoutDto, PendingConfirmationDto, ProfileDto,
        ResolutionDto,
    };

    fn snapshot() -> AppSnapshotDto {
        AppSnapshotDto {
            generation: 1,
            displays: vec![DisplayInfoDto {
                id_key: "1:1".into(),
                friendly_name: "Desk monitor".into(),
                is_active: true,
                is_primary: true,
                resolution: ResolutionDto {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            }],
            layout: LayoutDto { outputs: vec![] },
            profiles: vec![
                ProfileDto {
                    name: "Desk".into(),
                    layout: LayoutDto { outputs: vec![] },
                },
                ProfileDto {
                    name: "TV".into(),
                    layout: LayoutDto { outputs: vec![] },
                },
            ],
            settings: monarch::AppSettings::default(),
            capabilities: vec![],
            pending_confirmation: Some(PendingConfirmationDto { remaining_ms: 5000 }),
        }
    }

    #[test]
    fn repeated_polls_preserve_the_menu_installed_at_startup() {
        let snapshot = TrayMenuSnapshot::from(snapshot());
        let mut installed = Some(snapshot.clone());
        for _ in 0..5 {
            update_tray_menu(&mut installed, snapshot.clone(), |_| {
                panic!("an unchanged poll must not replace the open native menu")
            })
            .unwrap();
        }
    }

    #[test]
    fn non_menu_snapshot_changes_do_not_replace_the_menu() {
        let original = snapshot();
        let mut installed = Some(TrayMenuSnapshot::from(original.clone()));
        let mut next = original;
        next.generation += 1;
        next.pending_confirmation.as_mut().unwrap().remaining_ms = 3000;
        next.settings.start_with_windows = !next.settings.start_with_windows;
        next.displays[0].resolution.width = 2560;
        next.displays[0].refresh_rate_mhz = 144_000;
        next.displays[0].is_primary = false;
        update_tray_menu(&mut installed, next.into(), |_| {
            panic!("metadata and display modes do not change the tray menu")
        })
        .unwrap();
    }

    #[test]
    fn changed_labels_actions_and_order_replace_the_menu_once() {
        let original = TrayMenuSnapshot::from(snapshot());
        let changes: &[fn(&mut TrayMenuSnapshot)] = &[
            |s| s.profiles.push("Game".into()),
            |s| s.profiles.clear(),
            |s| s.profiles[0] = "Work".into(),
            |s| s.profiles.swap(0, 1),
            |s| s.displays[0].is_active = false,
            |s| s.displays[0].friendly_name = "TV".into(),
            |s| s.displays[0].id_key = "2:1".into(),
            |s| s.displays.clear(),
            |s| {
                s.displays.push(TrayMenuDisplay {
                    id_key: "1:2".into(),
                    friendly_name: "Second monitor".into(),
                    is_active: false,
                });
            },
        ];
        for change in changes {
            let mut installed = Some(original.clone());
            let mut next = original.clone();
            change(&mut next);
            let mut replacements = 0;
            for _ in 0..2 {
                update_tray_menu(&mut installed, next.clone(), |requested| {
                    assert_eq!(requested, &next);
                    replacements += 1;
                    Ok(())
                })
                .unwrap();
            }
            assert_eq!(replacements, 1);
            assert_eq!(installed.as_ref(), Some(&next));
        }
    }

    #[test]
    fn failed_replacement_preserves_the_installed_state_and_retries() {
        let original = TrayMenuSnapshot::from(snapshot());
        let mut next = original.clone();
        next.displays[0].is_active = false;
        for previous in [None, Some(original)] {
            let mut installed = previous.clone();
            let error = update_tray_menu(&mut installed, next.clone(), |_| {
                Err(std::io::Error::other("native menu replacement failed").into())
            });
            assert!(error.is_err());
            assert_eq!(installed, previous);
            let mut retried = false;
            update_tray_menu(&mut installed, next.clone(), |_| {
                retried = true;
                Ok(())
            })
            .unwrap();
            assert!(retried);
            assert_eq!(installed.as_ref(), Some(&next));
        }
    }
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
