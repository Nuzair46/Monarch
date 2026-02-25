import { getVersion as tauriGetVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import packageJson from "../package.json";

import type {
  AppSettings,
  AppSnapshot,
  ConfirmationEvent,
  Layout,
  Profile,
} from "./types";

type EventPayloadMap = {
  "monarch://state-changed": void;
  "monarch://confirmation": ConfirmationEvent;
};

type MockListener = (event: { payload: unknown }) => void;

export type ReleaseUpdateCheckResult = {
  currentVersion: string;
  latestVersion: string;
  latestTag: string;
  updateAvailable: boolean;
  releaseUrl: string;
};

const mockListeners = new Map<string, Set<MockListener>>();
const viteEnv = (import.meta as ImportMeta & { env?: Record<string, string | undefined> }).env;
const useWebMock =
  (viteEnv?.VITE_MONARCH_WEB_MOCK ?? "") === "1" || !isTauriRuntime();
const GITHUB_RELEASES_LATEST_API = "https://api.github.com/repos/Nuzair46/Monarch/releases/latest";
const GITHUB_RELEASES_URL = "https://github.com/Nuzair46/Monarch/releases";

let mockState = buildMockSnapshot();
let mockRestorableLayout = cloneLayout(mockState.layout);

function isTauriRuntime(): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  return typeof (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined";
}

function deepClone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function cloneLayout(layout: Layout): Layout {
  return deepClone(layout);
}

function normalizeVersion(version: string): string {
  return version.trim().replace(/^v/i, "");
}

function compareVersionStrings(a: string, b: string): number {
  const [aBase, aPre = ""] = normalizeVersion(a).split("-", 2);
  const [bBase, bPre = ""] = normalizeVersion(b).split("-", 2);
  const aParts = aBase.split(".").map((part) => Number.parseInt(part, 10));
  const bParts = bBase.split(".").map((part) => Number.parseInt(part, 10));
  const maxLen = Math.max(aParts.length, bParts.length);

  for (let index = 0; index < maxLen; index += 1) {
    const left = Number.isFinite(aParts[index]) ? aParts[index] : 0;
    const right = Number.isFinite(bParts[index]) ? bParts[index] : 0;
    if (left > right) {
      return 1;
    }
    if (left < right) {
      return -1;
    }
  }

  if (aPre && !bPre) {
    return -1;
  }
  if (!aPre && bPre) {
    return 1;
  }
  return aPre.localeCompare(bPre);
}

function emitMockEvent<E extends keyof EventPayloadMap>(
  eventName: E,
  payload: EventPayloadMap[E],
): void {
  const listeners = mockListeners.get(eventName);
  if (!listeners) {
    return;
  }
  for (const listener of listeners) {
    listener({ payload });
  }
}

function syncDisplaysFromLayout(): void {
  const activeOutputs = new Map(
    mockState.layout.outputs.map((output) => [output.display_key, output]),
  );

  for (const display of mockState.displays) {
    const output = activeOutputs.get(display.id_key);
    if (!output) {
      display.is_active = false;
      display.is_primary = false;
      continue;
    }

    display.is_active = output.enabled;
    display.is_primary = output.enabled && output.primary;
    display.resolution = { ...output.resolution };
    display.refresh_rate_mhz = output.refresh_rate_mhz;
  }
}

function ensureMockLayoutValid(layout: Layout): void {
  const enabled = layout.outputs.filter((output) => output.enabled);
  if (enabled.length === 0) {
    throw new Error("cannot disable the last active display");
  }

  let primaryFound = false;
  for (const output of layout.outputs) {
    if (!output.enabled) {
      output.primary = false;
      continue;
    }

    if (output.primary && !primaryFound) {
      primaryFound = true;
      continue;
    }
    output.primary = false;
  }

  if (!primaryFound) {
    const firstEnabled = layout.outputs.find((output) => output.enabled);
    if (firstEnabled) {
      firstEnabled.primary = true;
    }
  }
}

function findProfile(name: string): Profile | undefined {
  return mockState.profiles.find((profile) => profile.name === name);
}

function buildMockSnapshot(): AppSnapshot {
  const displays = [
    {
      id_key: "0000000000000001:1:0000000000001001",
      friendly_name: "Primary Display (Mock)",
      is_active: true,
      is_primary: true,
      resolution: { width: 2560, height: 1440 },
      refresh_rate_mhz: 144_000,
    },
    {
      id_key: "0000000000000001:2:0000000000001002",
      friendly_name: "Side Display (Mock)",
      is_active: true,
      is_primary: false,
      resolution: { width: 1920, height: 1080 },
      refresh_rate_mhz: 60_000,
    },
    {
      id_key: "0000000000000002:1:0000000000002001",
      friendly_name: "Portrait Display (Mock)",
      is_active: false,
      is_primary: false,
      resolution: { width: 1080, height: 1920 },
      refresh_rate_mhz: 60_000,
    },
  ];

  const layout: Layout = {
    outputs: [
      {
        display_key: displays[0].id_key,
        enabled: true,
        position: { x: 0, y: 0 },
        resolution: { ...displays[0].resolution },
        refresh_rate_mhz: displays[0].refresh_rate_mhz,
        primary: true,
      },
      {
        display_key: displays[1].id_key,
        enabled: true,
        position: { x: 2560, y: 140 },
        resolution: { ...displays[1].resolution },
        refresh_rate_mhz: displays[1].refresh_rate_mhz,
        primary: false,
      },
      {
        display_key: displays[2].id_key,
        enabled: false,
        position: { x: -1080, y: 0 },
        resolution: { ...displays[2].resolution },
        refresh_rate_mhz: displays[2].refresh_rate_mhz,
        primary: false,
      },
    ],
  };

  return {
    displays,
    layout,
    profiles: [
      { name: "Desk", layout: cloneLayout(layout) },
      {
        name: "Focus",
        layout: {
          outputs: layout.outputs.map((output) => ({
            ...output,
            enabled: output.display_key === displays[0].id_key,
            primary: output.display_key === displays[0].id_key,
          })),
        },
      },
    ],
    settings: {
      revert_timeout_secs: 10,
      start_with_windows: false,
      startup_profile_name: null,
      global_shortcuts_enabled: true,
      profile_shortcut_base: "Ctrl+Shift",
      display_toggle_shortcut_base: "Ctrl+Alt",
      profile_shortcuts: {},
      display_toggle_shortcuts: {},
    },
    pending_confirmation: null,
  };
}

export async function listenMonarchEvent<E extends keyof EventPayloadMap>(
  eventName: E,
  handler: (event: { payload: EventPayloadMap[E] }) => void,
): Promise<() => void> {
  if (!useWebMock) {
    return tauriListen(eventName, handler as never);
  }

  const listener = handler as unknown as MockListener;
  let listeners = mockListeners.get(eventName);
  if (!listeners) {
    listeners = new Set();
    mockListeners.set(eventName, listeners);
  }
  listeners.add(listener);
  return () => {
    listeners?.delete(listener);
  };
}

export async function getSnapshot(): Promise<AppSnapshot> {
  if (useWebMock) {
    return deepClone(mockState);
  }
  return invoke<AppSnapshot>("get_snapshot");
}

export async function getAppVersion(): Promise<string> {
  if (useWebMock) {
    return packageJson.version;
  }

  try {
    return await tauriGetVersion();
  } catch {
    return packageJson.version;
  }
}

export async function checkGithubReleaseUpdate(): Promise<ReleaseUpdateCheckResult> {
  const currentVersion = await getAppVersion();

  if (useWebMock) {
    return {
      currentVersion,
      latestVersion: packageJson.version,
      latestTag: `v${packageJson.version}`,
      updateAvailable: false,
      releaseUrl: GITHUB_RELEASES_URL,
    };
  }

  const response = await fetch(GITHUB_RELEASES_LATEST_API, {
    headers: {
      Accept: "application/vnd.github+json",
    },
  });

  if (!response.ok) {
    throw new Error(`GitHub releases check failed (${response.status})`);
  }

  const payload = (await response.json()) as {
    tag_name?: unknown;
    html_url?: unknown;
  };

  if (typeof payload.tag_name !== "string" || payload.tag_name.trim().length === 0) {
    throw new Error("GitHub releases response missing tag_name");
  }

  const latestTag = payload.tag_name.trim();
  const latestVersion = normalizeVersion(latestTag);
  const releaseUrl =
    typeof payload.html_url === "string" && payload.html_url.trim().length > 0
      ? payload.html_url
      : GITHUB_RELEASES_URL;

  return {
    currentVersion,
    latestVersion,
    latestTag,
    updateAvailable: compareVersionStrings(currentVersion, latestVersion) < 0,
    releaseUrl,
  };
}

export async function toggleDisplay(displayKey: string): Promise<void> {
  if (useWebMock) {
    const nextLayout = cloneLayout(mockState.layout);
    const output = nextLayout.outputs.find((item) => item.display_key === displayKey);
    if (!output) {
      throw new Error(`Display not found: ${displayKey}`);
    }
    mockRestorableLayout = cloneLayout(mockState.layout);
    output.enabled = !output.enabled;
    ensureMockLayoutValid(nextLayout);
    mockState.layout = nextLayout;
    syncDisplaysFromLayout();
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("toggle_display", { displayKey });
}

export async function applyLayout(layout: Layout): Promise<void> {
  if (useWebMock) {
    const nextLayout = cloneLayout(layout);
    ensureMockLayoutValid(nextLayout);
    mockRestorableLayout = cloneLayout(mockState.layout);
    mockState.layout = nextLayout;
    syncDisplaysFromLayout();
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("apply_layout", { layout });
}

export async function applyProfile(name: string): Promise<void> {
  if (useWebMock) {
    const profile = findProfile(name);
    if (!profile) {
      throw new Error(`Profile not found: ${name}`);
    }
    mockRestorableLayout = cloneLayout(mockState.layout);
    const nextLayout = cloneLayout(profile.layout);
    ensureMockLayoutValid(nextLayout);
    mockState.layout = nextLayout;
    syncDisplaysFromLayout();
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("apply_profile", { name });
}

export async function deleteProfile(name: string): Promise<void> {
  if (useWebMock) {
    mockState.profiles = mockState.profiles.filter((profile) => profile.name !== name);
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("delete_profile", { name });
}

export async function saveProfile(name: string): Promise<void> {
  if (useWebMock) {
    const trimmed = name.trim();
    if (!trimmed) {
      throw new Error("profile name cannot be empty");
    }
    const nextProfile: Profile = {
      name: trimmed,
      layout: cloneLayout(mockState.layout),
    };
    const existingIndex = mockState.profiles.findIndex((profile) => profile.name === trimmed);
    if (existingIndex >= 0) {
      mockState.profiles[existingIndex] = nextProfile;
    } else {
      mockState.profiles.push(nextProfile);
      mockState.profiles.sort((a, b) => a.name.localeCompare(b.name));
    }
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("save_profile", { name });
}

export async function restoreLastLayout(): Promise<void> {
  if (useWebMock) {
    const current = cloneLayout(mockState.layout);
    const nextLayout = cloneLayout(mockRestorableLayout);
    ensureMockLayoutValid(nextLayout);
    mockState.layout = nextLayout;
    mockRestorableLayout = current;
    syncDisplaysFromLayout();
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("restore_last_layout");
}

export async function confirmCurrentLayout(): Promise<void> {
  if (useWebMock) {
    mockState.pending_confirmation = null;
    emitMockEvent("monarch://confirmation", { kind: "confirmed" });
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("confirm_current_layout");
}

export async function rollbackPending(): Promise<void> {
  if (useWebMock) {
    mockState.pending_confirmation = null;
    emitMockEvent("monarch://confirmation", { kind: "reverted", reason: "manual" });
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("rollback_pending");
}

export async function updateSettings(settings: AppSettings): Promise<void> {
  if (useWebMock) {
    mockState.settings = {
      ...settings,
      revert_timeout_secs: Math.max(1, Math.floor(settings.revert_timeout_secs)),
      start_with_windows: Boolean(settings.start_with_windows),
      startup_profile_name:
        typeof settings.startup_profile_name === "string" &&
        settings.startup_profile_name.trim().length > 0
          ? settings.startup_profile_name.trim()
          : null,
      global_shortcuts_enabled: settings.global_shortcuts_enabled !== false,
      profile_shortcut_base:
        typeof settings.profile_shortcut_base === "string" &&
        settings.profile_shortcut_base.trim().length > 0
          ? settings.profile_shortcut_base.trim()
          : "Ctrl+Shift",
      display_toggle_shortcut_base:
        typeof settings.display_toggle_shortcut_base === "string" &&
        settings.display_toggle_shortcut_base.trim().length > 0
          ? settings.display_toggle_shortcut_base.trim()
          : "Ctrl+Alt",
      profile_shortcuts: Object.fromEntries(
        Object.entries(settings.profile_shortcuts ?? {}).flatMap(([name, shortcut]) => {
          const nextName = String(name ?? "").trim();
          const nextShortcut = String(shortcut ?? "").trim();
          return nextName && nextShortcut ? [[nextName, nextShortcut]] : [];
        }),
      ),
      display_toggle_shortcuts: Object.fromEntries(
        Object.entries(settings.display_toggle_shortcuts ?? {}).flatMap(([displayKey, shortcut]) => {
          const nextDisplayKey = String(displayKey ?? "").trim();
          const nextShortcut = String(shortcut ?? "").trim();
          return nextDisplayKey && nextShortcut ? [[nextDisplayKey, nextShortcut]] : [];
        }),
      ),
    };
    emitMockEvent("monarch://state-changed", undefined);
    return;
  }
  return invoke("update_settings", { settings });
}
