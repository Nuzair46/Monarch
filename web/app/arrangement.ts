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
