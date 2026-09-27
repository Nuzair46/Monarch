use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::backend::DisplayBackend;
use crate::model::{
    AppConfig, AppSettings, DisplayId, DisplayInfo, Layout, Profile,
    DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE, DEFAULT_PROFILE_SHORTCUT_BASE,
};
use crate::store::ConfigStore;
use crate::ManagerError;

#[derive(Clone, Debug)]
struct PendingConfirmation {
    previous_layout: Layout,
    applied_at: Instant,
    timeout: Duration,
}

impl PendingConfirmation {
    fn new(previous_layout: Layout, timeout: Duration) -> Self {
        Self {
            previous_layout,
            applied_at: Instant::now(),
            timeout,
        }
    }

    fn expired(&self) -> bool {
        self.applied_at.elapsed() >= self.timeout
    }

    fn remaining(&self) -> Duration {
        self.timeout
            .checked_sub(self.applied_at.elapsed())
            .unwrap_or(Duration::ZERO)
    }
}

#[derive(Debug)]
pub struct MonarchDisplayManager<B, S> {
    backend: B,
    store: S,
    config: AppConfig,
    pending_confirmation: Option<PendingConfirmation>,
    confirmation_timeout: Duration,
}

impl<B, S> MonarchDisplayManager<B, S>
where
    B: DisplayBackend,
    S: ConfigStore,
{
    pub fn new(backend: B, store: S) -> Result<Self, ManagerError> {
        let mut config = store.load()?;
        let current_layout = backend.get_layout()?;
        let current_displays = backend.list_displays().unwrap_or_default();
        let mut should_persist = false;
        if config
            .settings
            .profile_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            config.settings.profile_shortcut_base = Some(DEFAULT_PROFILE_SHORTCUT_BASE.to_string());
            should_persist = true;
        }
        if config
            .settings
            .display_toggle_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            config.settings.display_toggle_shortcut_base =
                Some(DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE.to_string());
            should_persist = true;
        }
        let confirmation_timeout = Duration::from_secs(config.settings.revert_timeout_secs.max(1));

        if config.last_known_good_layout.is_none() || config.last_restorable_layout.is_none() {
            if config.last_known_good_layout.is_none() {
                config.last_known_good_layout = Some(current_layout.clone());
            }
            if config.last_restorable_layout.is_none() {
                config.last_restorable_layout = Some(current_layout.clone());
            }
            should_persist = true;
        }
        if sync_display_fingerprints(&mut config, &current_displays) {
            should_persist = true;
        }
        if migrate_saved_layout_ids_with_fingerprints(
            &mut config,
            &current_layout,
            &current_displays,
        ) {
            should_persist = true;
        }
        if should_persist {
            store.save(&config)?;
        }

        Ok(Self {
            backend,
            store,
            config,
            pending_confirmation: None,
            confirmation_timeout,
        })
    }

    pub fn set_confirmation_timeout(&mut self, timeout: Duration) {
        self.confirmation_timeout = timeout;
    }

    pub fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.backend.list_displays()
    }

    pub fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.backend.get_layout()
    }

    pub fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        self.backend.color_state_signature()
    }

    pub fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        self.backend.reapply_color_calibration()
    }

    pub fn invalidate_backend_cache(&self) -> Result<(), ManagerError> {
        self.backend.invalidate_cache()
    }

    pub fn has_pending_confirmation(&self) -> bool {
        self.pending_confirmation.is_some()
    }

    pub fn pending_confirmation_remaining(&self) -> Option<Duration> {
        self.pending_confirmation
            .as_ref()
            .map(PendingConfirmation::remaining)
    }

    pub fn apply_layout(&mut self, layout: Layout) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;
        let mut layout = layout;
        layout.ensure_valid()?;
        normalize_primary(&mut layout);

        let current_layout = self.backend.get_layout()?;
        self.config.last_known_good_layout = Some(current_layout.clone());
        self.config.last_restorable_layout = Some(current_layout.clone());
        self.persist_config()?;

        self.backend.apply_layout(layout)?;
        self.pending_confirmation = Some(PendingConfirmation::new(
            current_layout,
            self.confirmation_timeout,
        ));
        Ok(())
    }

    pub fn confirm_current_layout(&mut self) -> Result<(), ManagerError> {
        if self.pending_confirmation.is_none() {
            return Err(ManagerError::NoPendingConfirmation);
        }

        let current_layout = self.backend.get_layout()?;
        self.pending_confirmation = None;
        self.config.last_known_good_layout = Some(current_layout);
        self.persist_config()
    }

    pub fn rollback_pending(&mut self) -> Result<(), ManagerError> {
        let pending = self
            .pending_confirmation
            .take()
            .ok_or(ManagerError::NoPendingConfirmation)?;

        if let Err(error) = self.backend.apply_layout(pending.previous_layout.clone()) {
            // Keep the pending confirmation when the rollback apply fails, so the state is not
            // silently dropped: the caller can retry the rollback or confirm the current layout.
            self.pending_confirmation = Some(pending);
            return Err(error);
        }
        self.config.last_known_good_layout = Some(pending.previous_layout);
        self.persist_config()
    }

    pub fn rollback_if_confirmation_expired(&mut self) -> Result<bool, ManagerError> {
        let expired = self
            .pending_confirmation
            .as_ref()
            .map(PendingConfirmation::expired)
            .unwrap_or(false);

        if expired {
            self.rollback_pending()?;
            return Ok(true);
        }

        Ok(false)
    }

    pub fn toggle_display(&mut self, display_id: &DisplayId) -> Result<(), ManagerError> {
        let mut layout = self.backend.get_layout()?;
        let resolved_display_id = resolve_display_id_for_layout_action(display_id, &layout)
            .unwrap_or_else(|| display_id.clone());
        let index = layout
            .find_output_index(&resolved_display_id)
            .ok_or_else(|| {
                ManagerError::NotFound(format!(
                    "display ({}, {})",
                    display_id.adapter_luid, display_id.target_id
                ))
            })?;

        let currently_enabled = layout.outputs[index].enabled;
        if currently_enabled && layout.enabled_output_count() == 1 {
            return Err(ManagerError::Validation(
                "cannot disable the last active display".to_string(),
            ));
        }

        layout.outputs[index].enabled = !currently_enabled;
        if !layout.outputs[index].enabled {
            layout.outputs[index].primary = false;
        }

        normalize_primary(&mut layout);
        self.apply_layout(layout)
    }

    pub fn save_profile(&mut self, name: impl Into<String>) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;

        let name = name.into();
        let name = name.trim();
        if name.is_empty() {
            return Err(ManagerError::Validation(
                "profile name cannot be empty".to_string(),
            ));
        }

        let layout = self.backend.get_layout()?;
        let profile = Profile {
            name: name.to_string(),
            layout,
        };

        if let Some(existing) = self
            .config
            .profiles
            .iter_mut()
            .find(|candidate| candidate.name == profile.name)
        {
            *existing = profile;
        } else {
            self.config.profiles.push(profile);
            self.config.profiles.sort_by(|a, b| a.name.cmp(&b.name));
        }

        self.persist_config()
    }

    pub fn list_profiles(&self) -> Vec<Profile> {
        self.config.profiles.clone()
    }

    pub fn apply_profile(&mut self, name: &str) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;

        let profile = self
            .config
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .cloned()
            .ok_or_else(|| ManagerError::NotFound(format!("profile '{name}'")))?;

        let mut target_layout = profile.layout;
        target_layout.ensure_valid()?;
        normalize_primary(&mut target_layout);

        let (target_layout, current_layout) = self.remap_and_resolve_for_apply(target_layout)?;

        if current_layout == target_layout {
            return Ok(());
        }

        self.apply_layout(target_layout)
    }

    /// Remap the desired layout onto the current enumeration and strictly validate it. An enabled
    /// output that does not resolve names a display the backend is not enumerating at all, so
    /// there is nothing to apply: the backend gets a chance to record why (it must not change the
    /// topology — nothing it could do would make an absent display appear), and then the layout
    /// is rejected with an actionable error. Attaching a display that IS enumerated but detached
    /// needs none of this: it already resolves, and `apply_layout` attaches it.
    fn remap_and_resolve_for_apply(
        &self,
        target_layout: Layout,
    ) -> Result<(Layout, Layout), ManagerError> {
        let mut current_layout = self.backend.get_layout()?;
        normalize_primary(&mut current_layout);
        let target_layout = remap_layout_display_ids(&target_layout, &current_layout);

        if ensure_all_enabled_outputs_resolve(&target_layout, &current_layout).is_ok() {
            return Ok((target_layout, current_layout));
        }

        self.backend.prepare_attach_targets(&target_layout)?;
        let mut current_layout = self.backend.get_layout()?;
        normalize_primary(&mut current_layout);
        let target_layout = remap_layout_display_ids(&target_layout, &current_layout);
        self.ensure_outputs_resolve_or_report_disconnected(&target_layout, &current_layout)?;
        Ok((target_layout, current_layout))
    }

    fn ensure_outputs_resolve_or_report_disconnected(
        &self,
        desired: &Layout,
        current: &Layout,
    ) -> Result<(), ManagerError> {
        let current_ids: HashSet<&DisplayId> = current
            .outputs
            .iter()
            .map(|output| &output.display_id)
            .collect();

        let Some(unresolved) = desired
            .outputs
            .iter()
            .find(|output| output.enabled && !current_ids.contains(&output.display_id))
        else {
            return Ok(());
        };

        let edid_hash = unresolved
            .display_id
            .edid_hash
            .map(|value| format!("{value:016x}"))
            .unwrap_or_else(|| "-".to_string());
        let friendly = self
            .backend
            .list_displays()
            .unwrap_or_default()
            .into_iter()
            .find(|display| {
                display.id == unresolved.display_id
                    || (unresolved.display_id.edid_hash.is_some()
                        && display.id.edid_hash == unresolved.display_id.edid_hash)
            })
            .map(|display| format!("'{}' ", display.friendly_name))
            .unwrap_or_default();
        // This is NOT an attach failure: the display is not being enumerated at all (powered off,
        // unplugged, or reported unavailable). Say so, instead of blaming a rescue that never ran.
        Err(ManagerError::Validation(format!(
            "display {friendly}(target_id={}, edid_hash={edid_hash}) is not connected right now: Windows does not report it as an available display. turn it on or reconnect it and try again, or re-save the profile without it",
            unresolved.display_id.target_id
        )))
    }

    pub fn delete_profile(&mut self, name: &str) -> Result<(), ManagerError> {
        let before = self.config.profiles.len();
        self.config.profiles.retain(|profile| profile.name != name);
        if self.config.profiles.len() == before {
            return Err(ManagerError::NotFound(format!("profile '{name}'")));
        }
        self.persist_config()
    }

    pub fn restore_last_layout(&mut self) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;

        let target_layout = self
            .config
            .last_restorable_layout
            .clone()
            .or_else(|| self.config.last_known_good_layout.clone())
            .ok_or_else(|| ManagerError::NotFound("last restorable layout".to_string()))?;

        let mut remapped_target_layout = target_layout;
        remapped_target_layout.ensure_valid()?;
        normalize_primary(&mut remapped_target_layout);
        let (remapped_target_layout, current_layout) =
            self.remap_and_resolve_for_apply(remapped_target_layout)?;
        self.backend.apply_layout(remapped_target_layout.clone())?;
        self.pending_confirmation = None;
        self.config.last_restorable_layout = Some(current_layout);
        self.config.last_known_good_layout = Some(remapped_target_layout);
        self.persist_config()
    }

    pub fn settings(&self) -> &AppSettings {
        &self.config.settings
    }

    pub fn update_settings(&mut self, settings: AppSettings) -> Result<(), ManagerError> {
        let revert_timeout_secs = settings.revert_timeout_secs.max(1);
        let startup_profile_name = settings
            .startup_profile_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let profile_shortcut_base = settings
            .profile_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_PROFILE_SHORTCUT_BASE.to_string());
        let display_toggle_shortcut_base = settings
            .display_toggle_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE.to_string());
        if profile_shortcut_base.eq_ignore_ascii_case(&display_toggle_shortcut_base) {
            return Err(ManagerError::Validation(
                "profile and monitor shortcut bases must be different".to_string(),
            ));
        }
        let profile_shortcuts = settings
            .profile_shortcuts
            .into_iter()
            .filter_map(|(name, shortcut)| {
                let name = name.trim();
                let shortcut = shortcut.trim();
                if name.is_empty() || shortcut.is_empty() {
                    return None;
                }
                Some((name.to_string(), shortcut.to_string()))
            })
            .collect();
        let display_toggle_shortcuts = settings
            .display_toggle_shortcuts
            .into_iter()
            .filter_map(|(display_key, shortcut)| {
                let display_key = display_key.trim();
                let shortcut = shortcut.trim();
                if display_key.is_empty() || shortcut.is_empty() {
                    return None;
                }
                Some((display_key.to_string(), shortcut.to_string()))
            })
            .collect();
        self.confirmation_timeout = Duration::from_secs(revert_timeout_secs);
        self.config.settings = AppSettings {
            revert_timeout_secs,
            start_with_windows: settings.start_with_windows,
            startup_profile_name,
            global_shortcuts_enabled: settings.global_shortcuts_enabled,
            profile_shortcut_base: Some(profile_shortcut_base),
            display_toggle_shortcut_base: Some(display_toggle_shortcut_base),
            profile_shortcuts,
            display_toggle_shortcuts,
        };
        self.persist_config()
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    fn ensure_no_pending_confirmation(&self) -> Result<(), ManagerError> {
        if self.pending_confirmation.is_some() {
            return Err(ManagerError::ConfirmationPending);
        }
        Ok(())
    }

    fn persist_config(&self) -> Result<(), ManagerError> {
        self.store.save(&self.config)
    }
}

