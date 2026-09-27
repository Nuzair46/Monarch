import { useState } from "react";
import { Dialog } from "radix-ui";
import { Button } from "@/components/ui/button";
import {
  capabilityMatches,
  changeAttachment,
  editOutput,
  nativeResolution,
  sharedResolutionChoices,
  sharedScaleChoices,
  canPreserveScaling,
  sourceMembers,
  refreshChoices,
  layoutError,
  rebaseLayout,
  fitDesktop,
} from "@/app/display-editor";
import type {
  AppSnapshot,
  DisplayCapabilities,
  Layout,
  OutputConfig,
} from "@/types";

const selectClass =
  "h-9 w-full rounded-md border bg-background px-2 text-sm disabled:opacity-50";
const rotations = [
  ["landscape", "Landscape"],
  ["portrait", "Portrait (90°)"],
  ["landscape_flipped", "Landscape (180°)"],
  ["portrait_flipped", "Portrait (270°)"],
] as const;

function RefreshRateField({
  output,
  cap,
  label = "Refresh rate",
  onChange,
}: {
  output: OutputConfig;
  cap: DisplayCapabilities | undefined;
  label?: string;
  onChange: (rate: number) => void;
}) {
  const rates = refreshChoices(cap, output);
  return (
    <label className="grid gap-1 text-sm">
      {label}
      <select
        className={selectClass}
        aria-label={label}
        value={output.refresh_rate_mhz}
        disabled={!rates.length}
        onChange={(e) => onChange(Number(e.target.value))}
      >
        {!rates.includes(output.refresh_rate_mhz) && (
          <option value={output.refresh_rate_mhz}>
            {output.refresh_rate_mhz / 1000} Hz
            {rates.some((r) => Math.abs(r - output.refresh_rate_mhz) <= 2)
              ? " (current)"
              : " (unavailable)"}
          </option>
        )}
        {rates.map((rate) => (
          <option key={rate} value={rate}>
            {rate / 1000} Hz
          </option>
        ))}
      </select>
    </label>
  );
}

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
  onSave: (layout: Layout) => Promise<boolean>;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState(() => structuredClone(initial));
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const locked = busy || saving;
  const output = draft.outputs.find((o) => o.display_key === displayKey)!;
  const display = snapshot.displays.find((d) => d.id_key === displayKey);
  const cap = snapshot.capabilities.find((c) => capabilityMatches(output, c));
  const dimensions = nativeResolution(output);
  const resolutionKey = `${dimensions.width}x${dimensions.height}`;
  const members = sourceMembers(draft, output);
  const duplicated = members.length > 1;
  const resolutions = sharedResolutionChoices(
    draft,
    output,
    snapshot.capabilities,
  );
  const scales = sharedScaleChoices(draft, output, snapshot.capabilities);
  const preserveScaling = canPreserveScaling(
    draft,
    output,
    snapshot.capabilities,
  );
  const validMode =
    !output.enabled ||
    members.every((member) =>
      refreshChoices(
        snapshot.capabilities.find((c) => capabilityMatches(member, c)),
        member,
      ).some((r) => Math.abs(r - member.refresh_rate_mhz) <= 2),
    );
  const change = (patch: Partial<OutputConfig>, key = displayKey) => {
    const next = editOutput(draft, key, patch);
    setDraft(patch.resolution ? fitDesktop(next) : next);
    setError(null);
  };
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
        if (!open && !locked) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/70" />
        <Dialog.Content
          className="fixed left-1/2 top-1/2 z-40 max-h-[90vh] w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-lg border bg-background p-5"
          onEscapeKeyDown={(e) => {
            if (locked) e.preventDefault();
          }}
        >
          <Dialog.Title className="text-lg font-semibold">
            {display?.friendly_name ?? "Display"} settings
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-muted-foreground">
            Save to apply these settings. You can confirm or revert afterward.
          </Dialog.Description>
          {draft.outputs.some((o) => {
            const before = initial.outputs.find(
              (p) => p.display_key === o.display_key,
            );
            return (
              before &&
              (before.position.x !== o.position.x ||
                before.position.y !== o.position.y)
            );
          }) && (
            <p className="mt-2 text-sm text-muted-foreground">
              Monitor positions will adjust to keep display edges joined and the
              primary display at the desktop origin.
            </p>
          )}
          <fieldset
            disabled={locked}
            className="my-5 grid gap-4 sm:grid-cols-2"
          >
            <label className="grid gap-1 text-sm sm:col-span-2">
              Display mode
              <select
                className={selectClass}
                aria-label="Display mode"
                value={attachment}
                onChange={(e) => {
                  try {
                    setDraft(
                      fitDesktop(
                        changeAttachment(
                          draft,
                          displayKey,
                          e.target.value,
                          snapshot.capabilities,
                        ),
                      ),
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
            {duplicated && (
              <p className="text-sm text-muted-foreground sm:col-span-2">
                These monitors mirror the same desktop:{" "}
                {members
                  .map(
                    (member) =>
                      snapshot.displays.find(
                        (d) => d.id_key === member.display_key,
                      )?.friendly_name ?? "Display",
                  )
                  .join(", ")}
                . Resolution and scaling are shared, with choices supported by
                every monitor. Refresh rate, orientation and HDR stay per
                monitor.
              </p>
            )}
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
            <RefreshRateField
              output={output}
              cap={cap}
              onChange={(rate) => change({ refresh_rate_mhz: rate })}
            />
            {members
              .filter((member) => member !== output)
              .map((member) => (
                <div key={member.display_key} className="sm:col-span-2">
                  <RefreshRateField
                    output={member}
                    cap={snapshot.capabilities.find((c) =>
                      capabilityMatches(member, c),
                    )}
                    label={`Refresh rate — ${snapshot.displays.find((d) => d.id_key === member.display_key)?.friendly_name ?? "Display"}`}
                    onChange={(rate) =>
                      change({ refresh_rate_mhz: rate }, member.display_key)
                    }
                  />
                </div>
              ))}
            {!validMode && (
              <p
                role="alert"
                className="text-sm text-destructive sm:col-span-2"
              >
                {duplicated
                  ? "Select a supported refresh rate for each monitor at the shared resolution."
                  : "Select a refresh rate supported at this resolution."}
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
                disabled={!scales.length}
                onChange={(e) =>
                  change({
                    scale_percent:
                      e.target.value === "preserve"
                        ? null
                        : Number(e.target.value),
                  })
                }
              >
                {preserveScaling && (
                  <option value="preserve">Preserve scaling</option>
                )}
                {output.scale_percent != null &&
                  !scales.includes(output.scale_percent) && (
                    <option value={output.scale_percent}>
                      {output.scale_percent}% (current)
                    </option>
                  )}
                {scales.map((s) => (
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
            <label className="flex min-h-9 items-center gap-2 text-sm sm:col-span-2">
              <input
                type="checkbox"
                className="m-0 h-4 w-4 shrink-0 accent-primary"
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
            <Button variant="outline" disabled={locked} onClick={onClose}>
              Cancel
            </Button>
            <Button
              disabled={locked || !validMode}
              onClick={async () => {
                const validation = layoutError(draft, snapshot.capabilities);
                if (validation) {
                  setError(validation);
                  return;
                }
                setError(null);
                setSaving(true);
                try {
                  if (await onSave(rebaseLayout(draft))) onClose();
                  else
                    setError(
                      "Settings could not be applied. Review the reported error and try again.",
                    );
                } finally {
                  setSaving(false);
                }
              }}
            >
              {saving ? "Saving…" : "Save settings"}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
