import { useRef, useState } from "react";
import { Dialog } from "radix-ui";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { SelectField } from "@/components/ui/select-field";
import { X } from "lucide-react";
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
  const choices = rates.map((rate) => ({
    value: String(rate),
    label: `${rate / 1000} Hz`,
  }));
  if (!rates.includes(output.refresh_rate_mhz)) {
    const current = rates.some(
      (r) => Math.abs(r - output.refresh_rate_mhz) <= 2,
    );
    choices.unshift({
      value: String(output.refresh_rate_mhz),
      label: `${output.refresh_rate_mhz / 1000} Hz (${current ? "current" : "unavailable"})`,
    });
  }
  return (
    <SelectField
      label={label}
      value={String(output.refresh_rate_mhz)}
      choices={choices}
      disabled={!rates.length}
      onValueChange={(value) => onChange(Number(value))}
    />
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
  const opener = useRef(
    document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null,
  );
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
  const resolutionChoices = resolutions.map((r) => ({
    value: `${r.width}x${r.height}`,
    label: `${r.width} × ${r.height}`,
  }));
  if (!resolutionChoices.some((r) => r.value === resolutionKey))
    resolutionChoices.unshift({
      value: resolutionKey,
      label: `${dimensions.width} × ${dimensions.height} (current)`,
    });
  const scaleChoices = scales.map((scale) => ({
    value: String(scale),
    label: `${scale}%`,
  }));
  if (output.scale_percent != null && !scales.includes(output.scale_percent))
    scaleChoices.unshift({
      value: String(output.scale_percent),
      label: `${output.scale_percent}% (current)`,
    });
  if (preserveScaling)
    scaleChoices.unshift({ value: "preserve", label: "Preserve scaling" });

  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open && !locked) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/60" />
        <Dialog.Content
          className="fixed left-1/2 top-1/2 z-40 flex max-h-[90vh] w-[calc(100%-2rem)] max-w-[560px] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-lg border bg-background shadow-lg outline-none"
          onEscapeKeyDown={(event) => {
            if (locked) event.preventDefault();
          }}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            if (opener.current?.isConnected) opener.current.focus();
          }}
        >
          <div className="relative shrink-0 border-b px-5 py-4 pr-14">
            <Dialog.Title className="text-base font-semibold">
              {display?.friendly_name ?? "Display"} settings
            </Dialog.Title>
            <Dialog.Description className="mt-1 text-xs text-muted-foreground">
              Changes apply when you save. Confirm to keep them, or revert.
            </Dialog.Description>
            <Dialog.Close asChild>
              <Button
                variant="ghost"
                size="icon"
                disabled={locked}
                className="absolute right-3 top-3"
                aria-label="Close monitor settings"
              >
                <X />
              </Button>
            </Dialog.Close>
          </div>
          <div className="min-h-0 overflow-y-auto px-5 py-5">
            <fieldset disabled={locked} className="grid gap-4 sm:grid-cols-2">
              <SelectField
                label="Display mode"
                className="sm:col-span-2"
                value={attachment}
                choices={[
                  { value: "extend", label: "Extend" },
                  { value: "detached", label: "Detached" },
                  ...draft.outputs
                    .filter((o) => o !== output && o.enabled)
                    .map((o) => ({
                      value: o.display_key,
                      label: `Duplicate of ${snapshot.displays.find((d) => d.id_key === o.display_key)?.friendly_name ?? "Display"}`,
                    })),
                ]}
                onValueChange={(value) => {
                  try {
                    setDraft(
                      fitDesktop(
                        changeAttachment(
                          draft,
                          displayKey,
                          value,
                          snapshot.capabilities,
                        ),
                      ),
                    );
                    setError(null);
                  } catch (error) {
                    setError(
                      error instanceof Error ? error.message : String(error),
                    );
                  }
                }}
              />
              {duplicated && (
                <p className="rounded-md border bg-muted/20 p-3 text-xs leading-relaxed text-muted-foreground sm:col-span-2">
                  Mirrors{" "}
                  {members
                    .filter((member) => member !== output)
                    .map(
                      (member) =>
                        snapshot.displays.find(
                          (d) => d.id_key === member.display_key,
                        )?.friendly_name ?? "Display",
                    )
                    .join(", ")}
                  . Resolution and scaling are shared. Refresh rate, orientation
                  and HDR stay per monitor.
                </p>
              )}
              <SelectField
                label="Resolution"
                value={resolutionKey}
                choices={resolutionChoices}
                disabled={!resolutions.length}
                onValueChange={(value) => {
                  const resolution = resolutions.find(
                    (r) => `${r.width}x${r.height}` === value,
                  )!;
                  change({
                    resolution:
                      output.rotation === "portrait" ||
                      output.rotation === "portrait_flipped"
                        ? { width: resolution.height, height: resolution.width }
                        : { ...resolution },
                  });
                }}
              />
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
                  className="text-xs text-danger-text sm:col-span-2"
                >
                  {duplicated
                    ? "Select a supported refresh rate for each monitor at the shared resolution."
                    : "Select a refresh rate supported at this resolution."}
                </p>
              )}
              <SelectField
                label="Orientation"
                value={output.rotation ?? "landscape"}
                choices={rotations.map(([value, label]) => ({ value, label }))}
                onValueChange={(value) => {
                  const rotation = value as OutputConfig["rotation"];
                  change({
                    rotation,
                    resolution:
                      rotation === "portrait" || rotation === "portrait_flipped"
                        ? { width: dimensions.height, height: dimensions.width }
                        : { ...dimensions },
                  });
                }}
              />
              <SelectField
                label="Scaling"
                value={String(output.scale_percent ?? "preserve")}
                choices={scaleChoices}
                disabled={!scales.length}
                onValueChange={(value) =>
                  change({
                    scale_percent: value === "preserve" ? null : Number(value),
                  })
                }
              />
              <SelectField
                label="HDR"
                value={
                  output.hdr_enabled == null
                    ? "preserve"
                    : String(output.hdr_enabled)
                }
                choices={[
                  { value: "preserve", label: "Preserve HDR" },
                  { value: "true", label: "On" },
                  { value: "false", label: "Off" },
                ]}
                disabled={!cap?.hdr_supported}
                onValueChange={(value) =>
                  change({
                    hdr_enabled: value === "preserve" ? null : value === "true",
                  })
                }
              />
              <label className="flex min-h-9 items-center gap-2.5 self-end text-sm">
                <Checkbox
                  checked={output.primary}
                  disabled={!output.enabled || output.primary}
                  onCheckedChange={() => change({ primary: true })}
                />
                Primary display
              </label>
            </fieldset>
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
              <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
                Monitor positions will adjust to keep display edges joined and
                the primary display at the desktop origin.
              </p>
            )}
            {!cap && (
              <p className="mt-4 text-xs text-muted-foreground">
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
                <p
                  key={reason}
                  className="mt-3 text-xs leading-relaxed text-muted-foreground"
                >
                  {reason}
                </p>
              ))}
            {error && (
              <p role="alert" className="mt-4 text-sm text-danger-text">
                {error}
              </p>
            )}
          </div>
          <div className="flex shrink-0 justify-end gap-2 border-t bg-muted/10 px-5 py-4">
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
