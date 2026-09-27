# Changelog

## Unreleased

### Display recovery

The display-recovery changes under review address detached monitors being omitted from a restore:

- Discover connected, disabled monitors at startup, even when the saved connection cache is incomplete.
- Keep all saved connection paths through consecutive detach operations.
- Reconnect the exact requested monitor set using fresh Windows path data when cached paths fail.
- Verify the actual active monitor set before reporting success; restore the previous desktop on failure.
- Retain pending rollback information if recovery or saving configuration fails, with bounded automatic retries and an explicit failure message.
- Reopen the running app when its taskbar shortcut is launched after closing to the tray.

These changes still require real Windows hardware verification before release. Verify three-or-more-display profile switching, sequential detach/reattach, application restart, and sleep/hibernate. Check primary display, placement, resolution, refresh rate, and tray/window responsiveness. Record Windows version, GPU/driver, display models and the diagnostic log with the result.

Existing profile files remain compatible.

Run automated checks with `cargo test` and `cargo test --manifest-path src-tauri/Cargo.toml --lib`.
The desktop suite includes an ignored hardware test that temporarily disables two secondary
monitors, simulates an incomplete cache after restart, and restores the original desktop:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib restores_two_detached_monitors_after_cold_start_with_incomplete_cache -- --ignored --nocapture --test-threads=1
```

Run that test explicitly on a local Windows desktop with at least three active monitors.
It attempts to restore the original layout even if an assertion fails.
