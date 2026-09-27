# Changelog

## Unreleased

### Display recovery and identity

- Discover connected, disabled monitors from fresh Windows inventory, including after sequential detach operations, restart, and resume.
- Reconnect the exact requested monitor set using a complete source assignment and verify the observed result before reporting success.
- Use a shared identity resolver for profiles, toggles, recovery, and remembered geometry. Validated EDID serials can identify a monitor after moving ports; device paths and connection fingerprints provide fallback evidence. Ambiguous matches and conflicting serials fail explicitly.
- Save and verify display rotation.
- Reject duplicate targets, invalid geometry, and unsupported cloned or overlapping layouts before changing displays. Profile restoration supports extended desktops.
- Replace persisted native Win32 byte snapshots with validated geometry preferences. Native attachment routes always come from a fresh query; old `topology_snapshot.json` files are ignored.

### Recovery and settings reliability

- Persist recovery intent before changing displays. Failed applies remain eligible for rollback, and unfinished transactions are recovered when Monarch restarts.
- Keep the recovery journal until restoration and configuration persistence both succeed. Retry transient recovery failures and report exhausted retries without losing manual recovery.
- Save configuration through a flushed temporary file and atomic replacement, retaining a valid backup. Failed profile/settings writes preserve the previous live state.
- Remove profile/configuration migrations. Outdated, malformed or unsupported configuration deletes `config.json` and its backup and starts Monarch with default settings and no saved profiles. Valid current-format profiles and recovery journals are retained, including when a monitor is disconnected.
- Use numeric Windows registry status codes for startup registration, fixing missing-value errors on non-English Windows (#45). Unchanged startup settings no longer trigger a registry write on every settings save.
- Preserve slideshow mode, options, disabled backgrounds, placement and color around display operations, avoiding unnecessary wallpaper resets (#36). Balance COM initialization on failure and release interfaces before apartment shutdown.
- Preserve custom shortcut bindings and stop generating a second copy of indexed shortcut mappings. Monitor shortcut order uses identity evidence instead of friendly names.

### Responsiveness and maintenance

- Serialize display operations through a bounded worker queue. UI and tray reads use published snapshots; recovery has priority over queued changes. Refresh notifications are coalesced and unchanged hotkeys stay registered.
- Reopen the running app from its taskbar shortcut. Use a user/session-scoped Windows named pipe with a user-only access rule instead of a global TCP port. Separate request acceptance from completion and report uncertain outcomes accurately.
- Show snapshot/registration failures in the app instead of retaining an unused error state.
- Dispose event subscriptions that finish registering after UI cleanup. Separate browser simulation from the Tauri transport and exercise confirmation, rollback and timeout behavior in regression tests.
- Remove obsolete backend stubs, native snapshot serialization, attachment-candidate code, unused Tauri read commands and unused frontend packages.
- Align the locked Tauri runtime dependencies. Version bumps update both Cargo lockfiles, release builds use locked dependencies, and release tags are created after successful build validation.

### Verification before release

Run `cargo test --locked`, `cargo test --locked --manifest-path src-tauri/Cargo.toml`, `yarn test`, and `yarn build`.

Windows hardware acceptance must cover three-or-more-display switching, consecutive detach/reattach, restart during confirmation, sleep/hibernate, port/dock changes, identical monitors, mixed GPUs, portrait layouts, slideshow/disabled backgrounds, non-English Windows startup settings, and two logged-in sessions. Check active monitors, placement, primary, rotation, resolution, refresh rate and UI responsiveness. Record Windows version, GPU/driver, display models and diagnostics.

The desktop suite includes an ignored test that temporarily disables two secondary monitors, creates a new backend and restores the original desktop:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib restores_two_detached_monitors_after_cold_start_with_incomplete_cache -- --ignored --nocapture --test-threads=1
```

Run that test explicitly on a local Windows desktop with at least three active monitors. It attempts to restore the original layout even if an assertion fails. Automated source checks and mock tests do not replace these hardware checks.
