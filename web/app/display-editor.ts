import type { Layout, OutputConfig } from "@/types";
export const sameSource = (a: OutputConfig, b: OutputConfig) => a.enabled && b.enabled && Boolean(a.clone_group) && a.clone_group === b.clone_group;
export function normalizeGroups(layout: Layout): Layout {
  for (const output of layout.outputs) {
    if (!output.enabled || layout.outputs.filter((o) => sameSource(output,o)).length < 2) output.clone_group = null;
    if (!output.enabled) output.primary = false;
  }
  const primary = layout.outputs.find((o)=>o.enabled && o.primary) ?? layout.outputs.find((o)=>o.enabled);
  for (const output of layout.outputs) output.primary = Boolean(primary && output.enabled && (output === primary || sameSource(output,primary)));
  return layout;
}
export function changeAttachment(layout: Layout, key: string, mode: string): Layout {
  const draft = structuredClone(layout);
  const output = draft.outputs.find((o)=>o.display_key === key)!;
  if (mode === "detached") {
    if (output.enabled && draft.outputs.filter((o)=>o.enabled).length === 1) throw new Error("Cannot detach the last active display.");
    output.enabled = false; output.primary = false; output.clone_group = null;
  } else if (mode === "extend") {
    output.enabled = true; output.clone_group = null;
    output.position = { x: Math.max(0,...draft.outputs.filter((o)=>o !== output && o.enabled).map((o)=>o.position.x+o.resolution.width)), y:0 };
  } else {
    const target = draft.outputs.find((o)=>o.display_key === mode && o.enabled && o !== output);
    if (!target) throw new Error("Choose an active display to duplicate.");
    const groups = new Set(draft.outputs.map((o)=>o.clone_group));
    let group = target.clone_group;
    if (!group) { let index = 1; while (groups.has(`clone-${index}`)) index++; group = `clone-${index}`; }
    target.clone_group = group;
    Object.assign(output, { enabled:true, clone_group:group, position:{...target.position}, resolution:{...target.resolution}, scale_percent:target.scale_percent, refresh_rate_mhz:target.refresh_rate_mhz, primary:target.primary });
  }
  return normalizeGroups(draft);
}
export function editOutput(layout: Layout, key: string, patch: Partial<OutputConfig>): Layout {
  const draft = structuredClone(layout);
  const output = draft.outputs.find((o)=>o.display_key === key)!;
  Object.assign(output,patch);
  for (const member of draft.outputs.filter((o)=>o !== output && sameSource(o,output))) {
    for (const field of ["position","resolution","scale_percent"] as const) {
      if (field in patch) Object.assign(member,{[field]:structuredClone(output[field])});
    }
  }
  if (patch.primary) {
    const offset = {...output.position};
    for (const member of draft.outputs) {
      member.primary = member.enabled && (member === output || sameSource(member,output));
      member.position.x -= offset.x; member.position.y -= offset.y;
    }
  }
  return normalizeGroups(draft);
}
