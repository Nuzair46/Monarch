use std::collections::{BTreeSet, HashMap};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::app::events::{
    handle_profile_apply_external_action, handle_toggle_display_external_action,
};
use crate::app::state::{format_display_key, MonarchAppState};

#[derive(Clone, Debug)]
enum ShortcutAction {
    ApplyProfile(String),
    ToggleDisplay(String),
}

#[derive(Clone, Debug)]
struct ShortcutBinding {
    shortcut: String,
    action: ShortcutAction,
    label: String,
}

pub fn sync_global_shortcuts<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let bindings = collect_bindings(app)?;
    validate_unique_shortcuts(&bindings)?;

    let manager = app.global_shortcut();
    manager
        .unregister_all()
        .map_err(|err| format!("failed to clear existing global shortcuts: {err}"))?;

    for binding in bindings {
        let shortcut_string = binding.shortcut.clone();
        let action = binding.action.clone();
        let label = binding.label.clone();

        if let Err(err) =
            manager.on_shortcut(shortcut_string.as_str(), move |app_handle, _, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }

                let app_handle = app_handle.clone();
                let action = action.clone();
                std::thread::spawn(move || match action {
                    ShortcutAction::ApplyProfile(name) => {
                        handle_profile_apply_external_action(&app_handle, &name)
                    }
                    ShortcutAction::ToggleDisplay(display_key) => {
                        handle_toggle_display_external_action(&app_handle, &display_key)
                    }
                });
            })
        {
            let _ = manager.unregister_all();
            return Err(format!(
                "failed to register global shortcut '{shortcut_string}' for {label}: {err}"
            ));
        }
    }

    Ok(())
}

fn collect_bindings<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<ShortcutBinding>, String> {
    let state = app.state::<MonarchAppState>();
    let guard = state
        .0
        .lock()
        .map_err(|_| "state mutex poisoned".to_string())?;

    let settings = guard.manager.settings().clone();
    if !settings.global_shortcuts_enabled {
        return Ok(Vec::new());
    }
    let existing_profiles = guard
        .manager
        .list_profiles()
        .into_iter()
        .map(|profile| profile.name)
        .collect::<BTreeSet<_>>();
    let current_displays = guard
        .manager
        .list_displays()
        .map_err(|err| err.to_string())?;

    let profile_shortcut_base = settings.profile_shortcut_base.clone().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let display_toggle_shortcut_base =
        settings
            .display_toggle_shortcut_base
            .clone()
            .and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            });
    let profile_shortcuts = settings.profile_shortcuts.clone();
    let display_toggle_shortcuts = settings.display_toggle_shortcuts.clone();

    let mut bindings = Vec::new();

    if let Some(base) = profile_shortcut_base.as_deref() {
        for (index, profile_name) in existing_profiles.iter().enumerate() {
            let Some(shortcut) = compose_index_shortcut(base, index) else {
                continue;
            };
            bindings.push(ShortcutBinding {
                shortcut,
                label: format!("profile '{profile_name}'"),
                action: ShortcutAction::ApplyProfile(profile_name.clone()),
            });
        }
    } else {
        for (profile_name, shortcut) in profile_shortcuts {
            if !existing_profiles.contains(&profile_name) {
                continue;
            }
            let shortcut = shortcut.trim();
            if shortcut.is_empty() {
                continue;
            }
            bindings.push(ShortcutBinding {
                shortcut: shortcut.to_string(),
                label: format!("profile '{profile_name}'"),
                action: ShortcutAction::ApplyProfile(profile_name),
            });
        }
    }

    if let Some(base) = display_toggle_shortcut_base.as_deref() {
        for (index, display) in current_displays.iter().enumerate() {
            let Some(shortcut) = compose_index_shortcut(base, index) else {
                continue;
            };
            let display_key = format_display_key(&display.id);
            bindings.push(ShortcutBinding {
                shortcut,
                label: format!("display '{}'", display.friendly_name),
                action: ShortcutAction::ToggleDisplay(display_key),
            });
        }
    } else {
        for (display_key, shortcut) in display_toggle_shortcuts {
            let shortcut = shortcut.trim();
            if shortcut.is_empty() {
                continue;
            }
            bindings.push(ShortcutBinding {
                shortcut: shortcut.to_string(),
                label: format!("display '{display_key}'"),
                action: ShortcutAction::ToggleDisplay(display_key),
            });
        }
    }

    Ok(bindings)
}

fn compose_index_shortcut(base: &str, index: usize) -> Option<String> {
    let slot = match index {
        0..=8 => char::from_u32(b'1' as u32 + index as u32)?,
        9 => '0',
        _ => return None,
    };
    Some(format!("{base}+{slot}"))
}

fn validate_unique_shortcuts(bindings: &[ShortcutBinding]) -> Result<(), String> {
    let mut seen = HashMap::<String, String>::new();
    for binding in bindings {
        let key = binding.shortcut.trim().to_ascii_lowercase();
        if let Some(previous) = seen.insert(key, binding.label.clone()) {
            return Err(format!(
                "duplicate global shortcut assignment: '{}' is used by {} and {}",
                binding.shortcut, previous, binding.label
            ));
        }
    }
    Ok(())
}
