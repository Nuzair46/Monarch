export type View = "main" | "profiles" | "settings";

export type PendingDisplayToggle = {
  idKey: string;
  friendlyName: string;
  currentlyActive: boolean;
};

export const DEFAULT_REVERT_TIMEOUT_SECS = 10;
export const DEFAULT_GLOBAL_SHORTCUTS_ENABLED = true;
export const DEFAULT_MONITOR_SHORTCUT_BASE = "Ctrl+Alt";
export const DEFAULT_PROFILE_SHORTCUT_BASE = "Ctrl+Shift";
export const REPO_URL = "https://github.com/Nuzair46/Monarch";

export const VIEW_OPTIONS: Array<{ id: View; label: string }> = [
  { id: "main", label: "Main" },
  { id: "profiles", label: "Profiles" },
  { id: "settings", label: "Settings" },
];

export function isView(value: string): value is View {
  return value === "main" || value === "profiles" || value === "settings";
}
