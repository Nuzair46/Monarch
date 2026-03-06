import { useMemo } from "react";

import { cn } from "@/lib/utils";
import type { AppSnapshot } from "@/types";

type LayoutOutput = AppSnapshot["layout"]["outputs"][number];

function previewOutputs(snapshot: AppSnapshot | null) {
  if (!snapshot) {
    return [];
  }

  const knownDisplayKeys = new Set(snapshot.displays.map((display) => display.id_key));
  return snapshot.layout.outputs.filter(
    (output) => output.enabled || knownDisplayKeys.has(output.display_key),
  );
}

function getOutputResolution(
  output: LayoutOutput,
  displayByKey: Map<string, AppSnapshot["displays"][number]>,
) {
  const display = displayByKey.get(output.display_key);
  if (!display?.is_active) {
    return output.resolution;
  }

  return display.resolution;
}

function layoutBounds(snapshot: AppSnapshot | null, outputs: LayoutOutput[]) {
  if (!snapshot || outputs.length === 0) {
    return null;
  }

  const displayByKey = new Map(snapshot.displays.map((display) => [display.id_key, display]));

  const left = Math.min(...outputs.map((o) => o.position.x));
  const top = Math.min(...outputs.map((o) => o.position.y));
  const right = Math.max(
    ...outputs.map((output) => output.position.x + getOutputResolution(output, displayByKey).width),
  );
  const bottom = Math.max(
    ...outputs.map((output) => output.position.y + getOutputResolution(output, displayByKey).height),
  );

  return { left, top, right, bottom, width: right - left, height: bottom - top };
}

export function LayoutPreview({ snapshot }: { snapshot: AppSnapshot | null }) {
  const outputs = useMemo(() => previewOutputs(snapshot), [snapshot]);
  const bounds = useMemo(() => layoutBounds(snapshot, outputs), [snapshot, outputs]);
  const displayByKey = useMemo(
    () => new Map((snapshot?.displays ?? []).map((display) => [display.id_key, display])),
    [snapshot],
  );
  const monitorNumberByDisplayKey = useMemo(
    () =>
      new Map(
        (snapshot?.displays ?? []).map((display, index) => [display.id_key, index + 1]),
      ),
    [snapshot],
  );

  if (!snapshot || !bounds) {
    return (
      <div className="grid min-h-[260px] place-items-center rounded-lg border border-dashed p-6 text-sm text-muted-foreground">
        No layout available
      </div>
    );
  }

  const scale = Math.min(720 / bounds.width, 300 / bounds.height);

  return (
    <div
      className="overflow-auto rounded-lg border p-3"
      aria-label="Display layout preview"
    >
      <div
        className="relative min-h-[190px] rounded-md border bg-muted/30"
        style={{
          width: `${Math.max(340, bounds.width * scale + 24)}px`,
          height: `${Math.max(180, bounds.height * scale + 24)}px`,
        }}
      >
        {outputs.map((output) => {
          const display = displayByKey.get(output.display_key);
          const monitorNumber = monitorNumberByDisplayKey.get(output.display_key);
          const active = output.enabled;
          const previewResolution =
            display?.is_active && active ? display.resolution : output.resolution;

          return (
            <div
              key={output.display_key}
              className={cn(
                "absolute flex min-w-0 flex-col justify-between rounded-md border p-2 text-[10px]",
                active
                  ? "border-primary/30 bg-primary/10"
                  : "bg-muted text-muted-foreground",
              )}
              style={{
                left: `${(output.position.x - bounds.left) * scale + 12}px`,
                top: `${(output.position.y - bounds.top) * scale + 12}px`,
                width: `${Math.max(42, previewResolution.width * scale)}px`,
                height: `${Math.max(28, previewResolution.height * scale)}px`,
              }}
              title={
                monitorNumber
                  ? `Monitor ${monitorNumber}: ${display?.friendly_name ?? output.display_key}`
                  : (display?.friendly_name ?? output.display_key)
              }
            >
              <div className="flex min-w-0 items-start justify-between gap-1">
                <div className="flex min-w-0 items-start gap-1">
                  {monitorNumber ? (
                    <span className="inline-flex h-5 min-w-5 shrink-0 items-center justify-center rounded border bg-background/70 px-1.5 text-[11px] font-bold leading-none">
                      {monitorNumber}
                    </span>
                  ) : null}
                  <span className="truncate text-[10px] font-medium leading-tight">
                    {display?.friendly_name ?? "Display"}
                  </span>
                </div>
                {output.primary ? (
                  <span className="shrink-0 rounded-full border px-1.5 py-0.5 text-[8px] font-semibold uppercase tracking-wide">
                    Primary
                  </span>
                ) : null}
              </div>
              <span className="text-[9px] leading-none text-muted-foreground">
                {active ? "Active" : "Detached"}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
