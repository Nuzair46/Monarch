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
  capabilities: DisplayCapabilities[],
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
    if (sameSource(output, target)) return draft;
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
      primary: target.primary,
    });
    const members = sourceMembers(draft, output);
    const resolutions = sharedResolutionChoices(draft, output, capabilities);
    if (!resolutions.length)
      throw new Error(
        "Windows reports no shared resolution for these monitors. Extend and refresh them to enumerate their modes before duplicating.",
      );
    const preferred = nativeResolution({
      ...output,
      resolution: target.resolution,
    });
    const own = nativeResolution(output);
    const resolution =
      resolutions.find((r) => sameResolution(r, preferred)) ??
      resolutions.find((r) => sameResolution(r, own)) ??
      [...resolutions].sort(
        (a, b) => b.width * b.height - a.width * a.height,
      )[0];
    const sharedResolution = orientedResolution(resolution, output.rotation);
    const scales = sharedScaleChoices(draft, output, capabilities);
    const targetScale =
      target.scale_percent ??
      capabilities.find((c) => capabilityMatches(target, c))?.scale_percent;
    const ownScale =
      output.scale_percent ??
      capabilities.find((c) => capabilityMatches(output, c))?.scale_percent;
    const scale =
      [targetScale, ownScale, ...scales].find(
        (value): value is number => value != null && scales.includes(value),
      ) ?? null;
    if (scale == null && !canPreserveScaling(draft, output, capabilities))
      throw new Error(
        "Windows cannot report a shared scaling value for these monitors. Extend and refresh them before duplicating.",
      );
    for (const member of members) {
      member.resolution = { ...sharedResolution };
      member.scale_percent = scale;
      const cap = capabilities.find((c) => capabilityMatches(member, c));
      const rates = refreshChoices(cap, member);
      if (!rates.some((r) => Math.abs(r - member.refresh_rate_mhz) <= 2)) {
        member.refresh_rate_mhz = [...rates].sort(
          (a, b) =>
            Math.abs(a - member.refresh_rate_mhz) -
              Math.abs(b - member.refresh_rate_mhz) || b - a,
        )[0];
      }
    }
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
  return orientedResolution(output.resolution, output.rotation);
}

function orientedResolution(
  resolution: Resolution,
  rotation: OutputConfig["rotation"],
): Resolution {
  return rotation === "portrait" || rotation === "portrait_flipped"
    ? { width: resolution.height, height: resolution.width }
    : resolution;
}

const sameResolution = (a: Resolution, b: Resolution) =>
  a.width === b.width && a.height === b.height;

export function sourceMembers(
  layout: Layout,
  output: OutputConfig,
): OutputConfig[] {
  return layout.outputs.filter((o) => o === output || sameSource(output, o));
}

// Clone members share a desktop surface, while each target keeps its own
// rotation, refresh and HDR. Compare capabilities in desktop orientation.
export function sharedResolutionChoices(
  layout: Layout,
  output: OutputConfig,
  capabilities: DisplayCapabilities[],
): Resolution[] {
  const cap = capabilities.find((c) => capabilityMatches(output, c));
  const members = sourceMembers(layout, output);
  return resolutionChoices(cap).filter((resolution) => {
    const desktop = orientedResolution(resolution, output.rotation);
    return members.every((member) => {
      const native = orientedResolution(desktop, member.rotation);
      return capabilities
        .find((c) => capabilityMatches(member, c))
        ?.modes.some(
          (m) => sameResolution(m.resolution, native) && m.refresh_rate_mhz > 0,
        );
    });
  });
}

export function sharedScaleChoices(
  layout: Layout,
  output: OutputConfig,
  capabilities: DisplayCapabilities[],
): number[] {
  const members = sourceMembers(layout, output);
  const cap = capabilities.find((c) => capabilityMatches(output, c));
  return (cap?.scale_percentages ?? []).filter((scale) =>
    members.every((member) => {
      const other = capabilities.find((c) => capabilityMatches(member, c));
      return (
        other?.scale_percent != null && other.scale_percentages.includes(scale)
      );
    }),
  );
}

export function canPreserveScaling(
  layout: Layout,
  output: OutputConfig,
  capabilities: DisplayCapabilities[],
): boolean {
  const scales = sourceMembers(layout, output).map(
    (member) =>
      capabilities.find((c) => capabilityMatches(member, c))?.scale_percent ??
      null,
  );
  return scales.every((scale) => scale === scales[0]);
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
            m.resolution.height === resolution.height &&
            m.refresh_rate_mhz > 0,
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