fn normalize_primary(layout: &mut Layout) {
    let mut primary_found = false;

    for output in &mut layout.outputs {
        if !output.enabled {
            output.primary = false;
            continue;
        }

        if output.primary && !primary_found {
            primary_found = true;
            continue;
        }

        output.primary = false;
    }

    if !primary_found {
        if let Some(output) = layout.outputs.iter_mut().find(|output| output.enabled) {
            output.primary = true;
        }
    }

    if let Some(primary) = layout
        .outputs
        .iter()
        .find(|output| output.enabled && output.primary)
    {
        if primary.position.x != 0 || primary.position.y != 0 {
            let offset_x = primary.position.x;
            let offset_y = primary.position.y;
            for output in &mut layout.outputs {
                output.position.x -= offset_x;
                output.position.y -= offset_y;
            }
        }
    }
}

fn resolve_display_id_for_layout_action(
    requested: &DisplayId,
    layout: &Layout,
) -> Option<DisplayId> {
    if layout.find_output_index(requested).is_some() {
        return Some(requested.clone());
    }

    if let Some(edid_hash) = requested.edid_hash {
        let mut matches = layout
            .outputs
            .iter()
            .filter(|output| output.display_id.edid_hash == Some(edid_hash));
        let first = matches.next()?;
        if matches.next().is_none() {
            return Some(first.display_id.clone());
        }
    }

    if requested.edid_hash.is_none() {
        let mut matches = layout
            .outputs
            .iter()
            .filter(|output| output.display_id.target_id == requested.target_id);
        let first = matches.next()?;
        if matches.next().is_none() {
            return Some(first.display_id.clone());
        }
    }

    None
}

