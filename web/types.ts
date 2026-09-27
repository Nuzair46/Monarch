export type Resolution = {
  width: number;
  height: number;
};

export type Position = {
  x: number;
  y: number;
};

export type DisplayInfo = {
  id_key: string;
  friendly_name: string;
  is_active: boolean;
  is_primary: boolean;
  resolution: Resolution;
  refresh_rate_mhz: number;
};

export type MonitorIdentity = {
  device_path: string | null;
  edid_serial: string | null;
};

export type OutputConfig = {
  display_key: string;
  identity?: MonitorIdentity;
  enabled: boolean;
  position: Position;
  resolution: Resolution;
  refresh_rate_mhz: number;
  primary: boolean;
  hdr_enabled?: boolean | null;
  scale_percent?: number | null;
  clone_group?: string | null;
  rotation?:
    | "landscape"
    | "portrait"
    | "landscape_flipped"
    | "portrait_flipped"
    | null;
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
  cursor_correction_enabled: boolean;
  cursor_calibrations: CursorCalibration[];
};

export type PendingConfirmation = {
  remaining_ms: number;
};

export type AppSnapshot = {
  generation: number;
  displays: DisplayInfo[];
  layout: Layout;
  profiles: Profile[];
  capabilities: DisplayCapabilities[];
  settings: AppSettings;
  pending_confirmation: PendingConfirmation | null;
};

export type ConfirmationEvent =
  | { kind: "applied"; timeout_ms: number }
  | { kind: "confirmed" }
  | { kind: "reverted"; reason: "manual" | "timeout" }
  | { kind: "rollback_failed"; message: string };

export type DisplayCapabilities = {
  display_key: string;
  identity?: MonitorIdentity;
  modes: { resolution: Resolution; refresh_rate_mhz: number }[];
  modes_unavailable_reason: string | null;
  hdr_supported: boolean;
  hdr_enabled: boolean | null;
  hdr_unavailable_reason: string | null;
  scale_percent: number | null;
  scale_percentages: number[];
  scaling_unavailable_reason: string | null;
  physical_size_mm: Resolution | null;
};

export type CursorCalibration = {
  display_key: string;
  identity?: { device_path: string | null; edid_serial: string | null };
  width_mm: number;
  height_mm: number;
  position_mm: Position;
  clone_representative: boolean;
};
