export type DisplayInfo = {
  id_key: string;
  friendly_name: string;
  is_active: boolean;
  is_primary: boolean;
  resolution: { width: number; height: number };
  refresh_rate_mhz: number;
};

export type OutputConfig = {
  display_key: string;
  enabled: boolean;
  position: { x: number; y: number };
  resolution: { width: number; height: number };
  refresh_rate_mhz: number;
  primary: boolean;
};

export type Layout = {
  outputs: OutputConfig[];
};

export type Profile = {
  name: string;
  layout: Layout;
};

export type AppSettings = {
  revert_timeout_secs: number;
  start_with_windows: boolean;
  startup_profile_name: string | null;
  global_shortcuts_enabled: boolean;
  profile_shortcut_base: string | null;
  display_toggle_shortcut_base: string | null;
  profile_shortcuts: Record<string, string>;
  display_toggle_shortcuts: Record<string, string>;
};

export type PendingConfirmation = {
  remaining_ms: number;
};

export type AppSnapshot = {
  displays: DisplayInfo[];
  layout: Layout;
  profiles: Profile[];
  settings: AppSettings;
  pending_confirmation: PendingConfirmation | null;
};

export type ConfirmationEvent =
  | { kind: "applied"; timeout_ms: number }
  | { kind: "confirmed" }
  | { kind: "reverted"; reason: "manual" | "timeout" | "error" };
