# Windows hardware acceptance: display profiles and cursor alignment

Status: **not exercised on physical Windows displays in this implementation environment**.
Hosted Windows CI compiles and tests the EXE/MSI and pure/native planning code; it is
not evidence that these hardware scenarios passed. Record Windows version, GPU/driver,
monitor models/ports, observed results and logs alongside each completed scenario.
Issue #8 is deferred. This PR prepares version 2.0.0; it does not publish a release.

Before testing, save the baseline as a profile and record its placement, resolution,
refresh, rotation, HDR, scaling, primary source, wallpaper and color calibration.
Keep Windows Display Settings available for comparison.

| Scenario | Procedure and expected result | Status |
| --- | --- | --- |
| Fresh 2.0 configuration | Start with 1.x settings/profiles and geometry history. Confirm they are not imported or used for recovery; create new profiles from the current desktop. Cursor alignment starts off. | Unverified |
| Layout draft | Drag monitors in the preview (including the primary and a clone group), edit one monitor through the list, and Discard. Windows must remain unchanged. Repeat, Save layout, and exercise Confirm, Revert and timeout. Profiles only save/apply/delete. | Unverified |
| Independent mode controls | Change resolution while preserving the selected refresh rate. If unsupported, require an explicit compatible rate before saving. Check fractional rates. | Unverified |
| Mixed modes | Use real 1080p/1440p/4K panels, including 59.94/119.88 Hz and all four orientations. Apply and Confirm; compare observed Windows settings. | Unverified |
| Capability boundaries | Test SDR-only and HDR panels, an older Windows HDR API, unavailable DPI queries and custom global scaling. Unsupported controls explain the limitation; invalid requests do not change displays. | Unverified |
| Scaling | Apply supported standard percentages at several resolutions. Verify text scale in Windows, profile switching, Confirm, Revert and timeout. No custom/global scaling is changed. | Unverified |
| HDR recovery | Change modes and HDR together. Exercise Confirm, manual Revert and timeout. Restore the exact original HDR preference, plus SDR gamma/ICC calibration and wallpaper. | Unverified |
| Duplication | Duplicate a pair while keeping a third display extended. Verify shared position/resolution/scaling and logical primary status; save and reapply. Try incompatible rotations, refresh rates and HDR combinations: reject explicitly without reducing requested settings. | Unverified |
| Clone transitions | Split a member to Extend and check placement beside the desktop. Detach either member; the other stays active and a singleton loses its group. Reattach, clone again, change primary source, then revert. Last-active protection remains. | Unverified |
| Routing | Exercise displays on separate adapters/docks and a topology without a compatible common source. The unsupported grouping is rejected; existing displays remain recoverable. | Unverified |
| Hot-plug | Unplug a referenced display before Apply and during confirmation. Saved settings remain intact, missing identity errors are actionable, and recovery stays pending until the monitor can be restored. Test reconnect on another dock/port. | Unverified |
| Restart recovery | Apply a different layout and terminate Monarch before confirming. Restart; topology, clone membership, modes, HDR and scaling return to the journaled layout before ordinary queued actions. | Unverified |
| Failure recovery | Use a driver-rejected combination or disconnect while applying. Confirm successful rollback restores every captured property. If hardware prevents rollback, verify the durable journal is retained and manual retry works after reconnect. | Unverified |
| Wallpaper/calibration | Repeat apply/rollback with static wallpaper, slideshow, disabled wallpaper and custom SDR calibration. Confirm the previous background mode and calibration survive. | Unverified |
| Cursor ultrawides | Use 2560×1080 and 3440×1440 at 100% scaling with either monitor primary. Confirm physical sizes, enable alignment and cross both directions at several heights, including the Windows pixel-edge dead end. The active status and corrected-crossings counter must update; Ctrl must temporarily restore native crossing. For equal-height panels y=540 maps to y=720. | Unverified |
| Cursor setup failures | Create a physical gap or overlap, omit dimensions, or select the other clone representative. Verify the UI explains missing usable boundaries and does not silently claim correction is active. Restore adjoining edges and check the counter. | Unverified |
| Cursor mixed sizes | Calibrate two or three physically different panels at mixed pixel resolutions and 100/150/200% scaling. Cross at several physical heights. Interior pointer speed/acceleration remains unchanged. | Unverified |
| Cursor arrangements | Test monitors left/above the primary, negative pixel coordinates, staggered physical offsets and both portrait orientations. Include fast crossings across multiple boundaries and a Windows pixel-edge dead end. | Unverified |
| Cursor clones | Test the default stable representative and switch representatives for a duplicate group. Exactly one cursor surface represents that group. | Unverified |
| Cursor bypass | Hold Ctrl, use injected mouse input, run an application that clips the cursor, lock/unlock, and switch desktops/UAC. Input passes through when correction is unavailable, with no correction loops. | Unverified |
| Cursor lifecycle | Disable via Settings and tray; confirm hook removal. Apply, timeout, rollback, hot-plug and resume must suspend stale maps and rebuild them. Exit must unregister the hook. Profiles must not overwrite calibration. | Unverified |
| Existing entry points | Repeat compatible profile selection through UI, tray, global shortcuts and `--profile`. Auto-confirm occurs only after successful verification; failure remains recoverable. | Unverified |

The existing ignored Windows hardware test can explicitly detach and restore two
monitors on a local console with at least three active monitors:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml restores_two_detached_monitors_after_cold_start_with_incomplete_cache -- --ignored --nocapture
```
