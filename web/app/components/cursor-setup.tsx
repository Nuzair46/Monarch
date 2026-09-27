import { useEffect, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { AppSettings, AppSnapshot, CursorCalibration } from "@/types";

type Row = CursorCalibration & { calibrated: boolean };
function seed(snapshot: AppSnapshot): Row[] {
  const saved = snapshot.settings.cursor_calibrations;
  const root = snapshot.layout.outputs.find((o) => o.enabled && o.primary);
  const size = snapshot.capabilities.find(
    (c) => c.display_key === root?.display_key,
  )?.physical_size_mm;
  return [
    ...snapshot.displays.map((d) => {
      const previous = saved.find((c) => c.display_key === d.id_key);
      if (previous) return { ...previous, calibrated: true };
      const output = snapshot.layout.outputs.find(
        (o) => o.display_key === d.id_key,
      );
      const dimensions = snapshot.capabilities.find(
        (c) => c.display_key === d.id_key,
      )?.physical_size_mm;
      return {
        display_key: d.id_key,
        width_mm: dimensions?.width ?? 0,
        height_mm: dimensions?.height ?? 0,
        position_mm: {
          x: Math.round(
            ((output?.position.x ?? 0) * (size?.width ?? 0)) /
              (root?.resolution.width || 1),
          ),
          y: Math.round(
            ((output?.position.y ?? 0) * (size?.height ?? 0)) /
              (root?.resolution.height || 1),
          ),
        },
        clone_representative: false,
        calibrated: false,
      };
    }),
    ...saved
      .filter((c) => !snapshot.displays.some((d) => d.id_key === c.display_key))
      .map((c) => ({ ...c, calibrated: true })),
  ];
}
export function CursorSetup({
  snapshot,
  busy,
  onSave,
}: {
  snapshot: AppSnapshot;
  busy: boolean;
  onSave: (settings: AppSettings) => Promise<boolean>;
}) {
  const [rows, setRows] = useState(() => seed(snapshot));
  const [enabled, setEnabled] = useState(
    snapshot.settings.cursor_correction_enabled,
  );
  const [dirty, setDirty] = useState(false);
  const signature = JSON.stringify([
    snapshot.settings.cursor_correction_enabled,
    snapshot.settings.cursor_calibrations,
    snapshot.displays.map((d) => d.id_key),
  ]);
  useEffect(() => {
    if (!dirty) {
      setRows(seed(snapshot));
      setEnabled(snapshot.settings.cursor_correction_enabled);
    }
  }, [signature]);
  function edit(index: number, patch: Partial<Row>) {
    setDirty(true);
    setRows(rows.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  }
  const calibrated = rows.filter((r) => r.calibrated);
  const valid = calibrated.every(
    (r) =>
      [r.width_mm, r.height_mm].every(
        (n) => Number.isInteger(n) && n >= 10 && n <= 10000,
      ) &&
      [r.position_mm.x, r.position_mm.y].every(
        (n) => Number.isInteger(n) && Math.abs(n) <= 1000000,
      ),
  );
  const surfaces = rows
    .filter((r) => r.calibrated)
    .filter((r) => {
      const output = snapshot.layout.outputs.find(
        (o) => o.display_key === r.display_key,
      );
      if (!output?.enabled) return false;
      if (!output.clone_group) return true;
      const members = snapshot.displays.filter((d) =>
        snapshot.layout.outputs.some(
          (o) =>
            o.display_key === d.id_key &&
            o.enabled &&
            o.clone_group === output.clone_group,
        ),
      );
      const representative =
        members.find(
          (d) =>
            rows.find((c) => c.display_key === d.id_key)?.clone_representative,
        ) ?? members[0];
      return representative?.id_key === r.display_key;
    })
    .map((r) => {
      const rotation = snapshot.layout.outputs.find(
        (o) => o.display_key === r.display_key,
      )?.rotation;
      return {
        ...r,
        w:
          rotation === "portrait" || rotation === "portrait_flipped"
            ? r.height_mm
            : r.width_mm,
        h:
          rotation === "portrait" || rotation === "portrait_flipped"
            ? r.width_mm
            : r.height_mm,
      };
    });
  const left = Math.min(0, ...surfaces.map((r) => r.position_mm.x));
  const top = Math.min(0, ...surfaces.map((r) => r.position_mm.y));
  const width =
    Math.max(1, ...surfaces.map((r) => r.position_mm.x + r.w)) - left;
  const height =
    Math.max(1, ...surfaces.map((r) => r.position_mm.y + r.h)) - top;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Cursor alignment</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={enabled}
            disabled={busy}
            onChange={(e) => {
              setEnabled(e.target.checked);
              setDirty(true);
            }}
          />
          Align cursor across monitors
        </label>
        <p className="text-sm text-muted-foreground">
          Measure the visible panel, without the bezel. Dimensions use
          millimeters before rotation; X and Y place the rotated panel’s
          top-left corner. Adjacent physical edges should touch. Hold Ctrl to
          bypass correction.
        </p>
        <p className="text-sm text-muted-foreground">
          Calibration is shared by all profiles. Only calibrated monitors
          participate; normal pointer speed stays unchanged.
        </p>
        {surfaces.length > 0 && valid && (
          <svg
            className="h-40 w-full rounded-md border bg-muted/30 p-2"
            viewBox={`${left - 15} ${top - 15} ${width + 30} ${height + 30}`}
            role="img"
            aria-label="Physical monitor arrangement in millimeters"
          >
            {surfaces.map((r) => (
              <g key={r.display_key}>
                <rect
                  x={r.position_mm.x}
                  y={r.position_mm.y}
                  width={r.w}
                  height={r.h}
                  className="fill-background stroke-foreground"
                  strokeWidth={2}
                />
                <text
                  x={r.position_mm.x + r.w / 2}
                  y={r.position_mm.y + r.h / 2}
                  textAnchor="middle"
                  className="fill-foreground"
                  fontSize={Math.max(18, Math.min(r.w, r.h) / 8)}
                >
                  {snapshot.displays.findIndex(
                    (d) => d.id_key === r.display_key,
                  ) + 1}
                </text>
              </g>
            ))}
          </svg>
        )}
        <fieldset disabled={busy} className="space-y-4">
          {rows.map((row, index) => {
            const display = snapshot.displays.find(
              (d) => d.id_key === row.display_key,
            );
            const output = snapshot.layout.outputs.find(
              (o) => o.display_key === row.display_key,
            );
            const members = output?.clone_group
              ? snapshot.displays.filter((d) =>
                  snapshot.layout.outputs.some(
                    (o) =>
                      o.display_key === d.id_key &&
                      o.enabled &&
                      o.clone_group === output.clone_group,
                  ),
                )
              : [];
            const representative =
              members.find(
                (d) =>
                  rows.find((r) => r.display_key === d.id_key)
                    ?.clone_representative,
              ) ?? members[0];
            return (
              <section
                className="space-y-2 border-t pt-3"
                aria-label={`Calibration ${index + 1}`}
                key={row.display_key}
              >
                <label className="flex items-center gap-2 text-sm font-medium">
                  <input
                    type="checkbox"
                    checked={row.calibrated}
                    onChange={(e) =>
                      edit(index, { calibrated: e.target.checked })
                    }
                  />
                  Calibrate {display?.friendly_name ?? "disconnected display"}
                </label>
                <div className="grid gap-3 sm:grid-cols-4">
                  {(
                    [
                      ["width_mm", "Width (mm)"],
                      ["height_mm", "Height (mm)"],
                    ] as const
                  ).map(([key, label]) => (
                    <label className="grid gap-1 text-sm" key={key}>
                      {label}
                      <Input
                        type="number"
                        min={10}
                        max={10000}
                        value={row[key] || ""}
                        placeholder="Measure panel"
                        onChange={(e) =>
                          edit(index, { [key]: Number(e.target.value) })
                        }
                      />
                    </label>
                  ))}
                  {(["x", "y"] as const).map((axis) => (
                    <label className="grid gap-1 text-sm" key={axis}>
                      {axis.toUpperCase()} (mm)
                      <Input
                        type="number"
                        value={row.position_mm[axis]}
                        onChange={(e) =>
                          edit(index, {
                            position_mm: {
                              ...row.position_mm,
                              [axis]: Number(e.target.value),
                            },
                          })
                        }
                      />
                    </label>
                  ))}
                </div>
                {members.length > 1 && (
                  <label className="flex items-center gap-2 text-sm">
                    <input
                      type="radio"
                      name={`representative-${output!.clone_group}`}
                      checked={representative?.id_key === row.display_key}
                      onChange={() => {
                        setDirty(true);
                        setRows(
                          rows.map((r) =>
                            members.some((d) => d.id_key === r.display_key)
                              ? {
                                  ...r,
                                  clone_representative:
                                    r.display_key === row.display_key,
                                }
                              : r,
                          ),
                        );
                      }}
                    />
                    Use this physical monitor for the duplicated desktop
                  </label>
                )}
                {!display && (
                  <p className="text-xs text-muted-foreground">
                    Disconnected; calibration is retained.
                  </p>
                )}
              </section>
            );
          })}
        </fieldset>
        {!valid && (
          <p role="alert" className="text-sm text-destructive">
            Enter valid whole-number panel dimensions (10–10000 mm) and
            positions for each calibrated monitor.
          </p>
        )}
        {enabled && surfaces.length < 2 && (
          <p className="text-sm text-muted-foreground">
            Correction needs at least two calibrated, active desktop surfaces
            with adjoining physical edges.
          </p>
        )}
        <Button
          disabled={busy || !dirty || !valid}
          onClick={() => {
            const cursor_calibrations = calibrated.map(
              ({ calibrated: _, ...calibration }) => calibration,
            );
            void onSave({
              ...snapshot.settings,
              cursor_correction_enabled: enabled,
              cursor_calibrations,
            }).then((ok) => {
              if (ok) setDirty(false);
            });
          }}
        >
          Save cursor settings
        </Button>
      </CardContent>
    </Card>
  );
}
