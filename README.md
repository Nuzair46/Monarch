<center>
  <h1 align="center">Monarch</h1>
  <h4 align="center">Detach, restore, and switch monitor layouts without touching cables.</h4>
  <h5 align="center">Built for fast display switching, standby behavior, and safe rollback if something goes wrong</h5>
  <p align="center">
    <a href="https://github.com/Nuzair46/Monarch/releases">
      <img src="src-tauri/icons/icon.png" alt="Monarch logo" width="180" />
    </a>
  </p>
</center>

<p align="center">
  <a href="https://github.com/Nuzair46/Monarch/actions/workflows/ci-build-release.yml"><img alt="Release Build and Publish" src="https://github.com/Nuzair46/Monarch/actions/workflows/ci-build-release.yml/badge.svg" /></a>
  <img alt="Downloads" src="https://img.shields.io/github/downloads/Nuzair46/Monarch/total.svg" />
  <img alt="Latest Release" src="https://img.shields.io/github/v/release/Nuzair46/Monarch?display_name=tag" />
  <img alt="Platform" src="https://img.shields.io/badge/Platform-Windows%2010%2F11-0078D4?logo=windows&logoColor=white" />
</p>

<p align="center">
  <a href="https://github.com/Nuzair46/Monarch/releases"><strong>Download Latest Release</strong></a>
  ·
  <a href="#quick-start"><strong>Quick Start</strong></a>
  ·
  <a href="#if-something-goes-wrong"><strong>Recovery</strong></a>
</p>

## What Is Monarch?

Monarch lets you:

- Detach a monitor in software (no cable unplugging)
- Reattach it later
- Edit resolution, fractional refresh, orientation, HDR and supported scaling
- Extend, duplicate or detach displays, including a duplicated pair beside an extended monitor
- Save display layouts with an optional audio output per profile
- Switch the Windows playback output from the main page
- Restore the previous layout quickly
- Recover automatically with a confirmation timeout if a layout change goes wrong
- Easy apply with hotkeys

It uses Windows display topology APIs (`DisplayConfig`) to change which outputs are active.

## Download & Install (End Users)