fn fingerprint_for_display(display_id: &DisplayId, friendly_name: Option<&str>) -> Option<String> {
    let edid_hash = display_id.edid_hash?;
    let normalized_name = friendly_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("")
        .to_ascii_uppercase();
    Some(format!("{edid_hash:016x}:{normalized_name}"))
}

fn sync_display_fingerprints(config: &mut AppConfig, displays: &[DisplayInfo]) -> bool {
    let mut changed = false;
    for display in displays {
        let fingerprint = fingerprint_for_display(&display.id, Some(&display.friendly_name));
        let next = crate::model::DisplayFingerprint {
            display_id: display.id.clone(),
            friendly_name: display.friendly_name.clone(),
            edid_fingerprint: fingerprint,
        };

        if let Some(existing) = config
            .display_fingerprints
            .iter_mut()
            .find(|candidate| candidate.display_id == next.display_id)
        {
            if existing != &next {
                *existing = next;
                changed = true;
            }
        } else {
            config.display_fingerprints.push(next);
            changed = true;
        }
    }

    if changed {
        config
            .display_fingerprints
            .sort_by(|left, right| left.display_id.cmp(&right.display_id));
    }
    changed
}

fn migrate_saved_layout_ids_with_fingerprints(
    config: &mut AppConfig,
    current_layout: &Layout,
    current_displays: &[DisplayInfo],
) -> bool {
    let mut changed = false;
    let mut remap_profile_layout = |layout: &mut Layout| {
        let remapped = remap_layout_display_ids_with_fingerprints(
            layout,
            current_layout,
            current_displays,
            &config.display_fingerprints,
        );
        if &remapped != layout {
            *layout = remapped;
            changed = true;
        }
    };

    for profile in &mut config.profiles {
        remap_profile_layout(&mut profile.layout);
    }

    if let Some(layout) = &mut config.last_known_good_layout {
        remap_profile_layout(layout);
    }
    if let Some(layout) = &mut config.last_restorable_layout {
        remap_profile_layout(layout);
    }

    changed
}

