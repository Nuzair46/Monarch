import type { AppSnapshot, CursorCalibration, OutputConfig } from "@/types";
import { capabilityMatches } from "./display-editor";
import { rectanglesOverlap, type MonitorRect } from "./arrangement";

export type CalibrationRow = CursorCalibration & { calibrated: boolean };
const sizeValid = (row: CursorCalibration) =>
  [row.width_mm, row.height_mm].every(
    (n) => Number.isInteger(n) && n >= 10 && n <= 10000,
  );
export const calibrationValid = (row: CursorCalibration) =>
  sizeValid(row) &&
  [row.position_mm.x, row.position_mm.y].every(
    (n) => Number.isInteger(n) && Math.abs(n) <= 1000000,
  );
const order = (o: OutputConfig) => {
  const [adapter = "", target = "", hash = "-"] = o.display_key.split(":");
  return [
    o.identity?.edid_serial ?? "",
    o.identity?.device_path ?? "",
    hash,
    adapter,
    target.padStart(10, "0"),
  ].join("\0");
};

export function calibrationSurfaces(
  snapshot: AppSnapshot,
  rows: CalibrationRow[],
): MonitorRect[] {
  const active = snapshot.layout.outputs
    .filter((o) => o.enabled)
    .sort((a, b) => (order(a) < order(b) ? -1 : order(a) > order(b) ? 1 : 0));
  const seen = new Set<string>();
  return active.flatMap((output) => {
    if (seen.has(output.display_key)) return [];
    const members = active.filter(
      (o) =>
        o === output ||
        Boolean(output.clone_group && o.clone_group === output.clone_group),
    );
    members.forEach((m) => seen.add(m.display_key));
    const representative =
      members.find((m) =>
        rows.some(
          (r) =>
            r.display_key === m.display_key &&
            r.clone_representative &&
            r.calibrated,
        ),
      ) ?? members[0];
    const row = rows.find(
      (r) =>
        r.display_key === representative.display_key &&
        r.calibrated &&
        calibrationValid(r),
    );
    if (!row) return [];
    const portrait =
      representative.rotation === "portrait" ||
      representative.rotation === "portrait_flipped";
    return [
      {
        key: row.display_key,
        ...row.position_mm,
        width: portrait ? row.height_mm : row.width_mm,
        height: portrait ? row.width_mm : row.height_mm,
        label: members
          .map(
            (m) =>
              snapshot.displays.findIndex((d) => d.id_key === m.display_key) +
              1,
          )
          .join(" + "),
        name:
          snapshot.displays.find((d) => d.id_key === row.display_key)
            ?.friendly_name ?? "Display",
      },
    ];
  });
}

export function calibrationConnections(surfaces: MonitorRect[]) {
  const overlapping = new Set<string>();
  for (const a of surfaces)
    for (const b of surfaces)
      if (a !== b && rectanglesOverlap(a, b)) {
        overlapping.add(a.key);
        overlapping.add(b.key);
      }
  const usable = surfaces.filter((s) => !overlapping.has(s.key));
  const connected = new Set<string>();
  let boundaries = 0;
  usable.forEach((a, i) =>
    usable.slice(i + 1).forEach((b) => {
      const vertical = a.y < b.y + b.height && b.y < a.y + a.height;
      const horizontal = a.x < b.x + b.width && b.x < a.x + a.width;
      if (
        (vertical &&
          (Math.abs(a.x + a.width - b.x) <= 1 ||
            Math.abs(b.x + b.width - a.x) <= 1)) ||
        (horizontal &&
          (Math.abs(a.y + a.height - b.y) <= 1 ||
            Math.abs(b.y + b.height - a.y) <= 1))
      ) {
        boundaries++;
        connected.add(a.key);
        connected.add(b.key);
      }
    }),
  );
  return {
    boundaries,
    overlapping,
    isolated: usable.filter((s) => !connected.has(s.key)),
  };
}

