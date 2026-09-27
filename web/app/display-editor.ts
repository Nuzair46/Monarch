import type {
  Layout,
  OutputConfig,
  DisplayCapabilities,
  Resolution,
  Position,
} from "@/types";
import {
  connectedPosition,
  edgesTouch,
  rectanglesOverlap,
} from "./arrangement";
export const sameSource = (a: OutputConfig, b: OutputConfig) =>
  a.enabled &&
  b.enabled &&
  Boolean(a.clone_group) &&
  a.clone_group === b.clone_group;
export function normalizeGroups(layout: Layout): Layout {
  for (const output of layout.outputs) {
    if (
      !output.enabled ||
      layout.outputs.filter((o) => sameSource(output, o)).length < 2
    )
      output.clone_group = null;
    if (!output.enabled) output.primary = false;
  }
  const primary =
    layout.outputs.find((o) => o.enabled && o.primary) ??
    layout.outputs.find((o) => o.enabled);
  for (const output of layout.outputs)
    output.primary = Boolean(
      primary &&
      output.enabled &&
      (output === primary || sameSource(output, primary)),
    );
  return layout;
}
export function changeAttachment(
  layout: Layout,
  key: string,
  mode: string,
): Layout {
  const draft = structuredClone(layout);
  const output = draft.outputs.find((o) => o.display_key === key)!;
  if (mode === "detached") {
    if (output.enabled && draft.outputs.filter((o) => o.enabled).length === 1)
      throw new Error("Cannot detach the last active display.");
    output.enabled = false;
    output.primary = false;
    output.clone_group = null;
  } else if (mode === "extend") {
    output.enabled = true;
    output.clone_group = null;
    output.position = {
      x: Math.max(
        0,
        ...draft.outputs
          .filter((o) => o !== output && o.enabled)
          .map((o) => o.position.x + o.resolution.width),
      ),
      y: 0,
    };
  } else {
    const target = draft.outputs.find(
      (o) => o.display_key === mode && o.enabled && o !== output,
    );
    if (!target) throw new Error("Choose an active display to duplicate.");
    const groups = new Set(draft.outputs.map((o) => o.clone_group));
    let group = target.clone_group;
    if (!group) {
      let index = 1;
      while (groups.has(`clone-${index}`)) index++;
      group = `clone-${index}`;
    }
    target.clone_group = group;
    Object.assign(output, {
      enabled: true,
      clone_group: group,
      position: { ...target.position },
      resolution: { ...target.resolution },
      scale_percent: target.scale_percent,
      refresh_rate_mhz: target.refresh_rate_mhz,
      primary: target.primary,
    });
  }
  return normalizeGroups(draft);
}
export function editOutput(
  layout: Layout,
  key: string,
  patch: Partial<OutputConfig>,
): Layout {
  const draft = structuredClone(layout);
  const output = draft.outputs.find((o) => o.display_key === key)!;
  Object.assign(output, patch);
  for (const member of draft.outputs.filter(
    (o) => o !== output && sameSource(o, output),
  )) {
    for (const field of ["position", "resolution", "scale_percent"] as const) {
      if (field in patch)
        Object.assign(member, { [field]: structuredClone(output[field]) });
    }
  }
  if (patch.primary) {
    const offset = { ...output.position };
    for (const member of draft.outputs) {
      member.primary =
        member.enabled && (member === output || sameSource(member, output));
      member.position.x -= offset.x;
      member.position.y -= offset.y;
    }
  }
  return normalizeGroups(draft);
}

export function capabilityMatches(
  output: OutputConfig,
  cap: DisplayCapabilities,
): boolean {
  return (
    output.display_key === cap.display_key &&
    !(
      output.identity?.edid_serial &&
      cap.identity?.edid_serial &&
      output.identity.edid_serial !== cap.identity.edid_serial
    )
  );
}

export function nativeResolution(output: OutputConfig): Resolution {
  return output.rotation === "portrait" ||
    output.rotation === "portrait_flipped"
    ? { width: output.resolution.height, height: output.resolution.width }
    : output.resolution;
}

export function resolutionChoices(
  cap: DisplayCapabilities | undefined,
): Resolution[] {
  return [
    ...new Map(
      (cap?.modes ?? []).map((m) => [
        `${m.resolution.width}x${m.resolution.height}`,
        m.resolution,
      ]),
    ).values(),
  ];
}

export function refreshChoices(
  cap: DisplayCapabilities | undefined,
  output: OutputConfig,
): number[] {
  const resolution = nativeResolution(output);
  return [
    ...new Set(
      (cap?.modes ?? [])
        .filter(
          (m) =>
            m.resolution.width === resolution.width &&
            m.resolution.height === resolution.height,
        )
        .map((m) => m.refresh_rate_mhz),
    ),
  ].sort((a, b) => a - b);
}

