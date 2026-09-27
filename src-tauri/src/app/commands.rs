use std::process::Command;

use monarch::{
    AppSettings, DisplayId, DisplayInfo, Layout, OutputConfig, Position, Profile, Resolution,
};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, PhysicalPosition, Position as TauriPosition, Runtime, State, WebviewWindow,
};

use crate::app::coordinator::Operation;
use crate::app::state::{format_display_key, MonarchAppState};
use tauri::Manager;

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
    #[serde(default)]
    pub identity: monarch::MonitorIdentity,
    pub enabled: bool,
    pub position: PositionDto,
    pub resolution: ResolutionDto,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
    #[serde(default)]
    pub rotation: Option<monarch::Rotation>,
    #[serde(default)]
    pub hdr_enabled: Option<bool>,
    #[serde(default)]
    pub scale_percent: Option<u32>,
    #[serde(default)]
    pub clone_group: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutDto {
    pub outputs: Vec<OutputConfigDto>,
}

#[derive(Clone, Serialize)]
pub struct ProfileDto {
    pub name: String,
    pub layout: LayoutDto,
    pub audio_output: Option<monarch::AudioOutput>,
}

#[derive(Clone, Serialize)]
pub struct PendingConfirmationDto {
    pub remaining_ms: u64,
}

#[derive(Clone, Serialize)]
pub struct AppSnapshotDto {
    pub generation: u64,
    pub displays: Vec<DisplayInfoDto>,
    pub layout: LayoutDto,
    pub profiles: Vec<ProfileDto>,
    pub audio: monarch::AudioSnapshot,
    pub capabilities: Vec<DisplayCapabilitiesDto>,
    pub settings: AppSettings,
    pub pending_confirmation: Option<PendingConfirmationDto>,
}

#[derive(Clone, Serialize)]
pub struct DisplayCapabilitiesDto {
    pub display_key: String,
    pub identity: monarch::MonitorIdentity,
    #[serde(flatten)]
    pub capabilities: monarch::capabilities::DisplayCapabilities,
}

#[tauri::command]
pub async fn get_display_capabilities(
    state: State<'_, MonarchAppState>,
) -> CommandResult<Vec<DisplayCapabilitiesDto>> {
    state.controller.refresh(false);
    Ok(state.controller.snapshot()?.capabilities)
}

#[tauri::command]
pub async fn get_snapshot(state: State<'_, MonarchAppState>) -> CommandResult<AppSnapshotDto> {
    state.controller.snapshot()
}

async fn execute<R: Runtime>(app: &AppHandle<R>, operation: Operation) -> CommandResult<()> {
    let controller = app.state::<MonarchAppState>().controller.clone();
    controller.execute(operation).await
}

#[tauri::command]
pub async fn toggle_display<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    display_key: String,
) -> CommandResult<()> {
    let id = monarch::identity::parse_display_key(&display_key).map_err(|e| e.to_string())?;
    let snapshot = app.state::<MonarchAppState>().controller.snapshot()?;
    maybe_move_window_before_detach(&window, &dto_to_layout(snapshot.layout)?, &id);
    execute(&app, Operation::Toggle(display_key, false)).await
}

#[tauri::command]
pub async fn apply_layout<R: Runtime>(app: AppHandle<R>, layout: LayoutDto) -> CommandResult<()> {
    execute(&app, Operation::ApplyLayout(dto_to_layout(layout)?)).await
}

#[tauri::command]
pub async fn apply_profile<R: Runtime>(app: AppHandle<R>, name: String) -> CommandResult<()> {
    execute(&app, Operation::ApplyProfile(name, false)).await
}

#[tauri::command]
pub async fn save_profile<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    audio_output_id: Option<String>,
) -> CommandResult<()> {
    execute(&app, Operation::SaveProfile(name, audio_output_id)).await
}

#[tauri::command]
pub async fn set_profile_audio<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    audio_output_id: Option<String>,
) -> CommandResult<()> {
    execute(&app, Operation::ProfileAudio(name, audio_output_id)).await
}

#[tauri::command]
pub async fn set_audio_output<R: Runtime>(app: AppHandle<R>, id: String) -> CommandResult<()> {
    execute(&app, Operation::AudioOutput(id)).await
}

#[tauri::command]
pub async fn delete_profile<R: Runtime>(app: AppHandle<R>, name: String) -> CommandResult<()> {
    execute(&app, Operation::DeleteProfile(name)).await
}

#[tauri::command]
pub async fn restore_last_layout<R: Runtime>(app: AppHandle<R>) -> CommandResult<()> {
    execute(&app, Operation::Restore).await
}

