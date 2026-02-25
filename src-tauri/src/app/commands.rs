use std::time::Duration;

use monarch::{
    AppSettings, DisplayId, DisplayInfo, Layout, OutputConfig, Position, Profile, Resolution,
};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, PhysicalPosition, Position as TauriPosition, Runtime, State, WebviewWindow,
};

use crate::app::events::{
    emit_confirmation, emit_state_changed, refresh_tray_menu, spawn_confirmation_watchdog,
    spawn_deferred_tray_refresh, ConfirmationEvent, ConfirmationRevertReason,
};
use crate::app::startup;
use crate::app::state::{format_display_key, MonarchAppState};

type CommandResult<T> = Result<T, String>;

#[derive(Clone, Serialize)]
pub struct DisplayInfoDto {
    pub id_key: String,
    pub friendly_name: String,
    pub is_active: bool,
    pub is_primary: bool,
    pub resolution: ResolutionDto,
    pub refresh_rate_mhz: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResolutionDto {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PositionDto {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OutputConfigDto {
    pub display_key: String,
    pub enabled: bool,
    pub position: PositionDto,
    pub resolution: ResolutionDto,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutDto {
    pub outputs: Vec<OutputConfigDto>,
}

#[derive(Clone, Serialize)]
pub struct ProfileDto {
    pub name: String,
    pub layout: LayoutDto,
}

#[derive(Clone, Serialize)]
pub struct PendingConfirmationDto {
    pub remaining_ms: u64,
}

#[derive(Clone, Serialize)]
pub struct AppSnapshotDto {
    pub displays: Vec<DisplayInfoDto>,
    pub layout: LayoutDto,
    pub profiles: Vec<ProfileDto>,
    pub settings: AppSettings,
    pub pending_confirmation: Option<PendingConfirmationDto>,
}

#[tauri::command]
pub async fn list_displays(
    state: State<'_, MonarchAppState>,
) -> CommandResult<Vec<DisplayInfoDto>> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    let displays = guard
        .manager
        .list_displays()
        .map_err(|err| err.to_string())?;
    Ok(displays.into_iter().map(display_to_dto).collect())
}

#[tauri::command]
pub async fn get_layout(state: State<'_, MonarchAppState>) -> CommandResult<LayoutDto> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    let layout = guard.manager.get_layout().map_err(|err| err.to_string())?;
    layout_to_dto(&layout)
}

#[tauri::command]
pub async fn list_profiles(state: State<'_, MonarchAppState>) -> CommandResult<Vec<ProfileDto>> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    Ok(guard
        .manager
        .list_profiles()
        .into_iter()
        .map(profile_to_dto)
        .collect::<Result<Vec<_>, _>>()?)
}

#[tauri::command]
pub async fn get_snapshot(state: State<'_, MonarchAppState>) -> CommandResult<AppSnapshotDto> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;
    snapshot_from_manager(&guard.manager).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn toggle_display<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, MonarchAppState>,
    display_key: String,
) -> CommandResult<()> {
    let display_id =
        crate::app::state::parse_display_key(&display_key).map_err(|err| err.to_string())?;

    {
        let guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        maybe_move_window_before_detach(
            &window,
            &guard.manager.get_layout().map_err(|e| e.to_string())?,
            &display_id,
        );
    }

    let timeout = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .toggle_display(&display_id)
            .map_err(|err| err.to_string())?;
        guard
            .manager
            .pending_confirmation_remaining()
            .unwrap_or_else(|| Duration::from_secs(10))
    };

    refresh_tray_menu(&app);
    emit_state_changed(&app);
    emit_confirmation(
        &app,
        ConfirmationEvent::Applied {
            timeout_ms: timeout.as_millis() as u64,
        },
    );
    spawn_confirmation_watchdog(app, timeout);
    Ok(())
}

#[tauri::command]
pub async fn apply_layout<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
    layout: LayoutDto,
) -> CommandResult<()> {
    let layout = dto_to_layout(layout)?;
    let timeout = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .apply_layout(layout)
            .map_err(|err| err.to_string())?;
        guard
            .manager
            .pending_confirmation_remaining()
            .unwrap_or_else(|| Duration::from_secs(10))
    };

    refresh_tray_menu(&app);
    emit_state_changed(&app);
    emit_confirmation(
        &app,
        ConfirmationEvent::Applied {
            timeout_ms: timeout.as_millis() as u64,
        },
    );
    spawn_confirmation_watchdog(app, timeout);
    Ok(())
}

