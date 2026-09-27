import { useState } from "react";
import { Dialog } from "radix-ui";
import { Button } from "@/components/ui/button";
import {
  capabilityMatches,
  changeAttachment,
  editOutput,
  nativeResolution,
  resolutionChoices,
  refreshChoices,
} from "@/app/display-editor";
import type { AppSnapshot, Layout, OutputConfig } from "@/types";

const selectClass =
  "h-9 w-full rounded-md border bg-background px-2 text-sm disabled:opacity-50";
const rotations = [
  ["landscape", "Landscape"],
  ["portrait", "Portrait (90°)"],
  ["landscape_flipped", "Landscape (180°)"],
  ["portrait_flipped", "Portrait (270°)"],
] as const;

export function DisplayProperties({
  snapshot,
  initial,
  displayKey,
  busy,
  onSave,
  onClose,
}: {
  snapshot: AppSnapshot;
  initial: Layout;
  displayKey: string;
  busy: boolean;
  onSave: (layout: Layout) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState(() => structuredClone(initial));
  const [error, setError] = useState<string | null>(null);
  const output = draft.outputs.find((o) => o.display_key === displayKey)!;
  const display = snapshot.displays.find((d) => d.id_key === displayKey);
  const cap = snapshot.capabilities.find((c) => capabilityMatches(output, c));
  const dimensions = nativeResolution(output);
  const resolutionKey = `${dimensions.width}x${dimensions.height}`;
  const resolutions = resolutionChoices(cap);
  const rates = refreshChoices(cap, output);
  const validMode =
    !output.enabled ||
    !cap?.modes.length ||
    rates.some((r) => Math.abs(r - output.refresh_rate_mhz) <= 2);
  const change = (patch: Partial<OutputConfig>) =>
    setDraft(editOutput(draft, displayKey, patch));
  const attachment = !output.enabled
    ? "detached"
    : output.clone_group
      ? (draft.outputs.find(
          (o) =>
            o !== output && o.enabled && o.clone_group === output.clone_group,
        )?.display_key ?? "extend")
      : "extend";
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/70" />
        <Dialog.Content
          className="fixed left-1/2 top-1/2 z-40 max-h-[90vh] w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-lg border bg-background p-5"
          onEscapeKeyDown={(e) => {
            if (busy) e.preventDefault();
          }}
        >
          <Dialog.Title className="text-lg font-semibold">
            {display?.friendly_name ?? "Display"} properties
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-muted-foreground">
            Save the layout to apply these changes.
          </Dialog.Description>
          <fieldset disabled={busy} className="my-5 grid gap-4 sm:grid-cols-2">
            <label className="grid gap-1 text-sm sm:col-span-2">
              Display mode
              <select
                className={selectClass}
                aria-label="Display mode"
                value={attachment}
                onChange={(e) => {
                  try {
                    setDraft(
                      changeAttachment(draft, displayKey, e.target.value),
                    );
                    setError(null);
                  } catch (e) {
                    setError(e instanceof Error ? e.message : String(e));
                  }
                }}
              >
                <option value="extend">Extend</option>
                <option value="detached">Detached</option>
                {draft.outputs
                  .filter((o) => o !== output && o.enabled)
                  .map((o) => (
                    <option key={o.display_key} value={o.display_key}>
                      Duplicate of{" "}
                      {snapshot.displays.find((d) => d.id_key === o.display_key)
                        ?.friendly_name ?? "Display"}
                    </option>
                  ))}
              </select>
            </label>
            <label className="grid gap-1 text-sm">
              Resolution
              <select
                className={selectClass}
                aria-label="Resolution"
                value={resolutionKey}
                disabled={!resolutions.length}
                onChange={(e) => {
                  const resolution = resolutions.find(
                    (r) => `${r.width}x${r.height}` === e.target.value,
                  )!;
                  change({
                    resolution:
                      output.rotation === "portrait" ||
                      output.rotation === "portrait_flipped"
                        ? { width: resolution.height, height: resolution.width }
                        : { ...resolution },
                  });
                }}
              >
                {!resolutions.some(
                  (r) => `${r.width}x${r.height}` === resolutionKey,
                ) && (
                  <option value={resolutionKey}>
                    {dimensions.width} × {dimensions.height} (current)
                  </option>
                )}
                {resolutions.map((r) => (
                  <option
                    key={`${r.width}x${r.height}`}
                    value={`${r.width}x${r.height}`}
                  >
                    {r.width} × {r.height}
                  </option>
                ))}
              </select>
            </label>
            <label className="grid gap-1 text-sm">
              Refresh rate
              <select
                className={selectClass}
                aria-label="Refresh rate"
                value={output.refresh_rate_mhz}
                disabled={!rates.length}
                onChange={(e) =>
                  change({ refresh_rate_mhz: Number(e.target.value) })
                }
              >
                {!rates.includes(output.refresh_rate_mhz) && (
                  <option value={output.refresh_rate_mhz}>
                    {output.refresh_rate_mhz / 1000} Hz
                    {validMode ? " (current)" : " (unavailable)"}
                  </option>
                )}
                {rates.map((r) => (
                  <option key={r} value={r}>
                    {r / 1000} Hz
                  </option>
                ))}
              </select>
            </label>
            {!validMode && (
              <p
                role="alert"
                className="text-sm text-destructive sm:col-span-2"
              >
                Select a refresh rate supported at this resolution.
              </p>
            )}
            <label className="grid gap-1 text-sm">
              Orientation
              <select
                className={selectClass}
                aria-label="Orientation"
                value={output.rotation ?? "landscape"}
                onChange={(e) => {
                  const rotation = e.target.value as OutputConfig["rotation"];
                  change({
                    rotation,
                    resolution:
                      rotation === "portrait" || rotation === "portrait_flipped"
                        ? { width: dimensions.height, height: dimensions.width }
                        : { ...dimensions },
                  });
                }}
              >
                {rotations.map(([value, label]) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </select>
            </label>
            <label className="grid gap-1 text-sm">
              Scaling
              <select
                className={selectClass}
                aria-label="Scaling"
                value={output.scale_percent ?? "preserve"}
                disabled={!cap?.scale_percentages.length}
                onChange={(e) =>
                  change({
                    scale_percent:
                      e.target.value === "preserve"
                        ? null
                        : Number(e.target.value),
                  })
                }
              >
                <option value="preserve">Preserve scaling</option>
                {output.scale_percent != null &&
                  !cap?.scale_percentages.includes(output.scale_percent) && (
                    <option value={output.scale_percent}>
                      {output.scale_percent}% (current)
                    </option>
                  )}
                {cap?.scale_percentages.map((s) => (
                  <option key={s} value={s}>
                    {s}%
                  </option>
                ))}
              </select>
            </label>
            <label className="grid gap-1 text-sm">
              HDR
              <select
                className={selectClass}
                aria-label="HDR"
                value={
                  output.hdr_enabled == null
                    ? "preserve"
                    : String(output.hdr_enabled)
                }
                disabled={!cap?.hdr_supported}
                onChange={(e) =>
                  change({
                    hdr_enabled:
                      e.target.value === "preserve"
                        ? null
                        : e.target.value === "true",
                  })
                }
              >
                <option value="preserve">Preserve HDR</option>
                <option value="true">On</option>
                <option value="false">Off</option>
              </select>
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={output.primary}
                disabled={!output.enabled || output.primary}
                onChange={() => change({ primary: true })}
              />
              Primary display
            </label>
          </fieldset>
          {!cap && (
            <p className="text-sm text-muted-foreground">
              Live capabilities are unavailable. Reconnect the monitor before
              applying new properties.
            </p>
          )}
          {[
            cap?.modes_unavailable_reason,
            cap?.hdr_unavailable_reason,
            cap?.scaling_unavailable_reason,
          ]
            .filter(Boolean)
            .map((reason) => (
              <p key={reason} className="mt-2 text-xs text-muted-foreground">
                {reason}
              </p>
            ))}
          {error && (
            <p role="alert" className="mt-2 text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="mt-5 flex justify-end gap-2 border-t pt-4">
            <Button variant="outline" disabled={busy} onClick={onClose}>
              Cancel
            </Button>
            <Button disabled={busy || !validMode} onClick={() => onSave(draft)}>
              Done
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
