# Windows hardware acceptance: display profiles and settings

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
| Fresh 2.0 configuration | Start with 1.x settings/profiles and geometry history. Confirm they are not imported or used for recovery; create new profiles from the current desktop. | Unverified |
| Monitor settings mode changes | Change only resolution, only refresh (including a fractional rate), then both from Settings. Each supported request must reach confirmation without error 87; confirm, revert and let the timer expire on separate attempts. | Unverified |
| Duplicate settings from primary | Duplicate a display using the primary's supported mode while leaving another monitor extended. Confirm, return to Extend, then repeat and revert. Incompatible copied modes must ask for an explicit compatible choice. | Unverified |
| Layout draft | Drag monitors in the preview (including the primary and a clone group), including drops in empty space. The final position snaps to an adjoining edge; remaining gaps/overlaps prevent saving. Discard must leave Windows unchanged. Save layout must change only positions and preserve exact working modes. Exercise Confirm, Revert and timeout. | Unverified |
| Monitor settings | Open Settings beside Attach/Detach. Change properties and primary status, then Cancel: Windows stays unchanged. Save settings applies directly through confirmation. Leave a position draft in the preview while saving settings: the position draft must remain unapplied. Changing resolution/rotation keeps neighbouring display edges joined. | Unverified |
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
| Existing entry points | Repeat compatible profile selection through UI, tray, global shortcuts and `--profile`. Auto-confirm occurs only after successful verification; failure remains recoverable. | Unverified |

The existing ignored Windows hardware test can explicitly detach and restore two
monitors on a local console with at least three active monitors:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml restores_two_detached_monitors_after_cold_start_with_incomplete_cache -- --ignored --nocapture
```
