import type { Position } from "@/types";
export type MonitorRect = Position & {
  key: string;
  width: number;
  height: number;
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
        ]),
      ),
    ),
  };
}
export function rectanglesOverlap(a: MonitorRect, b: MonitorRect): boolean {
  return (
    a.x < b.x + b.width &&
    b.x < a.x + a.width &&
    a.y < b.y + b.height &&
    b.y < a.y + a.height
  );
}
