import type {
  AppSettings,
  AppSnapshot,
  Layout,
  Profile,
  DisplayCapabilities,
} from "./types";
import { normalizeGroups } from "@/app/display-editor";
import type { EventPayloadMap } from "./tauri";
import {
  DEFAULT_MONITOR_SHORTCUT_BASE,
  DEFAULT_PROFILE_SHORTCUT_BASE,
} from "@/app/ui";
type MockListener = (event: { payload: unknown }) => void;
const mockListeners = new Map<string, Set<MockListener>>();
let mockState = buildMockSnapshot();
let mockRestorableLayout = cloneLayout(mockState.layout);
let confirmationDeadline: number | null = null;
let confirmationTimer: ReturnType<typeof setTimeout> | undefined;
function ensureNoPending(): void {
  if (mockState.pending_confirmation)
    throw new Error("a layout is awaiting confirmation");
}

function clearPending(): void {
  clearTimeout(confirmationTimer);
  confirmationTimer = undefined;
  confirmationDeadline = null;
  mockState.pending_confirmation = null;
}

function revert(reason: "manual" | "timeout"): void {
  if (!mockState.pending_confirmation)
    throw new Error("no layout is awaiting confirmation");
  mockState.layout = cloneLayout(mockRestorableLayout);
  clearPending();
  syncDisplaysFromLayout();
  emitMockEvent("monarch://confirmation", { kind: "reverted", reason });
  emitMockStateChanged();
}

