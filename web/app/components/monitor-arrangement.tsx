import { useRef, useState } from "react";
import { snapPosition, type MonitorRect } from "@/app/arrangement";
import type { Position } from "@/types";

export function MonitorArrangement({
  monitors,
  disabled,
  onMove,
  label,
  step = 10,
}: {
  monitors: MonitorRect[];
  disabled?: boolean;
  onMove?: (key: string, position: Position) => void;
  label: string;
  step?: number;
}) {
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{
    pointer: number;
    monitor: MonitorRect;
    start: Position;
  } | null>(null);
  const [frozenBounds, setFrozenBounds] = useState<string | null>(null);
  if (!monitors.length)
    return (
      <div className="grid h-64 place-items-center rounded-md border text-sm text-muted-foreground">
        No active monitors
      </div>
    );
  const left = Math.min(...monitors.map((m) => m.x));
  const top = Math.min(...monitors.map((m) => m.y));
  const width = Math.max(...monitors.map((m) => m.x + m.width)) - left;
  const height = Math.max(...monitors.map((m) => m.y + m.height)) - top;
  const padding = Math.max(width, height) * 0.16;
  const bounds =
    frozenBounds ??
    `${left - padding} ${top - padding} ${width + padding * 2} ${height + padding * 2}`;
  const editable = Boolean(onMove && !disabled);
  function point(x: number, y: number): Position | null {
    const matrix = svg.current?.getScreenCTM();
    if (!matrix) return null;
    const p = new DOMPoint(x, y).matrixTransform(matrix.inverse());
    return { x: p.x, y: p.y };
  }
  function finish(cancel: boolean) {
    if (cancel && drag.current)
      onMove?.(drag.current.monitor.key, {
        x: drag.current.monitor.x,
        y: drag.current.monitor.y,
      });
    drag.current = null;
    setFrozenBounds(null);
  }
  return (
    <svg
      ref={svg}
      className="h-80 w-full touch-none select-none rounded-md border bg-muted/20"
      viewBox={bounds}
      aria-label={label}
      onPointerMove={(event) => {
        const current = drag.current;
        if (!current || event.pointerId !== current.pointer || !editable)
          return;
        const next = point(event.clientX, event.clientY);
        if (!next) return;
        onMove?.(
          current.monitor.key,
          snapPosition(
            current.monitor,
            monitors.filter((m) => m.key !== current.monitor.key),
            {
              x: current.monitor.x + next.x - current.start.x,
              y: current.monitor.y + next.y - current.start.y,
            },
            10 / (svg.current?.getScreenCTM()?.a || 1),
          ),
        );
      }}
      onPointerUp={() => finish(false)}
      onPointerCancel={() => finish(true)}
      onKeyDown={(event) => {
        if (event.key === "Escape" && drag.current) {
          event.preventDefault();
          finish(true);
        }
      }}
    >
      {monitors.map((monitor) => (
        <g
          key={monitor.key}
          role={editable ? "button" : "img"}
          tabIndex={editable ? 0 : undefined}
          aria-label={`${editable ? "Move monitor" : "Monitor"} ${monitor.label}: ${monitor.name}`}
          data-monitor-key={monitor.key}
          className={
            editable
              ? "group cursor-grab focus:outline-none active:cursor-grabbing"
              : "group"
          }
          onPointerDown={(event) => {
            if (!editable || event.button !== 0) return;
            const start = point(event.clientX, event.clientY);
            if (!start) return;
            event.preventDefault();
            event.currentTarget.focus();
            svg.current?.setPointerCapture(event.pointerId);
            drag.current = {
              pointer: event.pointerId,
              monitor: { ...monitor },
              start,
            };
            setFrozenBounds(bounds);
          }}
          onKeyDown={(event) => {
            if (!editable) return;
            const offsets: Record<string, Position> = {
              ArrowLeft: { x: -1, y: 0 },
              ArrowRight: { x: 1, y: 0 },
              ArrowUp: { x: 0, y: -1 },
              ArrowDown: { x: 0, y: 1 },
            };
            const offset = offsets[event.key];
            if (!offset) return;
            event.preventDefault();
            const distance = event.shiftKey ? step * 10 : step;
            onMove?.(monitor.key, {
              x: monitor.x + offset.x * distance,
              y: monitor.y + offset.y * distance,
            });
          }}
        >
          <rect
            x={monitor.x}
            y={monitor.y}
            width={monitor.width}
            height={monitor.height}
            className="fill-background stroke-foreground/40 group-focus-visible:stroke-primary"
            strokeWidth="2"
            vectorEffect="non-scaling-stroke"
          />
          <text
            x={monitor.x + monitor.width / 2}
            y={monitor.y + monitor.height / 2}
            textAnchor="middle"
            dominantBaseline="central"
            fontSize={Math.min(monitor.width, monitor.height) * 0.2}
            className="pointer-events-none fill-foreground"
          >
            {monitor.label}
          </text>
          {monitor.primary && (
            <text
              x={monitor.x + monitor.width / 2}
              y={monitor.y + monitor.height * 0.82}
              textAnchor="middle"
              fontSize={Math.min(monitor.width, monitor.height) * 0.07}
              className="pointer-events-none fill-muted-foreground"
            >
              Primary
            </text>
          )}
        </g>
      ))}
    </svg>
  );
}