#[tauri::command]
pub async fn confirm_current_layout<R: Runtime>(app: AppHandle<R>) -> CommandResult<()> {
    execute(&app, Operation::Confirm).await
}

#[tauri::command]
pub async fn rollback_pending<R: Runtime>(app: AppHandle<R>) -> CommandResult<()> {
    execute(&app, Operation::Rollback).await
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: AppSettings,
) -> CommandResult<()> {
    execute(&app, Operation::Settings(settings)).await
}

#[tauri::command]
pub async fn open_external_url(url: String) -> CommandResult<()> {
    let url = url.trim();
    if url.is_empty() {
        return Err("url cannot be empty".to_string());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) URLs are supported".to_string());
    }

    open_external_url_with_system(url)
}

pub fn snapshot_from_manager<B, S>(
    manager: &monarch::MonarchDisplayManager<B, S>,
) -> Result<AppSnapshotDto, monarch::ManagerError>
where
    B: monarch::DisplayBackend,
    S: monarch::ConfigStore,
{
    let observed = manager.snapshot()?;
    let generation = observed.generation;
    let displays = observed
        .displays
        .into_iter()
        .map(display_to_dto)
        .collect::<Vec<_>>();
    let layout = layout_to_dto(&observed.layout);
    let mut snapshot = AppSnapshotDto {
        generation,
        displays,
        layout,
        profiles: Vec::new(),
        audio: manager
            .audio_snapshot()
            .unwrap_or_else(|error| monarch::AudioSnapshot::unavailable(error.to_string())),
        capabilities: manager
            .get_display_capabilities()?
            .into_iter()
            .map(|capabilities| DisplayCapabilitiesDto {
                display_key: format_display_key(&capabilities.display_id),
                identity: capabilities.display_id.identity.clone(),
                capabilities,
            })
            .collect(),
        settings: manager.settings().clone(),
        pending_confirmation: None,
    };
    update_snapshot_metadata(&mut snapshot, manager);
    snapshot.profiles = manager
        .list_profiles()
        .into_iter()
        .map(|mut p| {
            let saved = p.layout.clone();
            p.layout = monarch::identity::remap_layout(&p.layout, &observed.layout);
            for (old, current) in saved.outputs.iter().zip(&mut p.layout.outputs) {
                monarch::identity::preserve_evidence(&old.display_id, &mut current.display_id);
            }
            profile_to_dto(p)
        })
        .collect();
    Ok(snapshot)
}

pub fn update_snapshot_metadata<B: monarch::DisplayBackend, S: monarch::ConfigStore>(
    snapshot: &mut AppSnapshotDto,
    manager: &monarch::MonarchDisplayManager<B, S>,
) {
    snapshot.profiles = manager
        .list_profiles()
        .into_iter()
        .map(profile_to_dto)
        .collect();
    snapshot.settings = manager.settings().clone();
    snapshot.pending_confirmation =
        manager
            .pending_confirmation_remaining()
            .map(|remaining| PendingConfirmationDto {
                remaining_ms: remaining.as_millis() as u64,
            });
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

fn layout_to_dto(layout: &Layout) -> LayoutDto {
    LayoutDto {
        outputs: layout.outputs.iter().map(output_to_dto).collect(),
    }
}

fn output_to_dto(output: &OutputConfig) -> OutputConfigDto {
    OutputConfigDto {
        display_key: format_display_key(&output.display_id),
        identity: output.display_id.identity.clone(),
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
        rotation: output.rotation,
        hdr_enabled: output.hdr_enabled,
        scale_percent: output.scale_percent,
        clone_group: output.clone_group.clone(),
    }
}

fn dto_to_layout(dto: LayoutDto) -> CommandResult<Layout> {
    let outputs = dto
        .outputs
        .into_iter()
        .map(|output| {
            let mut display_id = crate::app::state::parse_display_key(&output.display_key)
                .map_err(|err| err.to_string())?;
            display_id.identity = output.identity;
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
                rotation: output.rotation,
                hdr_enabled: output.hdr_enabled,
                scale_percent: output.scale_percent,
                clone_group: output.clone_group,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Layout { outputs })
}

fn profile_to_dto(profile: Profile) -> ProfileDto {
    ProfileDto {
        name: profile.name,
        layout: layout_to_dto(&profile.layout),
        audio_output: profile.audio_output,
    }
}

fn open_external_url_with_system(url: &str) -> CommandResult<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map_err(|err| format!("failed to open URL: {err}"))?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|err| format!("failed to open URL: {err}"))?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|err| format!("failed to open URL: {err}"))?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err("opening external URLs is not supported on this platform".to_string())
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
