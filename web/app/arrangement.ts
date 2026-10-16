import type { Position } from "@/types";
export type Rect = Position & {
  width: number;
  height: number;
};
export type MonitorRect = Rect & {
  key: string;
  label: string;
  name: string;
  primary?: boolean;
};
export function snapPosition(
  moving: MonitorRect,
  others: MonitorRect[],
  position: Position,
  tolerance: number,
): Position {
  const nearest = (value: number, candidates: number[]) => {
    const match = candidates.sort(
      (a, b) => Math.abs(a - value) - Math.abs(b - value),
    )[0];
    return match !== undefined && Math.abs(match - value) <= tolerance
      ? match
      : value;
  };
  return {
    x: Math.round(
      nearest(
        position.x,
        others.flatMap((o) => [
          o.x - moving.width,
          o.x + o.width,
          o.x,
          o.x + o.width - moving.width,
          o.x + (o.width - moving.width) / 2,
        ]),
      ),
    ),
    y: Math.round(
      nearest(
        position.y,
        others.flatMap((o) => [
          o.y - moving.height,
          o.y + o.height,
          o.y,
          o.y + o.height - moving.height,
          o.y + (o.height - moving.height) / 2,
        ]),
      ),
    ),
  };
}
export function rectanglesOverlap(a: Rect, b: Rect): boolean {
  return (
    a.x < b.x + b.width &&
    b.x < a.x + a.width &&
    a.y < b.y + b.height &&
    b.y < a.y + a.height
  );
}

export function edgesTouch(a: Rect, b: Rect): boolean {
  return (
    ((a.x + a.width === b.x || b.x + b.width === a.x) &&
      a.y < b.y + b.height &&
      b.y < a.y + a.height) ||
    ((a.y + a.height === b.y || b.y + b.height === a.y) &&
      a.x < b.x + b.width &&
      b.x < a.x + a.width)
  );
}

export function connectedPosition(
  moving: Rect,
  others: Rect[],
  position: Position,
): Position {
  if (!others.length) return position;
  const at = { ...moving, ...position };
  if (
    others.some((o) => edgesTouch(at, o)) &&
    !others.some((o) => rectanglesOverlap(at, o))
  )
    return position;
  const clamp = (value: number, start: number, end: number) =>
    Math.max(start, Math.min(value, end));
  const candidates = others
    .flatMap((o) => {
      const x = clamp(position.x, o.x - moving.width + 1, o.x + o.width - 1);
      const y = clamp(position.y, o.y - moving.height + 1, o.y + o.height - 1);
      return [
        { x: o.x - moving.width, y },
        { x: o.x + o.width, y },
        { x, y: o.y - moving.height },
        { x, y: o.y + o.height },
        // Corner-aligned alternatives can avoid another display blocking a side.
        ...[o.x, o.x + o.width - moving.width].flatMap((x) => [
          { x, y: o.y - moving.height },
          { x, y: o.y + o.height },
        ]),
        ...[o.y, o.y + o.height - moving.height].flatMap((y) => [
          { x: o.x - moving.width, y },
          { x: o.x + o.width, y },
        ]),
      ];
    })
    .filter(
      (p) => !others.some((o) => rectanglesOverlap({ ...moving, ...p }, o)),
    );
  candidates.sort(
    (a, b) =>
      Math.hypot(a.x - position.x, a.y - position.y) -
      Math.hypot(b.x - position.x, b.y - position.y),
  );
  return candidates[0] ?? position;
}

export function closeDisconnectedGroups(monitors: MonitorRect[]): MonitorRect[] {
  const result = monitors.map((monitor) => ({ ...monitor }));
  const remaining = [...result];
  const groups: MonitorRect[][] = [];
  while (remaining.length) {
    const group = [remaining.pop()!];
    for (;;) {
      const index = remaining.findIndex((monitor) =>
        group.some((member) => edgesTouch(monitor, member) ||
          (monitor.x === member.x && monitor.y === member.y &&
            monitor.width === member.width && monitor.height === member.height)));
      if (index < 0) break;
      group.push(remaining.splice(index, 1)[0]);
    }
    groups.push(group);
  }
  if (groups.length < 2) return result;
  const primary = groups.findIndex((group) => group.some((monitor) => monitor.primary));
  if (primary < 0) throw new Error("gap closure requires a primary display");
  const attached = groups.splice(primary, 1)[0];
  while (groups.length) {
    let best: { distance: number; index: number; dx: number; dy: number } | undefined;
    for (const [index, group] of groups.entries()) {
      for (const moving of group) {
        for (const anchor of attached) {
          const x = Math.max(anchor.x - moving.width + 1, Math.min(moving.x, anchor.x + anchor.width - 1));
          const y = Math.max(anchor.y - moving.height + 1, Math.min(moving.y, anchor.y + anchor.height - 1));
          const candidates = [
            { x: anchor.x - moving.width, y }, { x: anchor.x + anchor.width, y },
            { x, y: anchor.y - moving.height }, { x, y: anchor.y + anchor.height },
            ...[anchor.y, anchor.y + anchor.height - moving.height].flatMap((y) => [
              { x: anchor.x - moving.width, y }, { x: anchor.x + anchor.width, y },
            ]),
            ...[anchor.x, anchor.x + anchor.width - moving.width].flatMap((x) => [
              { x, y: anchor.y - moving.height }, { x, y: anchor.y + anchor.height },
            ]),
          ];
          for (const position of candidates) {
            const dx = position.x - moving.x, dy = position.y - moving.y;
            const valid = group.every((member) => {
              const next = { ...member, x: member.x + dx, y: member.y + dy };
              return Math.abs(next.x) <= 1_000_000 && Math.abs(next.y) <= 1_000_000 &&
                result.filter((other) => !group.includes(other))
                  .every((other) => !rectanglesOverlap(next, other));
            });
            const distance = dx * dx + dy * dy;
            if (valid && (!best || distance < best.distance)) best = { distance, index, dx, dy };
          }
        }
      }
    }
    if (!best) throw new Error("cannot close gaps left by missing displays without overlap; adjust and re-save the profile");
    const group = groups.splice(best.index, 1)[0];
    for (const member of group) { member.x += best.dx; member.y += best.dy; }
    attached.push(...group);
  }
  return result;
}
