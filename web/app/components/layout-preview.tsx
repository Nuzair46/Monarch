import { MonitorArrangement } from "./monitor-arrangement";
import { sameSource } from "@/app/display-editor";
import type { AppSnapshot, Position } from "@/types";

export function LayoutPreview({
  snapshot,
  disabled,
  onMove,
}: {
  snapshot: AppSnapshot | null;
  disabled?: boolean;
  onMove?: (key: string, position: Position) => void;
}) {
  const active = snapshot?.layout.outputs.filter((o) => o.enabled) ?? [];
  const monitors = active
    .filter((o, i) => !active.slice(0, i).some((other) => sameSource(o, other)))
    .map((o) => {
      const members = active.filter(
        (other) => other === o || sameSource(o, other),
      );
      return {
        key: o.display_key,
        ...o.position,
        ...o.resolution,
        primary: o.primary,
        label: members
          .map(
            (m) =>
              (snapshot?.displays.findIndex(
                (d) => d.id_key === m.display_key,
              ) ?? -1) + 1,
          )
          .join(" + "),
        name: members
          .map(
            (m) =>
              snapshot?.displays.find((d) => d.id_key === m.display_key)
                ?.friendly_name ?? "Display",
          )
          .join(" + "),
      };
    });
  return (
    <MonitorArrangement
      monitors={monitors}
      disabled={disabled}
      onMove={onMove}
      joinOnDrop
      label="Display layout preview"
    />
  );
}
