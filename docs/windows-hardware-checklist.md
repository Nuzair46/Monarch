# Windows hardware acceptance: displays and audio

Status: **not exercised on physical Windows displays in this implementation environment**.
Hosted Windows CI compiles and tests the EXE/MSI and pure/native planning code; it is
not evidence that these hardware scenarios passed. Record Windows version, GPU/driver,
monitor models/ports, observed results and logs alongside each completed scenario.
Issue #8 is deferred. These checks do not publish a release.

Before testing, save the baseline as a profile and record its placement, resolution,
refresh, rotation, HDR, scaling, primary source, wallpaper and color calibration.
Keep Windows Display Settings available for comparison.

| Scenario | Procedure and expected result | Status |
| --- | --- | --- |
| Fresh 2.0 configuration | Start with 1.x settings/profiles and geometry history. Confirm they are not imported or used for recovery; create new profiles from the current desktop. | Unverified |
| Monitor settings mode changes | Change only resolution, only refresh (including a fractional rate), then both from Settings. Each supported request must reach confirmation without error 87; confirm, revert and let the timer expire on separate attempts. | Unverified |
| Duplicate desktop content | Duplicate monitors with different native resolutions, refresh rates, HDR support and scaling ranges while leaving another monitor extended. The dialog must offer common resolution/scaling and separate supported refresh rates. Save and verify both group members show the same desktop content, while the extended monitor remains independent. Confirm, return to Extend, then repeat and revert. | Unverified |
| Layout draft | Drag monitors in the preview (including the primary and a clone group), including drops in empty space. The final position snaps to an adjoining edge; remaining gaps/overlaps prevent saving. Discard must leave Windows unchanged. Save layout must change only positions and preserve exact working modes. Exercise Confirm, Revert and timeout. | Unverified |
| Monitor settings | Open Settings beside Attach/Detach. Change properties and primary status, then Cancel: Windows stays unchanged. Save settings applies directly through confirmation. Leave a position draft in the preview while saving settings: the position draft must remain unapplied. Changing resolution/rotation keeps neighbouring display edges joined. | Unverified |
| Independent mode controls | Change resolution while preserving the selected refresh rate. If unsupported, require an explicit compatible rate before saving. Check fractional rates. | Unverified |
| Mixed modes | Use real 1080p/1440p/4K panels, including 59.94/119.88 Hz and all four orientations. Apply and Confirm; compare observed Windows settings. | Unverified |
| Portrait geometry (#64) | Set an extended monitor to 1080×1920 in Windows, including Portrait and Portrait (flipped), beside a landscape primary. Check the monitor list and preview after refocusing Monarch and after restarting it. Repeat with a natively tall display in Landscape when available, negative offsets and mixed DPI. Drag and save the arrangement: visible edges and mouse transitions must match Windows. Save/reapply a profile, detach/reattach, and exercise Revert and timeout; geometry, orientation and refresh must be preserved. | Unverified |
| Capability boundaries | Test SDR-only and HDR panels, an older Windows HDR API, unavailable DPI queries and custom global scaling. Unsupported controls explain the limitation; invalid requests do not change displays. | Unverified |
| Scaling | Apply supported standard percentages at several resolutions. Verify text scale in Windows, profile switching, Confirm, Revert and timeout. No custom/global scaling is changed. | Unverified |
| HDR recovery | Change modes and HDR together. Exercise Confirm, manual Revert and timeout. Restore the exact original HDR preference, plus SDR gamma/ICC calibration and wallpaper. | Unverified |
| Duplication | Duplicate a pair while keeping a third display extended. Verify shared position/resolution/scaling and logical primary status; save and reapply. Try incompatible rotations, refresh rates and HDR combinations: reject explicitly without reducing requested settings. | Unverified |
| Clone transitions | Split a member to Extend and check placement beside the desktop. Detach either member; the other stays active and a singleton loses its group. Reattach, clone again, change primary source, then revert. Last-active protection remains. | Unverified |
| Routing | Exercise displays on separate adapters/docks and a topology without a compatible common source. The unsupported grouping is rejected; existing displays remain recoverable. | Unverified |
| Manual reattach (#59) | With three or more monitors, detach and confirm each non-primary display in turn, then use its Attach button and confirm. Repeat through tray/hotkeys, after restarting Monarch, and on different GPU ports. Repeat with non-preferred resolution/fractional refresh, portrait and offset/stacked arrangements. The requested display returns with its remembered mode; a stale position joins a free desktop edge, active displays stay unchanged, and other detached displays remain off. Compare with applying a saved profile. | Unverified |
| Reattach recovery/errors (#59) | Attach, then Revert; repeat and let confirmation expire. The prior active set and settings return. Try a genuinely rejected mode: Windows must reject it without silently lowering resolution/refresh. The operation error remains visible through background refresh and appears in the diagnostic log. | Unverified |
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

## Audio acceptance — issues #28 and #34

Record the initial console, multimedia, and communications playback devices
separately, plus microphone defaults, volume, and any per-app device assignments.
Use Windows Sound settings and a player following the Windows default output to
verify audible output. All scenarios below remain **unverified on real hardware**.

| Scenario | Procedure and expected result | Status |
| --- | --- | --- |
| Current output readout | Change playback devices in Windows Sound settings. The Layout Preview header updates to the observed device. Apply, confirm, and revert profiles and verify that this label follows the resulting output. No output selector appears on the main page. | Unverified |
| Profile save | Choose audio for a new profile. Change only an existing profile's audio and Save audio; verify its saved display layout and the active system remain unchanged. Cancel a draft. Available and unavailable outputs appear in separate alphabetical groups; inactive HDMI outputs remain selectable for profiles. | Unverified |
| Desk ↔ TV | Start with the TV detached. Save/select its inactive HDMI endpoint and apply a TV profile. Verify the screen activates before audio switches and the full confirmation countdown starts afterward. Switch back to speakers. | Unverified |
| Confirmation and timeout | Apply a combined display/audio profile, then Revert; repeat and let confirmation expire. All original playback roles and display properties return, including a separate communications device. | Unverified |
| Disconnected device | Unplug/disable the selected USB/HDMI output before Apply. After the bounded wait, verify an actionable error, prior display/audio recovery, and no silent fallback to a similarly named endpoint. Saved preference remains. | Unverified |
| HDMI timing and hot-plug | Enable a cold HDMI TV/AV receiver, unplug during apply and confirmation, and reconnect. If its endpoint does not appear within five seconds, recovery is explicit; repeat after the device is ready. | Unverified |
| Recovery failure and restart | Unplug the original audio device before rollback. The recovery journal remains. Restart Monarch, reconnect the device, and retry Revert; the original playback roles return and journals clear only on success. | Unverified |
| Crash after apply | Terminate Monarch after a combined profile succeeds but before confirming, then restart. Original display and audio preferences are restored. | Unverified |
| Confirmed restore | Confirm a profile, then Restore Last Layout. Both the previous display layout and associated audio defaults return. | Unverified |
| Identity | Connect two same-name endpoints. Profiles continue to use the selected ID across restart and ordinary reconnect. After a driver reinstall replaces an ID, require reselection rather than guessing by name. | Unverified |
| Audio service/support unavailable | Stop the audio service or use an environment without the PolicyConfig interface. Audio reports unavailable; display-only actions and clearing a profile audio choice remain usable. Audio-dependent apply must not mutate displays. | Unverified |
| Other entry points | Apply audio profiles using tray, hotkey, startup profile, and `--profile`. Each switches audio after displays and confirms only after verification. Existing instance CLI routing behaves the same. | Unverified |
| App routing | Compare an app following system defaults with one assigned a specific device. Only apps following the Windows default are expected to switch. | Unverified |