#[tauri::command]
pub async fn save_profile<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
    name: String,
) -> CommandResult<()> {
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .save_profile(name)
            .map_err(|err| err.to_string())?;
    }
    refresh_tray_menu(&app);
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn apply_profile<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
    name: String,
) -> CommandResult<()> {
    let pending_timeout = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .apply_profile(&name)
            .map_err(|err| err.to_string())?;
        guard.manager.pending_confirmation_remaining()
    };
    refresh_tray_menu(&app);
    emit_state_changed(&app);
    if let Some(timeout) = pending_timeout {
        emit_confirmation(
            &app,
            ConfirmationEvent::Applied {
                timeout_ms: timeout.as_millis() as u64,
            },
        );
        spawn_confirmation_watchdog(app, timeout);
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_profile<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
    name: String,
) -> CommandResult<()> {
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .delete_profile(&name)
            .map_err(|err| err.to_string())?;
    }
    refresh_tray_menu(&app);
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn restore_last_layout<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
) -> CommandResult<()> {
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .restore_last_layout()
            .map_err(|err| err.to_string())?;
    }
    refresh_tray_menu(&app);
    emit_state_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn confirm_current_layout<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
) -> CommandResult<()> {
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .confirm_current_layout()
            .map_err(|err| err.to_string())?;
    }
    // Avoid a synchronous tray rebuild here. It queries display state again and can block on some
    // systems immediately after topology changes, which leaves the UI action stuck in "busy".
    emit_state_changed(&app);
    spawn_deferred_tray_refresh(app.clone(), Duration::from_millis(250));
    emit_confirmation(&app, ConfirmationEvent::Confirmed);
    Ok(())
}

#[tauri::command]
pub async fn rollback_pending<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
) -> CommandResult<()> {
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .rollback_pending()
            .map_err(|err| err.to_string())?;
    }
    // Avoid a synchronous tray rebuild here for the same reason as confirm_current_layout().
    emit_state_changed(&app);
    spawn_deferred_tray_refresh(app.clone(), Duration::from_millis(250));
    emit_confirmation(
        &app,
        ConfirmationEvent::Reverted {
            reason: ConfirmationRevertReason::Manual,
        },
    );
    Ok(())
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, MonarchAppState>,
    settings: AppSettings,
) -> CommandResult<()> {
    let previous_settings = {
        let guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard.manager.settings().clone()
    };
    let startup_enabled = settings.start_with_windows;
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())?;
        guard
            .manager
            .update_settings(settings)
            .map_err(|err| err.to_string())?;
    }
    if let Err(err) = startup::sync_start_with_windows(startup_enabled) {
        let rollback_err = state
            .0
            .lock()
            .map_err(|_| "state mutex poisoned".to_string())
            .and_then(|mut guard| {
                guard
                    .manager
                    .update_settings(previous_settings)
                    .map_err(|inner| inner.to_string())
            });

        if let Err(rollback_err) = rollback_err {
            return Err(format!(
                "{err} (and failed to restore previous settings: {rollback_err})"
            ));
        }

        return Err(err);
    }
    refresh_tray_menu(&app);
    emit_state_changed(&app);
    Ok(())
}