function deepClone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function cloneLayout(layout: Layout): Layout {
  return deepClone(layout);
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

function emitMockStateChanged(): void {
  mockState.generation += 1;
  emitMockEvent("monarch://state-changed", undefined);
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
  normalizeGroups(layout);
  const primary = layout.outputs.find(
    (output) => output.enabled && output.primary,
  )!;
  const offset = { ...primary.position };
  for (const output of layout.outputs) {
    output.position.x -= offset.x;
    output.position.y -= offset.y;
  }
}

function findProfile(name: string): Profile | undefined {
  return mockState.profiles.find((profile) => profile.name === name);
}

function trimmedStringOrNull(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

function sanitizeShortcutMap(
  shortcuts: Record<string, string> | null | undefined,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(shortcuts ?? {}).flatMap(([rawKey, rawShortcut]) => {
      const key = trimmedStringOrNull(rawKey);
      const shortcut = trimmedStringOrNull(rawShortcut);
      return key && shortcut ? [[key, shortcut]] : [];
    }),
  );
}

function replaceMockLayout(nextLayout: Layout): void {
  ensureNoPending();
  const validatedLayout = cloneLayout(nextLayout);
  ensureMockLayoutValid(validatedLayout);
  for (const o of validatedLayout.outputs.filter((o) => o.enabled)) {
    const cap = mockState.capabilities.find(
      (c) => c.display_key === o.display_key,
    );
    if (!cap)
      throw new Error("Display unavailable; reconnect it before applying.");
    const portrait =
      o.rotation === "portrait" || o.rotation === "portrait_flipped";
    if (
      !cap.modes.some(
        (m) =>
          m.resolution.width ===
            (portrait ? o.resolution.height : o.resolution.width) &&
          m.resolution.height ===
            (portrait ? o.resolution.width : o.resolution.height) &&
          m.refresh_rate_mhz === o.refresh_rate_mhz,
      )
    )
      throw new Error(
        "Unsupported resolution/refresh combination; select a reported mode.",
      );
    if (o.hdr_enabled != null && !cap.hdr_supported)
      throw new Error("HDR unavailable; choose Preserve HDR.");
    if (
      o.scale_percent != null &&
      !cap.scale_percentages.includes(o.scale_percent)
    )
      throw new Error("Unsupported scaling; choose Preserve scaling.");
  }
  mockRestorableLayout = cloneLayout(mockState.layout);
  mockState.layout = validatedLayout;
  const timeout_ms = mockState.settings.revert_timeout_secs * 1000;
  confirmationDeadline = Date.now() + timeout_ms;
  mockState.pending_confirmation = { remaining_ms: timeout_ms };
  confirmationTimer = setTimeout(() => revert("timeout"), timeout_ms);
  emitMockEvent("monarch://confirmation", { kind: "applied", timeout_ms });
  syncDisplaysFromLayout();
  emitMockStateChanged();
}

function buildMockSnapshot(): AppSnapshot {
  const displays = [
    {
      id_key: "0000000000000001:1:0000000000001001",
      friendly_name: "Primary Display (Mock)",
      is_active: true,
      is_primary: true,
      resolution: { width: 2560, height: 1440 },
      refresh_rate_mhz: 144000,
    },
    {
      id_key: "0000000000000001:2:0000000000001002",
      friendly_name: "Side Display (Mock)",
      is_active: true,
      is_primary: false,
      resolution: { width: 1920, height: 1080 },
      refresh_rate_mhz: 60000,
    },
    {
      id_key: "0000000000000002:1:0000000000002001",
      friendly_name: "Portrait Display (Mock)",
      is_active: false,
      is_primary: false,
      resolution: { width: 1080, height: 1920 },
      refresh_rate_mhz: 60000,
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
        rotation: "portrait",
        enabled: false,
        position: { x: -1080, y: 0 },
        resolution: { ...displays[2].resolution },
        refresh_rate_mhz: displays[2].refresh_rate_mhz,
        primary: false,
      },
    ],
  };
  normalizeGroups(layout);
  return {
    generation: 0,
    displays,
    layout,
    capabilities: displays.map((display, index) => ({
      display_key: display.id_key,
      modes: [
        { resolution: { width: 1920, height: 1080 }, refresh_rate_mhz: 60000 },
        { resolution: { width: 1920, height: 1080 }, refresh_rate_mhz: 59940 },
        ...(index < 2
          ? [
              {
                resolution: { width: 2560, height: 1440 },
                refresh_rate_mhz: 60000,
              },
            ]
          : []),
        ...(index === 0
          ? [
              {
                resolution: { width: 2560, height: 1440 },
                refresh_rate_mhz: 144000,
              },
            ]
          : []),
      ],
      modes_unavailable_reason: null,
      hdr_supported: index === 0,
      hdr_enabled: index === 0 ? false : null,
      hdr_unavailable_reason:
        index === 0 ? null : "HDR is unsupported on this monitor.",
      scale_percent: index < 2 ? 100 : null,
      scale_percentages: index < 2 ? [100, 125, 150, 175, 200] : [],
      scaling_unavailable_reason:
        index < 2
          ? null
          : "Scaling is unavailable while this monitor is detached.",
    })),
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
      profile_shortcut_base: DEFAULT_PROFILE_SHORTCUT_BASE,
      display_toggle_shortcut_base: DEFAULT_MONITOR_SHORTCUT_BASE,
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
  const listener = handler as unknown as MockListener;
  const listeners = mockListeners.get(eventName) ?? new Set<MockListener>();
  mockListeners.set(eventName, listeners);
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export async function getSnapshot(): Promise<AppSnapshot> {
  const snapshot = deepClone(mockState);
  if (snapshot.pending_confirmation && confirmationDeadline !== null) {
    snapshot.pending_confirmation.remaining_ms = Math.max(
      0,
      confirmationDeadline - Date.now(),
    );
  }
  return snapshot;
}

export async function toggleDisplay(displayKey: string): Promise<void> {
  const nextLayout = cloneLayout(mockState.layout);
  const output = nextLayout.outputs.find(
    (item) => item.display_key === displayKey,
  );
  if (!output) {
    throw new Error(`Display not found: ${displayKey}`);
  }
  output.enabled = !output.enabled;
  replaceMockLayout(nextLayout);
  return;
}

export async function applyLayout(layout: Layout): Promise<void> {
  replaceMockLayout(layout);
  return;
}

export async function applyProfile(name: string): Promise<void> {
  const profile = findProfile(name);
  if (!profile) {
    throw new Error(`Profile not found: ${name}`);
  }
  replaceMockLayout(profile.layout);
  return;
}

export async function deleteProfile(name: string): Promise<void> {
  mockState.profiles = mockState.profiles.filter(
    (profile) => profile.name !== name,
  );
  emitMockStateChanged();
  return;
}

export async function saveProfile(name: string): Promise<void> {
  ensureNoPending();
  const trimmed = name.trim();
  if (!trimmed) {
    throw new Error("profile name cannot be empty");
  }
  const nextProfile: Profile = {
    name: trimmed,
    layout: cloneLayout(mockState.layout),
  };
  const existingIndex = mockState.profiles.findIndex(
    (profile) => profile.name === trimmed,
  );
  if (existingIndex >= 0) {
    mockState.profiles[existingIndex] = nextProfile;
  } else {
    mockState.profiles.push(nextProfile);
    mockState.profiles.sort((a, b) => a.name.localeCompare(b.name));
  }
  emitMockStateChanged();
  return;
}

export async function restoreLastLayout(): Promise<void> {
  if (mockState.pending_confirmation) {
    revert("manual");
    return;
  }
  const current = cloneLayout(mockState.layout);
  const nextLayout = cloneLayout(mockRestorableLayout);
  ensureMockLayoutValid(nextLayout);
  mockState.layout = nextLayout;
  mockRestorableLayout = current;
  syncDisplaysFromLayout();
  emitMockStateChanged();
  return;
}

export async function confirmCurrentLayout(): Promise<void> {
  if (!mockState.pending_confirmation)
    throw new Error("no layout is awaiting confirmation");
  clearPending();
  emitMockEvent("monarch://confirmation", { kind: "confirmed" });
  emitMockStateChanged();
  return;
}

export async function rollbackPending(): Promise<void> {
  revert("manual");
}

export async function updateSettings(settings: AppSettings): Promise<void> {
  if (
    !Number.isInteger(settings.revert_timeout_secs) ||
    settings.revert_timeout_secs < 1 ||
    settings.revert_timeout_secs > 60
  ) {
    throw new Error("revert timeout must be between 1 and 60 seconds");
  }
  mockState.settings = {
    ...settings,
    revert_timeout_secs: Math.max(1, Math.floor(settings.revert_timeout_secs)),
    start_with_windows: Boolean(settings.start_with_windows),
    startup_profile_name: trimmedStringOrNull(settings.startup_profile_name),
    global_shortcuts_enabled: settings.global_shortcuts_enabled !== false,
    profile_shortcut_base:
      trimmedStringOrNull(settings.profile_shortcut_base) ??
      (Object.keys(settings.profile_shortcuts).length
        ? null
        : DEFAULT_PROFILE_SHORTCUT_BASE),
    display_toggle_shortcut_base:
      trimmedStringOrNull(settings.display_toggle_shortcut_base) ??
      (Object.keys(settings.display_toggle_shortcuts).length
        ? null
        : DEFAULT_MONITOR_SHORTCUT_BASE),
    profile_shortcuts: sanitizeShortcutMap(settings.profile_shortcuts),
    display_toggle_shortcuts: sanitizeShortcutMap(
      settings.display_toggle_shortcuts,
    ),
  };
  emitMockStateChanged();
  return;
}

export async function getDisplayCapabilities(): Promise<DisplayCapabilities[]> {
  return deepClone(mockState.capabilities);
}