export function seedCalibration(
  snapshot: AppSnapshot,
  current?: CalibrationRow[],
  resetPositions = false,
): CalibrationRow[] {
  const saved =
    current ??
    snapshot.settings.cursor_calibrations.map((r) => ({
      ...r,
      calibrated: true,
    }));
  const rows = snapshot.displays.map((d) => {
    const output = snapshot.layout.outputs.find(
      (o) => o.display_key === d.id_key,
    );
    const previous = saved.find(
      (r) =>
        r.display_key === d.id_key &&
        !(
          r.identity?.edid_serial &&
          output?.identity?.edid_serial &&
          r.identity.edid_serial !== output.identity.edid_serial
        ),
    );
    if (previous) return structuredClone(previous);
    const cap =
      output && snapshot.capabilities.find((c) => capabilityMatches(output, c));
    const row = {
      display_key: d.id_key,
      identity: output?.identity,
      width_mm: cap?.physical_size_mm?.width ?? 0,
      height_mm: cap?.physical_size_mm?.height ?? 0,
      position_mm: { x: 0, y: 0 },
      clone_representative: false,
      calibrated: false,
    };
    row.calibrated = Boolean(output?.enabled && sizeValid(row));
    return row;
  });
  const surfaces = calibrationSurfaces(snapshot, rows);
  const placed = new Map<string, MonitorRect>();
  if (!resetPositions)
    for (const surface of surfaces)
      if (saved.some((r) => r.display_key === surface.key))
        placed.set(surface.key, surface);
  const pixel = (key: string) =>
    snapshot.layout.outputs.find((o) => o.display_key === key)!;
  const pending = surfaces.filter((s) => !placed.has(s.key));
  pending.sort(
    (a, b) => Number(pixel(b.key).primary) - Number(pixel(a.key).primary),
  );
  if (!placed.size && pending.length) {
    const first = pending.shift()!;
    placed.set(first.key, { ...first, x: 0, y: 0 });
  }
  // Use each panel's physical dimensions at Windows-adjacent edges. Applying
  // the primary panel's pixel pitch to every position creates overlaps/gaps.
  while (pending.length) {
    let found = false;
    for (let i = 0; i < pending.length && !found; i++) {
      const next = pending[i],
        b = pixel(next.key);
      for (const previous of placed.values()) {
        const a = pixel(previous.key);
        const offset = (
          aStart: number,
          aPixels: number,
          aMm: number,
          bStart: number,
          bPixels: number,
          bMm: number,
        ) =>
          aStart === bStart
            ? 0
            : aStart + aPixels === bStart + bPixels
              ? aMm - bMm
              : ((bStart - aStart) / aPixels) * aMm;
        let x: number, y: number;
        const verticalOverlap =
          a.position.y < b.position.y + b.resolution.height &&
          b.position.y < a.position.y + a.resolution.height;
        const horizontalOverlap =
          a.position.x < b.position.x + b.resolution.width &&
          b.position.x < a.position.x + a.resolution.width;
        if (
          verticalOverlap &&
          (Math.abs(a.position.x + a.resolution.width - b.position.x) <= 1 ||
            Math.abs(b.position.x + b.resolution.width - a.position.x) <= 1)
        ) {
          x =
            b.position.x > a.position.x
              ? previous.x + previous.width
              : previous.x - next.width;
          y =
            previous.y +
            offset(
              a.position.y,
              a.resolution.height,
              previous.height,
              b.position.y,
              b.resolution.height,
              next.height,
            );
        } else if (
          horizontalOverlap &&
          (Math.abs(a.position.y + a.resolution.height - b.position.y) <= 1 ||
            Math.abs(b.position.y + b.resolution.height - a.position.y) <= 1)
        ) {
          y =
            b.position.y > a.position.y
              ? previous.y + previous.height
              : previous.y - next.height;
          x =
            previous.x +
            offset(
              a.position.x,
              a.resolution.width,
              previous.width,
              b.position.x,
              b.resolution.width,
              next.width,
            );
        } else continue;
        placed.set(next.key, { ...next, x: Math.round(x), y: Math.round(y) });
        pending.splice(i, 1);
        found = true;
        break;
      }
    }
    if (!found) {
      const next = pending.shift()!;
      const right = Math.max(...[...placed.values()].map((s) => s.x + s.width));
      placed.set(next.key, { ...next, x: right + 20, y: 0 });
    }
  }
  for (const row of rows) {
    const surface = placed.get(row.display_key);
    if (surface) row.position_mm = { x: surface.x, y: surface.y };
  }
  return [
    ...rows,
    ...saved.filter((r) => !rows.some((d) => d.display_key === r.display_key)),
  ];
}