pub fn snapshot_from_manager<B, S>(
    manager: &monarch::MonarchDisplayManager<B, S>,
) -> Result<AppSnapshotDto, monarch::ManagerError>
where
    B: monarch::DisplayBackend,
    S: monarch::ConfigStore,
{
    let displays = manager
        .list_displays()?
        .into_iter()
        .map(display_to_dto)
        .collect::<Vec<_>>();
    let layout = layout_to_dto(&manager.get_layout()?).map_err(monarch::ManagerError::Backend)?;
    let profiles = manager
        .list_profiles()
        .into_iter()
        .map(profile_to_dto)
        .collect::<Result<Vec<_>, _>>()
        .map_err(monarch::ManagerError::Backend)?;
    let pending_confirmation =
        manager
            .pending_confirmation_remaining()
            .map(|remaining| PendingConfirmationDto {
                remaining_ms: remaining.as_millis() as u64,
            });

    Ok(AppSnapshotDto {
        displays,
        layout,
        profiles,
        settings: manager.settings().clone(),
        pending_confirmation,
    })
}

fn display_to_dto(display: DisplayInfo) -> DisplayInfoDto {
    DisplayInfoDto {
        id_key: format_display_key(&display.id),
        friendly_name: display.friendly_name,
        is_active: display.is_active,
        is_primary: display.is_primary,
        resolution: ResolutionDto {
            width: display.resolution.width,
            height: display.resolution.height,
        },
        refresh_rate_mhz: display.refresh_rate_mhz,
    }
}

fn layout_to_dto(layout: &Layout) -> Result<LayoutDto, String> {
    Ok(LayoutDto {
        outputs: layout
            .outputs
            .iter()
            .map(output_to_dto)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn output_to_dto(output: &OutputConfig) -> Result<OutputConfigDto, String> {
    Ok(OutputConfigDto {
        display_key: format_display_key(&output.display_id),
        enabled: output.enabled,
        position: PositionDto {
            x: output.position.x,
            y: output.position.y,
        },
        resolution: ResolutionDto {
            width: output.resolution.width,
            height: output.resolution.height,
        },
        refresh_rate_mhz: output.refresh_rate_mhz,
        primary: output.primary,
    })
}

fn dto_to_layout(dto: LayoutDto) -> CommandResult<Layout> {
    let outputs = dto
        .outputs
        .into_iter()
        .map(|output| {
            let display_id = crate::app::state::parse_display_key(&output.display_key)
                .map_err(|err| err.to_string())?;
            Ok(OutputConfig {
                display_id,
                enabled: output.enabled,
                position: Position {
                    x: output.position.x,
                    y: output.position.y,
                },
                resolution: Resolution {
                    width: output.resolution.width,
                    height: output.resolution.height,
                },
                refresh_rate_mhz: output.refresh_rate_mhz,
                primary: output.primary,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Layout { outputs })
}

fn profile_to_dto(profile: Profile) -> Result<ProfileDto, String> {
    Ok(ProfileDto {
        name: profile.name,
        layout: layout_to_dto(&profile.layout)?,
    })
}

fn maybe_move_window_before_detach<R: Runtime>(
    window: &WebviewWindow<R>,
    layout: &Layout,
    target: &DisplayId,
) {
    let target_output = match layout
        .outputs
        .iter()
        .find(|output| &output.display_id == target && output.enabled)
    {
        Some(output) => output,
        None => return,
    };
    let fallback = match layout
        .outputs
        .iter()
        .find(|output| output.enabled && output.display_id != *target)
    {
        Some(output) => output,
        None => return,
    };

    let Ok(position) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let center_x = position.x + (size.width as i32 / 2);
    let center_y = position.y + (size.height as i32 / 2);

    let inside_target = center_x >= target_output.position.x
        && center_x < target_output.position.x + target_output.resolution.width as i32
        && center_y >= target_output.position.y
        && center_y < target_output.position.y + target_output.resolution.height as i32;

    if inside_target {
        let _ = window.set_position(TauriPosition::Physical(PhysicalPosition {
            x: fallback.position.x + 48,
            y: fallback.position.y + 48,
        }));
    }
}