export function layoutError(
  layout: Layout,
  capabilities: DisplayCapabilities[],
): string | null {
  const active = layout.outputs.filter((o) => o.enabled);
  if (!active.length) return "Keep at least one monitor active.";
  for (const [index, output] of active.entries()) {
    if (
      Math.abs(output.position.x) > 1_000_000 ||
      Math.abs(output.position.y) > 1_000_000
    )
      return "Move monitors closer to the desktop origin.";
    const cap = capabilities.find((c) => capabilityMatches(output, c));
    if (!cap)
      return "A monitor is no longer available. Refresh the layout before saving.";
    if (
      cap.modes.length &&
      !refreshChoices(cap, output).some(
        (rate) => Math.abs(rate - output.refresh_rate_mhz) <= 2,
      )
    )
      return "Choose a supported resolution and refresh rate in the monitor’s properties.";
    for (const other of active.slice(index + 1)) {
      if (sameSource(output, other)) continue;
      if (
        output.position.x < other.position.x + other.resolution.width &&
        other.position.x < output.position.x + output.resolution.width &&
        output.position.y < other.position.y + other.resolution.height &&
        other.position.y < output.position.y + output.resolution.height
      )
        return "Monitors overlap. Drag them apart before saving the layout.";
    }
  }
  return desktopGeometryError(layout);
}

const rectangle = (output: OutputConfig) => ({
  ...output.position,
  ...output.resolution,
});
const sources = (layout: Layout) =>
  layout.outputs.filter(
    (o, i) =>
      o.enabled &&
      !layout.outputs.slice(0, i).some((other) => sameSource(o, other)),
  );

export function desktopGeometryError(layout: Layout): string | null {
  const active = sources(layout);
  if (
    active.some((a, i) =>
      active
        .slice(i + 1)
        .some((b) => rectanglesOverlap(rectangle(a), rectangle(b))),
    )
  )
    return "Monitors overlap. Drag them apart before saving the layout.";
  if (!active.length) return "Keep at least one monitor active.";
  const connected = new Set([active[0]]);
  let added = true;
  while (added) {
    added = false;
    for (const output of active) {
      if (
        !connected.has(output) &&
        [...connected].some((o) => edgesTouch(rectangle(o), rectangle(output)))
      ) {
        connected.add(output);
        added = true;
      }
    }
  }
  return connected.size === active.length
    ? null
    : "Display edges must touch. Drag the monitors together to remove gaps before saving.";
}

// Resolution and rotation can grow into a neighbour or leave a gap. Keep the
// desktop joined when applying properties without requiring a separate draft.
export function fitDesktop(layout: Layout): Layout {
  if (!desktopGeometryError(layout)) return layout;
  let result = structuredClone(layout);
  const pending = sources(result).sort(
    (a, b) => Number(b.primary) - Number(a.primary),
  );
  const placed = [pending.shift()!];
  if (!placed[0]) return layout;
  while (pending.length) {
    const index = pending.findIndex(
      (o) =>
        placed.some((p) => edgesTouch(rectangle(o), rectangle(p))) &&
        !placed.some((p) => rectanglesOverlap(rectangle(o), rectangle(p))),
    );
    const next = pending.splice(index < 0 ? 0 : index, 1)[0];
    const position = connectedPosition(
      rectangle(next),
      placed.map(rectangle),
      next.position,
    );
    result = editOutput(result, next.display_key, { position });
    placed.push({ ...next, position });
  }
  return rebaseLayout(result);
}

export function rebaseLayout(layout: Layout): Layout {
  const primary = layout.outputs.find((o) => o.enabled && o.primary);
  if (!primary) return layout;
  return {
    outputs: layout.outputs.map((o) => ({
      ...o,
      position: {
        x: o.position.x - primary.position.x,
        y: o.position.y - primary.position.y,
      },
    })),
  };
}

// Position drafts contain only offsets, so saving a monitor's properties never
// applies staged positions or replaces newer live mode/HDR/scaling settings.
export function arrangementDraft(
  layout: Layout,
  offsets: Record<string, Position>,
): Layout {
  const applied = new Set<string>();
  let result = structuredClone(layout);
  for (const output of layout.outputs) {
    if (!output.enabled || applied.has(output.display_key)) continue;
    const members = layout.outputs.filter(
      (o) => o === output || sameSource(o, output),
    );
    members.forEach((o) => applied.add(o.display_key));
    const offset = members.map((o) => offsets[o.display_key]).find(Boolean);
    if (offset)
      result = editOutput(result, output.display_key, {
        position: {
          x: output.position.x + offset.x,
          y: output.position.y + offset.y,
        },
      });
  }
  return result;
}