fn remap_layout_display_ids_with_fingerprints(
    desired: &Layout,
    current: &Layout,
    current_displays: &[DisplayInfo],
    fingerprints: &[crate::model::DisplayFingerprint],
) -> Layout {
    let mut remapped = remap_layout_display_ids(desired, current);
    let current_ids: HashSet<DisplayId> = current
        .outputs
        .iter()
        .map(|output| output.display_id.clone())
        .collect();
    let mut used: HashSet<DisplayId> = remapped
        .outputs
        .iter()
        .filter(|output| current_ids.contains(&output.display_id))
        .map(|output| output.display_id.clone())
        .collect();

    let friendly_by_id: HashMap<DisplayId, String> = current_displays
        .iter()
        .map(|display| (display.id.clone(), display.friendly_name.clone()))
        .collect();
    let fingerprint_by_id: HashMap<DisplayId, String> = fingerprints
        .iter()
        .filter_map(|entry| {
            entry
                .edid_fingerprint
                .as_ref()
                .map(|fingerprint| (entry.display_id.clone(), fingerprint.clone()))
        })
        .collect();

    let mut current_by_fingerprint: HashMap<String, Vec<DisplayId>> = HashMap::new();
    for output in &current.outputs {
        let fingerprint = fingerprint_by_id
            .get(&output.display_id)
            .cloned()
            .or_else(|| {
                let friendly = friendly_by_id.get(&output.display_id).map(String::as_str);
                fingerprint_for_display(&output.display_id, friendly)
            });
        if let Some(fingerprint) = fingerprint {
            current_by_fingerprint
                .entry(fingerprint)
                .or_default()
                .push(output.display_id.clone());
        }
    }

    for output in &mut remapped.outputs {
        if current_ids.contains(&output.display_id) {
            continue;
        }

        let fingerprint = fingerprint_by_id
            .get(&output.display_id)
            .cloned()
            .or_else(|| fingerprint_for_display(&output.display_id, None));
        let Some(fingerprint) = fingerprint else {
            continue;
        };
        let Some(candidates) = current_by_fingerprint.get(&fingerprint) else {
            continue;
        };

        let available: Vec<_> = candidates
            .iter()
            .filter(|candidate| !used.contains(*candidate))
            .cloned()
            .collect();
        if available.len() != 1 {
            continue;
        }

        let replacement = available[0].clone();
        used.insert(replacement.clone());
        output.display_id = replacement;
    }

    remapped
}

fn remap_layout_display_ids(desired: &Layout, current: &Layout) -> Layout {
    let current_ids: HashSet<DisplayId> = current
        .outputs
        .iter()
        .map(|output| output.display_id.clone())
        .collect();

    if desired
        .outputs
        .iter()
        .all(|output| current_ids.contains(&output.display_id))
    {
        return desired.clone();
    }

    let mut remapped = desired.clone();
    let mut used: HashSet<DisplayId> = HashSet::new();
    for output in &remapped.outputs {
        if current_ids.contains(&output.display_id) {
            used.insert(output.display_id.clone());
        }
    }

    let mut current_by_edid: HashMap<u64, Vec<&crate::model::OutputConfig>> = HashMap::new();
    for output in &current.outputs {
        if let Some(edid_hash) = output.display_id.edid_hash {
            current_by_edid.entry(edid_hash).or_default().push(output);
        }
    }

    for output in &mut remapped.outputs {
        if current_ids.contains(&output.display_id) {
            continue;
        }

        let mut replacement = None;

        if let Some(edid_hash) = output.display_id.edid_hash {
            let candidates = unique_unused_candidates(
                current_by_edid.get(&edid_hash).cloned().unwrap_or_default(),
                &used,
            );
            replacement =
                choose_remap_candidate(&candidates).map(|candidate| candidate.display_id.clone());
        }

        if replacement.is_none() && output.display_id.edid_hash.is_none() {
            // Deterministic fallback for legacy profiles created before EDID hashes were
            // persisted: remap by target id with the same deterministic preference, but never
            // guess across adapters (iGPU/dGPU pairs reuse the same target id numbering, so a
            // cross-adapter pick could land on the wrong physical monitor and get persisted).
            let candidates = unique_unused_candidates_by_target_id(
                output.display_id.target_id,
                &current.outputs,
                &used,
            );
            if candidates_share_one_adapter(&candidates) {
                replacement = choose_remap_candidate(&candidates)
                    .map(|candidate| candidate.display_id.clone());
            }
        }

        if let Some(next_id) = replacement {
            used.insert(next_id.clone());
            output.display_id = next_id;
        }
    }

    remapped
}

fn ensure_all_enabled_outputs_resolve(
    desired: &Layout,
    current: &Layout,
) -> Result<(), ManagerError> {
    let current_ids: HashSet<&DisplayId> = current
        .outputs
        .iter()
        .map(|output| &output.display_id)
        .collect();

    if let Some(unresolved) = desired
        .outputs
        .iter()
        .find(|output| output.enabled && !current_ids.contains(&output.display_id))
    {
        let edid_hash = unresolved
            .display_id
            .edid_hash
            .map(|value| format!("{value:016x}"))
            .unwrap_or_else(|| "-".to_string());
        return Err(ManagerError::Validation(format!(
            "profile/layout references an unknown display (target_id={}, edid_hash={edid_hash}). re-save the profile on this system",
            unresolved.display_id.target_id
        )));
    }

    Ok(())
}

fn candidates_share_one_adapter(candidates: &[&crate::model::OutputConfig]) -> bool {
    let mut adapters = candidates
        .iter()
        .map(|candidate| candidate.display_id.adapter_luid);
    let Some(first) = adapters.next() else {
        return true;
    };
    adapters.all(|adapter| adapter == first)
}

fn choose_remap_candidate<'a>(
    candidates: &[&'a crate::model::OutputConfig],
) -> Option<&'a crate::model::OutputConfig> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

    // Deterministic tie-break for duplicate identities (e.g. a stale cached entry plus the same
    // physical monitor re-enumerated under a new adapter LUID after resume/reboot): prefer the
    // single enabled candidate. Two active identical twins stay ambiguous and are left unmapped.
    let enabled: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.enabled)
        .collect();
    if enabled.len() == 1 {
        return Some(enabled[0]);
    }

    None
}

fn unique_unused_candidates<'a>(
    candidates: Vec<&'a crate::model::OutputConfig>,
    used: &HashSet<DisplayId>,
) -> Vec<&'a crate::model::OutputConfig> {
    candidates
        .into_iter()
        .filter(|candidate| !used.contains(&candidate.display_id))
        .collect()
}

