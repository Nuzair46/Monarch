const VIEW_IDS = ["main", "profiles", "settings"] as const;
export type View = (typeof VIEW_IDS)[number];

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

export const VIEW_OPTIONS = [
  { id: "main", label: "Main" },
  { id: "profiles", label: "Profiles" },
  { id: "settings", label: "Settings" },
] satisfies Array<{ id: View; label: string }>;

const VIEW_ID_SET = new Set<View>(VIEW_IDS);

export function isView(value: string): value is View {
  return VIEW_ID_SET.has(value as View);
}