1. Open the [Releases page](https://github.com/Nuzair46/Monarch/releases)
2. Download the latest `.msi` installer
3. Run the installer
4. Launch `Monarch` from Start Menu or Desktop

## Quick Start

1. Open `Monarch`
2. In the `Monitors` section, click `Detach` on the display you want to turn off
3. Confirm the layout change (or it auto-rolls back)
4. Click `Attach` later to bring the display back
5. Use `Save Current Layout` in `Profiles` to store common setups

## Editing displays

Monarch 2.0 starts with a fresh configuration. Settings and profiles from 1.x are
not supported or migrated; recreate profiles from your current desktop.

Drag monitors in **Layout Preview** to arrange them. Dropped monitors snap to an
adjoining edge; gaps and overlaps must be resolved before **Save layout**. This
button applies positions only. **Discard changes** restores the preview without
changing Windows. Arrow keys move a focused monitor; Shift makes larger adjustments.

Choose **Settings** beside a monitor's Attach/Detach button to change resolution,
refresh rate, orientation, scaling, HDR, duplication or the primary display.
**Save settings** applies directly from that dialog; **Cancel** discards its changes.
It does not apply unsaved positions from the preview. Both save actions use the
confirmation timer and restore captured settings if you revert or time out.

Mode lists come from Windows. Unsupported HDR or scaling controls explain why they
are unavailable. Detached or duplicated monitors may expose only their preferred
and observed modes; extend the monitor first to enumerate additional modes.
**Duplicate of…** mirrors the selected display's desktop. Monarch chooses a shared
resolution and scaling supported by all monitors in the group. Each monitor keeps
its own refresh rate when supported at that resolution; otherwise a supported rate
is selected for that monitor. Orientation and HDR remain per monitor. Review the
shared settings and each monitor's refresh rate in the dialog before saving.
If Windows rejects the combination, choose compatible settings explicitly. **Extend** places the member
beside the remaining desktop. **Detached** keeps the other group members active.
Profiles save the current confirmed layout and can be applied or deleted. Their
audio output can be changed independently without recapturing the display layout.

Real-device verification scenarios are listed in the [Windows hardware checklist](docs/windows-hardware-checklist.md).

## Audio output and profiles

Use **Audio output → Playback device → Switch output** on the main page to change
Windows' default system/media playback output immediately.

In **Profiles**, choose an **Audio output** when saving a layout, or change the
output beneath an existing profile and choose **Save audio**. Saving does not change
active displays or audio. **Leave unchanged** lets Windows manage audio when that
profile is applied. Microphones, communications preferences, volume, and per-app
routing are not changed by the output selector. Apps configured to use a specific
device may keep using it.

For a desk/TV setup, save a desk profile with your speakers and a TV profile with
its HDMI output. Apply either profile from the app, tray, shortcut, or CLI. Monarch
changes displays first, waits up to five seconds for the selected audio endpoint,
and verifies the default output before starting confirmation. Revert, timeout, and
restart recovery restore the captured display settings and previous playback roles.
The manual output switch and tray/shortcut/CLI profile actions confirm automatically.

Currently unavailable outputs are selectable for profiles, so a detached TV can
be configured in advance. If an output does not become available, the profile
change is rolled back with an error; it is never substituted by another similarly
named device. Turn on/reconnect or enable the device in Windows Sound settings and
retry. If a driver reinstall changes its endpoint ID, select the output again in
Monarch. Removing the profile's audio choice remains possible when audio support
is unavailable. Display-only operations remain available if the audio service fails;
audio cannot be captured for recovery in that case.

## Command-Line Profile Switch (Automation)

You can launch Monarch and ask it to apply a specific profile immediately:

```powershell
monarch-desktop.exe -profile "ProfileName"
```

Also supported:

```powershell
monarch-desktop.exe --profile "ProfileName"
monarch-desktop.exe --profile="ProfileName"
```

Notes:

- Useful for tools like Playnite scripts (before/after game launch)
- If Monarch is already running, the new command forwards the profile request to the running instance
- CLI profile argument takes precedence over the configured launch profile in Settings

## Safety Features

- Confirmation timer after layout changes
- Automatic rollback if you do not confirm in time
- `Restore Last Layout` action
- Prevents disabling the last active display

## If Something Goes Wrong

Try these in order:

1. Use Monarch tray menu: `Restore Displays`
2. Reopen Monarch and use `Restore Last Layout`
3. Use Windows shortcut `Win + P` and choose `Extend` or `PC screen only`
4. Reboot Windows (usually restores a usable display state)

## Notes (Important)

- Windows only
- Monarch changes display topology, not monitor power directly
- Most monitors enter standby when Windows stops sending signal
- If you change HDR/SDR mode in Windows, Monarch auto-reapplies calibration in the background (best effort)

## Troubleshooting

### The app opens but I can't see the window

- Check the system tray for the Monarch icon
- Double-click the tray icon or use `Open App`

### A layout change made the screen unusable

- Wait for the confirmation timer to expire (auto rollback)
- Or use `Win + P`

### My display arrangement in the UI looks outdated

- Refocus the app window (Monarch auto-refreshes)
- Wait a few seconds for the background refresh poll to update the layout

### Color calibration looks wrong after detaching a display

- Known issue on some systems with custom calibration (ICC / SDR / HDR calibration profiles)
- In testing, this can be triggered when:
  - a display is detached in Monarch, and then
  - Windows `Settings > System > Display` is opened
- The detach itself may look fine until Windows Display Settings is opened
- Workaround: reattach the detached display (this often restores the remaining display calibration)
- If needed, also reapply your calibration using your normal calibration tool / workflow

## FAQ

### Does Monarch physically power off the monitor?

No. It detaches the display output in Windows. Many monitors then enter standby automatically.

### Is it safe to test?

Yes, but test on a non-critical setup first. Monarch includes rollback protection, and `Win + P` / reboot are reliable fallbacks.

### Can I use it with NVIDIA / AMD / Intel?

Yes. Monarch is designed to work through Windows display APIs, not vendor-specific GPU control panels.

### Is color calibration perfectly preserved in every Windows display-settings scenario?

Not yet. Monarch handles many calibration cases (including common HDR/SDR transitions), but Windows Display Settings can still cause calibration resets on some systems after topology changes. See `Troubleshooting` for the current known issue and workaround.

## For Developers

<details>
  <summary>Build / Dev / CI details</summary>

### Project Layout

- `src/` Rust core library (layouts, profiles, rollback safety, persistence)
- `src-tauri/` Tauri desktop app + Windows backend
- `web/` React UI
- `.github/workflows/` Windows release workflow

### Build Locally (Windows)

Requirements:

- Node.js 20+
- `yarn`
- Rust (stable)
- Visual Studio Build Tools 2022 + Windows SDK (`rc.exe`)

Commands:

```bash
yarn install
rustup target add x86_64-pc-windows-msvc
yarn tauri dev
```

Build MSI:

```bash
yarn tauri build --bundles msi
```

Output:

- `src-tauri/target/release/bundle/msi/`

### CI / Release

- Workflow: `.github/workflows/ci-build-release.yml`
- Manual release workflow runs via `workflow_dispatch` and takes a version input
- Release pipeline updates these files together before building:
  - `Cargo.toml`
  - `src-tauri/Cargo.toml`
  - `package.json`
  - `src-tauri/tauri.conf.json`
- Release pipeline commits the version bump, creates tag `vX.Y.Z`, builds the Windows installer, and publishes the GitHub Release

Release process:

1. Make sure your release commit is on `main`.
2. Open `Actions` -> `Release Build and Publish` -> `Run workflow`.
3. Enter a version (example: `0.2.0`) or bump kind (`patch`, `minor`, `major`).
4. Run the workflow.
5. The workflow will bump all version files, commit the change, create the tag, build Windows artifacts, and publish the GitHub Release.

  </details>