fn unique_unused_candidates_by_target_id<'a>(
    target_id: u32,
    current_outputs: &'a [crate::model::OutputConfig],
    used: &HashSet<DisplayId>,
) -> Vec<&'a crate::model::OutputConfig> {
    current_outputs
        .iter()
        .filter(|candidate| candidate.display_id.target_id == target_id)
        .filter(|candidate| !used.contains(&candidate.display_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{OutputConfig, Position, Resolution},
        MemoryConfigStore, MockBackend,
    };

    fn sample_display_id(target_id: u32) -> DisplayId {
        sample_display_id_on_adapter(1, target_id)
    }

    fn sample_display_id_on_adapter(adapter_luid: u64, target_id: u32) -> DisplayId {
        DisplayId {
            adapter_luid,
            target_id,
            edid_hash: Some(target_id as u64),
        }
    }

    fn sample_layout() -> Layout {
        sample_layout_on_adapter(1)
    }

    fn sample_layout_on_adapter(adapter_luid: u64) -> Layout {
        Layout {
            outputs: vec![
                OutputConfig {
                    display_id: sample_display_id_on_adapter(adapter_luid, 1),
                    enabled: true,
                    position: Position { x: 0, y: 0 },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: true,
                },
                OutputConfig {
                    display_id: sample_display_id_on_adapter(adapter_luid, 2),
                    enabled: true,
                    position: Position { x: 1920, y: 0 },
                    resolution: Resolution {
                        width: 2560,
                        height: 1440,
                    },
                    refresh_rate_mhz: 144_000,
                    primary: false,
                },
            ],
        }
    }

    fn sample_displays() -> Vec<DisplayInfo> {
        sample_displays_on_adapter(1)
    }

    fn sample_displays_on_adapter(adapter_luid: u64) -> Vec<DisplayInfo> {
        vec![
            DisplayInfo {
                id: sample_display_id_on_adapter(adapter_luid, 1),
                friendly_name: "Primary".to_string(),
                is_active: true,
                is_primary: true,
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            },
            DisplayInfo {
                id: sample_display_id_on_adapter(adapter_luid, 2),
                friendly_name: "Secondary".to_string(),
                is_active: true,
                is_primary: false,
                resolution: Resolution {
                    width: 2560,
                    height: 1440,
                },
                refresh_rate_mhz: 144_000,
            },
        ]
    }

    fn build_manager() -> (
        MonarchDisplayManager<MockBackend, MemoryConfigStore>,
        MockBackend,
        MemoryConfigStore,
    ) {
        let backend = MockBackend::new(sample_displays(), sample_layout()).unwrap();
        let store = MemoryConfigStore::default();
        let manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
        (manager, backend, store)
    }

    /// Mock wrapper that counts `prepare_attach_targets` and `apply_layout` calls (the latter is
    /// the core-visible proxy for "the topology was touched") and can simulate apply failures.
    #[derive(Clone)]
    struct CountingBackend {
        inner: MockBackend,
        prepare_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        apply_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fail_apply: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl CountingBackend {
        fn new(inner: MockBackend) -> Self {
            Self {
                inner,
                prepare_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                apply_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                fail_apply: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }

        fn prepare_calls(&self) -> usize {
            self.prepare_calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn apply_calls(&self) -> usize {
            self.apply_calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn set_fail_apply(&self, fail: bool) {
            self.fail_apply
                .store(fail, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl DisplayBackend for CountingBackend {
        fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
            self.inner.list_displays()
        }

        fn get_layout(&self) -> Result<Layout, ManagerError> {
            self.inner.get_layout()
        }

        fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
            self.apply_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_apply.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ManagerError::Backend("simulated apply failure".to_string()));
            }
            self.inner.apply_layout(layout)
        }

        fn prepare_attach_targets(&self, _desired: &Layout) -> Result<(), ManagerError> {
            self.prepare_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn toggle_display_creates_pending_confirmation() {
        let (mut manager, backend, _) = build_manager();
        manager.toggle_display(&sample_display_id(2)).unwrap();

        let layout = backend.current_layout().unwrap();
        assert_eq!(layout.enabled_output_count(), 1);
        assert!(manager.has_pending_confirmation());
        assert!(manager.pending_confirmation_remaining().is_some());
    }

    #[test]
    fn cannot_disable_last_active_display() {
        let (mut manager, _, _) = build_manager();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();

        let err = manager.toggle_display(&sample_display_id(1)).unwrap_err();
        assert!(matches!(err, ManagerError::Validation(_)));
    }

    #[test]
    fn rollback_restores_previous_layout() {
        let (mut manager, backend, store) = build_manager();
        let original = backend.current_layout().unwrap();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.rollback_pending().unwrap();

        assert_eq!(backend.current_layout().unwrap(), original);
        assert_eq!(
            store.snapshot().unwrap().last_known_good_layout,
            Some(original)
        );
    }

    #[test]
    fn detaching_primary_reassigns_primary_and_rebases_origin() {
        let (mut manager, backend, _) = build_manager();

        manager.toggle_display(&sample_display_id(1)).unwrap();

        let layout = backend.current_layout().unwrap();
        let primary = layout
            .outputs
            .iter()
            .find(|output| output.enabled && output.primary)
            .expect("expected new primary after detaching original primary");
        assert_eq!(primary.display_id, sample_display_id(2));
        assert_eq!(primary.position, Position { x: 0, y: 0 });

        let detached_former_primary = layout
            .outputs
            .iter()
            .find(|output| output.display_id == sample_display_id(1))
            .expect("expected detached former primary output");
        assert!(!detached_former_primary.enabled);
        assert_eq!(
            detached_former_primary.position,
            Position { x: -1920, y: 0 }
        );
    }

    #[test]
    fn reattaching_primary_after_detach_preserves_non_overlapping_positions() {
        let (mut manager, backend, _) = build_manager();

        manager.toggle_display(&sample_display_id(1)).unwrap();
        manager.confirm_current_layout().unwrap();
        manager.toggle_display(&sample_display_id(1)).unwrap();

        let layout = backend.current_layout().unwrap();
        let enabled: Vec<_> = layout
            .outputs
            .iter()
            .filter(|output| output.enabled)
            .collect();
        assert_eq!(enabled.len(), 2);
        assert!(enabled
            .iter()
            .any(|output| output.position == Position { x: 0, y: 0 }));
        assert!(enabled
            .iter()
            .any(|output| output.position == Position { x: -1920, y: 0 }));
    }

    #[test]
    fn can_apply_again_after_manual_rollback() {
        let (mut manager, backend, _) = build_manager();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        assert!(manager.has_pending_confirmation());

        manager.rollback_pending().unwrap();
        assert!(!manager.has_pending_confirmation());

        manager.toggle_display(&sample_display_id(2)).unwrap();
        assert!(manager.has_pending_confirmation());
        assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 1);
    }

    #[test]
    fn save_and_apply_profile_round_trip() {
        let (mut manager, backend, _) = build_manager();

        manager.save_profile("dual").unwrap();
        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();

        manager.apply_profile("dual").unwrap();
        let layout = backend.current_layout().unwrap();
        assert_eq!(layout.enabled_output_count(), 2);
        assert!(manager.has_pending_confirmation());
    }

    #[test]
    fn applying_matching_profile_is_a_noop() {
        let (mut manager, backend, _) = build_manager();

        manager.save_profile("current").unwrap();
        let before = backend.current_layout().unwrap();

        manager.apply_profile("current").unwrap();

        assert_eq!(backend.current_layout().unwrap(), before);
        assert!(!manager.has_pending_confirmation());
    }

    #[test]
    fn apply_profile_remaps_display_ids_after_adapter_luid_change() {
        let backend =
            MockBackend::new(sample_displays_on_adapter(9), sample_layout_on_adapter(9)).unwrap();
        let profile_layout = sample_layout_on_adapter(1);
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "dual".to_string(),
                layout: profile_layout,
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager.apply_profile("dual").unwrap();

        let applied = backend.current_layout().unwrap();
        assert!(applied
            .outputs
            .iter()
            .all(|output| output.display_id.adapter_luid == 9));
        assert!(!manager.has_pending_confirmation());
    }

    #[test]
    fn apply_profile_does_not_fallback_to_wrong_target_when_edid_is_known() {
        let display_one = DisplayInfo {
            id: DisplayId {
                adapter_luid: 9,
                target_id: 1,
                edid_hash: Some(1),
            },
            friendly_name: "Left".to_string(),
            is_active: true,
            is_primary: true,
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            refresh_rate_mhz: 60_000,
        };
        let display_three_reusing_target = DisplayInfo {
            id: DisplayId {
                adapter_luid: 9,
                target_id: 2,
                edid_hash: Some(3),
            },
            friendly_name: "Ultrawide".to_string(),
            is_active: false,
            is_primary: false,
            resolution: Resolution {
                width: 3440,
                height: 1440,
            },
            refresh_rate_mhz: 144_000,
        };
        let backend = MockBackend::new(
            vec![display_one.clone(), display_three_reusing_target.clone()],
            Layout {
                outputs: vec![
                    OutputConfig {
                        display_id: display_one.id.clone(),
                        enabled: true,
                        position: Position { x: 0, y: 0 },
                        resolution: display_one.resolution.clone(),
                        refresh_rate_mhz: display_one.refresh_rate_mhz,
                        primary: true,
                    },
                    OutputConfig {
                        display_id: display_three_reusing_target.id.clone(),
                        enabled: false,
                        position: Position { x: 1920, y: 0 },
                        resolution: display_three_reusing_target.resolution.clone(),
                        refresh_rate_mhz: display_three_reusing_target.refresh_rate_mhz,
                        primary: false,
                    },
                ],
            },
        )
        .unwrap();
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "work".to_string(),
                layout: Layout {
                    outputs: vec![
                        OutputConfig {
                            display_id: DisplayId {
                                adapter_luid: 1,
                                target_id: 1,
                                edid_hash: Some(1),
                            },
                            enabled: true,
                            position: Position { x: 0, y: 0 },
                            resolution: Resolution {
                                width: 1920,
                                height: 1080,
                            },
                            refresh_rate_mhz: 60_000,
                            primary: true,
                        },
                        OutputConfig {
                            display_id: DisplayId {
                                adapter_luid: 1,
                                target_id: 2,
                                edid_hash: Some(2),
                            },
                            enabled: true,
                            position: Position { x: 1920, y: 0 },
                            resolution: Resolution {
                                width: 1920,
                                height: 1080,
                            },
                            refresh_rate_mhz: 60_000,
                            primary: false,
                        },
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend, store).unwrap();

        let err = manager.apply_profile("work").unwrap_err();
        assert!(matches!(
            err,
            ManagerError::Validation(message)
                if message.contains("is not connected right now")
        ));
    }

    fn duplicate_edid_current_state() -> (Vec<DisplayInfo>, Layout, DisplayId, DisplayId) {
        let primary_id = DisplayId {
            adapter_luid: 9,
            target_id: 1,
            edid_hash: Some(1),
        };
        let stale_id = DisplayId {
            adapter_luid: 1,
            target_id: 2,
            edid_hash: Some(2),
        };
        let fresh_id = DisplayId {
            adapter_luid: 9,
            target_id: 2,
            edid_hash: Some(2),
        };

        let displays = vec![
            DisplayInfo {
                id: primary_id.clone(),
                friendly_name: "Primary".to_string(),
                is_active: true,
                is_primary: true,
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            },
            DisplayInfo {
                id: stale_id.clone(),
                friendly_name: "Secondary".to_string(),
                is_active: false,
                is_primary: false,
                resolution: Resolution {
                    width: 2560,
                    height: 1440,
                },
                refresh_rate_mhz: 144_000,
            },
            DisplayInfo {
                id: fresh_id.clone(),
                friendly_name: "Secondary".to_string(),
                is_active: true,
                is_primary: false,
                resolution: Resolution {
                    width: 2560,
                    height: 1440,
                },
                refresh_rate_mhz: 144_000,
            },
        ];
        let layout = Layout {
            outputs: vec![
                OutputConfig {
                    display_id: primary_id,
                    enabled: true,
                    position: Position { x: 0, y: 0 },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: true,
                },
                OutputConfig {
                    display_id: stale_id.clone(),
                    enabled: false,
                    position: Position { x: 1920, y: 0 },
                    resolution: Resolution {
                        width: 2560,
                        height: 1440,
                    },
                    refresh_rate_mhz: 144_000,
                    primary: false,
                },
                OutputConfig {
                    display_id: fresh_id.clone(),
                    enabled: true,
                    position: Position { x: 1920, y: 0 },
                    resolution: Resolution {
                        width: 2560,
                        height: 1440,
                    },
                    refresh_rate_mhz: 144_000,
                    primary: false,
                },
            ],
        };

        (displays, layout, stale_id, fresh_id)
    }

    fn profile_output(display_id: DisplayId, x: i32, primary: bool) -> OutputConfig {
        OutputConfig {
            display_id,
            enabled: true,
            position: Position { x, y: 0 },
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            refresh_rate_mhz: 60_000,
            primary,
        }
    }

    #[test]
    fn apply_profile_prefers_active_candidate_among_duplicate_edid_entries() {
        let (displays, layout, stale_id, fresh_id) = duplicate_edid_current_state();
        let backend = MockBackend::new(displays, layout).unwrap();
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "dual".to_string(),
                layout: Layout {
                    outputs: vec![
                        profile_output(
                            DisplayId {
                                adapter_luid: 5,
                                target_id: 1,
                                edid_hash: Some(1),
                            },
                            0,
                            true,
                        ),
                        profile_output(
                            DisplayId {
                                adapter_luid: 5,
                                target_id: 2,
                                edid_hash: Some(2),
                            },
                            1920,
                            false,
                        ),
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager.apply_profile("dual").unwrap();

        let applied = backend.current_layout().unwrap();
        let secondary = applied
            .outputs
            .iter()
            .find(|output| output.display_id.edid_hash == Some(2))
            .expect("expected remapped secondary output");
        assert_eq!(secondary.display_id, fresh_id);
        assert!(secondary.enabled);
        assert!(!applied
            .outputs
            .iter()
            .any(|output| output.display_id == stale_id));
    }

    #[test]
    fn apply_profile_hashless_fallback_does_not_guess_across_adapters() {
        // Two candidates share target_id 2 but live on different adapters (stale LUID entry vs
        // fresh one). The hash-less fallback must not guess between them: iGPU/dGPU pairs reuse
        // target id numbering, so a cross-adapter pick could hit the wrong physical monitor.
        let (displays, layout, _, _) = duplicate_edid_current_state();
        let backend = MockBackend::new(displays, layout).unwrap();
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "legacy".to_string(),
                layout: Layout {
                    outputs: vec![
                        profile_output(
                            DisplayId {
                                adapter_luid: 5,
                                target_id: 1,
                                edid_hash: None,
                            },
                            0,
                            true,
                        ),
                        profile_output(
                            DisplayId {
                                adapter_luid: 5,
                                target_id: 2,
                                edid_hash: None,
                            },
                            1920,
                            false,
                        ),
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend, store).unwrap();

        let err = manager.apply_profile("legacy").unwrap_err();
        assert!(matches!(
            err,
            ManagerError::Validation(message)
                if message.contains("is not connected right now")
        ));
    }

    #[test]
    fn apply_profile_still_bails_for_two_active_twin_candidates() {
        let primary_id = DisplayId {
            adapter_luid: 9,
            target_id: 1,
            edid_hash: Some(1),
        };
        let twin_left_id = DisplayId {
            adapter_luid: 9,
            target_id: 2,
            edid_hash: Some(7),
        };
        let twin_right_id = DisplayId {
            adapter_luid: 9,
            target_id: 3,
            edid_hash: Some(7),
        };
        let twin_display = |id: &DisplayId| DisplayInfo {
            id: id.clone(),
            friendly_name: "Twin".to_string(),
            is_active: true,
            is_primary: false,
            resolution: Resolution {
                width: 2560,
                height: 1440,
            },
            refresh_rate_mhz: 144_000,
        };
        let displays = vec![
            DisplayInfo {
                id: primary_id.clone(),
                friendly_name: "Primary".to_string(),
                is_active: true,
                is_primary: true,
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            },
            twin_display(&twin_left_id),
            twin_display(&twin_right_id),
        ];
        let twin_output = |id: &DisplayId, x: i32| OutputConfig {
            display_id: id.clone(),
            enabled: true,
            position: Position { x, y: 0 },
            resolution: Resolution {
                width: 2560,
                height: 1440,
            },
            refresh_rate_mhz: 144_000,
            primary: false,
        };
        let layout = Layout {
            outputs: vec![
                OutputConfig {
                    display_id: primary_id,
                    enabled: true,
                    position: Position { x: 0, y: 0 },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: true,
                },
                twin_output(&twin_left_id, 1920),
                twin_output(&twin_right_id, 4480),
            ],
        };
        let backend = MockBackend::new(displays, layout).unwrap();
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "twins".to_string(),
                layout: Layout {
                    outputs: vec![
                        profile_output(
                            DisplayId {
                                adapter_luid: 5,
                                target_id: 1,
                                edid_hash: Some(1),
                            },
                            0,
                            true,
                        ),
                        profile_output(
                            DisplayId {
                                adapter_luid: 5,
                                target_id: 9,
                                edid_hash: Some(7),
                            },
                            1920,
                            false,
                        ),
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend, store).unwrap();

        let err = manager.apply_profile("twins").unwrap_err();
        assert!(matches!(
            err,
            ManagerError::Validation(message)
                if message.contains("is not connected right now")
        ));
    }

    #[test]
    fn apply_profile_hashless_legacy_output_remaps_to_seeded_inactive_display() {
        // Field case (Guido's TV): a legacy profile entry saved without edid_hash by an old
        // build, while the detached TV exists only as an ALL_PATHS-seeded inactive display
        // under the current adapter LUID. The hash-less target_id fallback must accept the
        // seeded candidate (Some(hash) on the candidate, None on the request) and remap.
        let primary_id = DisplayId {
            adapter_luid: 9,
            target_id: 1,
            edid_hash: Some(1),
        };
        let seeded_tv_id = DisplayId {
            adapter_luid: 9,
            target_id: 4352,
            edid_hash: Some(77),
        };
        let displays = vec![
            DisplayInfo {
                id: primary_id.clone(),
                friendly_name: "Primary".to_string(),
                is_active: true,
                is_primary: true,
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            },
            DisplayInfo {
                id: seeded_tv_id.clone(),
                friendly_name: "TV".to_string(),
                is_active: false,
                is_primary: false,
                resolution: Resolution {
                    width: 0,
                    height: 0,
                },
                refresh_rate_mhz: 60_000,
            },
        ];
        let layout = Layout {
            outputs: vec![
                OutputConfig {
                    display_id: primary_id,
                    enabled: true,
                    position: Position { x: 0, y: 0 },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: true,
                },
                OutputConfig {
                    display_id: seeded_tv_id.clone(),
                    enabled: false,
                    position: Position { x: 0, y: 0 },
                    resolution: Resolution {
                        width: 0,
                        height: 0,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: false,
                },
            ],
        };
        let backend = MockBackend::new(displays, layout).unwrap();
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "couch".to_string(),
                layout: Layout {
                    outputs: vec![
                        profile_output(
                            DisplayId {
                                adapter_luid: 1,
                                target_id: 1,
                                edid_hash: None,
                            },
                            0,
                            true,
                        ),
                        profile_output(
                            DisplayId {
                                adapter_luid: 1,
                                target_id: 4352,
                                edid_hash: None,
                            },
                            1920,
                            false,
                        ),
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager.apply_profile("couch").unwrap();

        let applied = backend.current_layout().unwrap();
        let tv = applied
            .outputs
            .iter()
            .find(|output| output.display_id == seeded_tv_id)
            .expect("expected TV output remapped to the seeded display");
        assert!(tv.enabled);
    }

    #[test]
    fn apply_profile_does_not_touch_topology_when_display_is_not_connected() {
        // Field case: after a resume, Windows reported one of the monitors with
        // targetAvailable=FALSE, so it was not enumerated at all. Applying a profile that wants
        // it must NOT change the topology — the old code forced a topology extend here, which
        // attached every connected-inactive display (including a TV the very same profile asks to
        // detach), stalled 3.5s and failed anyway. An absent display is not an attach failure.
        let backend =
            CountingBackend::new(MockBackend::new(sample_displays(), sample_layout()).unwrap());
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "PC".to_string(),
                layout: Layout {
                    outputs: vec![
                        profile_output(sample_display_id(1), 0, true),
                        // A monitor Windows is not enumerating right now.
                        profile_output(
                            DisplayId {
                                adapter_luid: 1,
                                target_id: 4353,
                                edid_hash: Some(0xa8c7_f832_281a_39c5),
                            },
                            1920,
                            false,
                        ),
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        let err = manager.apply_profile("PC").unwrap_err();
        assert!(
            matches!(&err, ManagerError::Validation(message) if message.contains("is not connected right now")),
            "expected the not-connected diagnosis, got: {err}"
        );
        assert_eq!(
            backend.apply_calls(),
            0,
            "an absent display must never trigger a topology change"
        );
        assert!(!manager.has_pending_confirmation());
    }

    #[test]
    fn apply_profile_calls_prepare_attach_targets_when_outputs_are_unresolved() {
        let backend =
            CountingBackend::new(MockBackend::new(sample_displays(), sample_layout()).unwrap());
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "ghost".to_string(),
                layout: Layout {
                    outputs: vec![
                        profile_output(sample_display_id(1), 0, true),
                        profile_output(
                            DisplayId {
                                adapter_luid: 1,
                                target_id: 9,
                                edid_hash: Some(99),
                            },
                            1920,
                            false,
                        ),
                    ],
                },
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        let err = manager.apply_profile("ghost").unwrap_err();
        assert!(matches!(
            err,
            ManagerError::Validation(message)
                if message.contains("is not connected right now")
        ));
        assert_eq!(backend.prepare_calls(), 1);
    }

    #[test]
    fn apply_profile_does_not_call_prepare_attach_targets_when_outputs_resolve() {
        let backend =
            CountingBackend::new(MockBackend::new(sample_displays(), sample_layout()).unwrap());
        let store = MemoryConfigStore::default();
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager.save_profile("dual").unwrap();
        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();

        manager.apply_profile("dual").unwrap();

        assert_eq!(backend.prepare_calls(), 0);
    }

    #[test]
    fn rollback_pending_keeps_pending_when_backend_apply_fails() {
        let backend =
            CountingBackend::new(MockBackend::new(sample_displays(), sample_layout()).unwrap());
        let store = MemoryConfigStore::default();
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        assert!(manager.has_pending_confirmation());

        backend.set_fail_apply(true);
        assert!(manager.rollback_pending().is_err());
        assert!(manager.has_pending_confirmation());

        backend.set_fail_apply(false);
        manager.rollback_pending().unwrap();
        assert!(!manager.has_pending_confirmation());
    }

    #[test]
    fn toggle_display_remaps_stale_adapter_luid_by_target_id() {
        let backend =
            MockBackend::new(sample_displays_on_adapter(9), sample_layout_on_adapter(9)).unwrap();
        let store = MemoryConfigStore::default();
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager
            .toggle_display(&sample_display_id_on_adapter(1, 2))
            .unwrap();

        let layout = backend.current_layout().unwrap();
        let output = layout
            .outputs
            .iter()
            .find(|output| output.display_id == sample_display_id_on_adapter(9, 2))
            .expect("expected remapped target");
        assert!(!output.enabled);
    }

    #[test]
    fn delete_profile_removes_saved_profile() {
        let (mut manager, _, _) = build_manager();
        manager.save_profile("dual").unwrap();
        manager.delete_profile("dual").unwrap();
        assert!(manager.list_profiles().is_empty());
    }

    #[test]
    fn restore_last_layout_restores_previous_confirmed_layout() {
        let (mut manager, backend, _) = build_manager();
        let original = backend.current_layout().unwrap();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();
        assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 1);

        manager.restore_last_layout().unwrap();
        assert_eq!(backend.current_layout().unwrap(), original);
    }

    #[test]
    fn expired_confirmation_triggers_auto_rollback() {
        let (mut manager, backend, _) = build_manager();
        let original = backend.current_layout().unwrap();

        manager.set_confirmation_timeout(Duration::ZERO);
        manager.toggle_display(&sample_display_id(2)).unwrap();

        let rolled_back = manager.rollback_if_confirmation_expired().unwrap();
        assert!(rolled_back);
        assert_eq!(backend.current_layout().unwrap(), original);
        assert!(!manager.has_pending_confirmation());
    }
}
