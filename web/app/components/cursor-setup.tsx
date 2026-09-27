import { useEffect, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MonitorArrangement } from "./monitor-arrangement";
import {
  calibrationConnections,
  calibrationSurfaces,
  calibrationValid,
  seedCalibration,
  type CalibrationRow,
} from "@/app/cursor-calibration";
import type { AppSettings, AppSnapshot } from "@/types";

export function CursorSetup({
  snapshot,
  busy,
  onSave,
}: {
  snapshot: AppSnapshot;
  busy: boolean;
  onSave: (settings: AppSettings) => Promise<boolean>;
}) {
  const [rows, setRows] = useState(() => seedCalibration(snapshot));
  const [enabled, setEnabled] = useState(
    snapshot.settings.cursor_correction_enabled,
  );
  const [dirty, setDirty] = useState(false);
  const signature = JSON.stringify([
    snapshot.settings.cursor_correction_enabled,
    snapshot.settings.cursor_calibrations,
    snapshot.layout,
    snapshot.capabilities.map((c) => [c.display_key, c.physical_size_mm]),
  ]);
  useEffect(() => {
    if (!dirty) {
      setRows(seedCalibration(snapshot));
      setEnabled(snapshot.settings.cursor_correction_enabled);
    }
  }, [signature]);
  function edit(key: string, patch: Partial<CalibrationRow>) {
    setDirty(true);
    setRows((current) =>
      current.map((r) => (r.display_key === key ? { ...r, ...patch } : r)),
    );
  }
  const calibrated = rows.filter((r) => r.calibrated);
  const valid = calibrated.every(calibrationValid);
  const surfaces = calibrationSurfaces(snapshot, rows);
  const connections = calibrationConnections(surfaces);
  const status = snapshot.cursor_status;
  const canRun = valid && connections.boundaries > 0;
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
          Keep the cursor at the same physical height or width when crossing
          monitors. Drag these monitors to match their real positions, with
          adjoining edges touching. Hold Ctrl to bypass correction.
        </p>
        <div className="border-y py-3 text-sm" role="status">
          {!status.platform_supported
            ? "Cursor correction runs in the Windows app; this is a browser preview."
            : !snapshot.settings.cursor_correction_enabled
              ? "Cursor alignment is off."
              : status.boundaries === 0
                ? "Cursor alignment is inactive. Fix the calibration below."
                : (status.pause_reason ??
                  (status.running
                    ? `Active across ${status.boundaries} ${status.boundaries === 1 ? "boundary" : "boundaries"}. ${status.corrected_crossings} corrected crossings this session.`
                    : "Waiting for the display configuration to settle."))}
          {dirty && (
            <p className="mt-1 text-muted-foreground">
              Unsaved calibration changes.
            </p>
          )}
        </div>
        <MonitorArrangement
          monitors={surfaces}
          disabled={busy}
          step={1}
          label="Physical monitor arrangement in millimeters"
          onMove={(key, position_mm) => edit(key, { position_mm })}
        />
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-sm text-muted-foreground">
            {connections.boundaries} adjoining{" "}
            {connections.boundaries === 1 ? "boundary" : "boundaries"}.
            Dimensions are the visible panel size, before rotation.
          </p>
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={() => {
              setRows(seedCalibration(snapshot, rows, true));
              setDirty(true);
            }}
          >
            Match Windows arrangement
          </Button>
        </div>
        {connections.overlapping.size > 0 && (
          <p role="alert" className="text-sm text-destructive">
            Physical monitors overlap. Drag them apart so their edges meet.
          </p>
        )}
        {connections.isolated.length > 0 && surfaces.length > 1 && (
          <p role="alert" className="text-sm text-destructive">
            Some physical monitors have no adjoining edge. Close the gaps in the
            arrangement above.
          </p>
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
              ? snapshot.layout.outputs.filter(
                  (o) => o.enabled && o.clone_group === output.clone_group,
                )
              : [];
            const representative = surfaces.some(
              (s) => s.key === row.display_key,
            );
            return (
              <section
                key={row.display_key}
                className="space-y-2 border-t pt-3"
                aria-label={`Calibration ${index + 1}`}
              >
                <label className="flex items-center gap-2 text-sm font-medium">
                  <input
                    type="checkbox"
                    checked={row.calibrated}
                    onChange={(e) =>
                      edit(row.display_key, { calibrated: e.target.checked })
                    }
                  />
                  Use {display?.friendly_name ?? "disconnected display"}
                </label>
                <div className="grid gap-3 sm:grid-cols-4">
                  {(
                    [
                      ["width_mm", "Width (mm)"],
                      ["height_mm", "Height (mm)"],
                    ] as const
                  ).map(([key, label]) => (
                    <label key={key} className="grid gap-1 text-sm">
                      {label}
                      <Input
                        type="number"
                        min={10}
                        max={10000}
                        value={row[key] || ""}
                        placeholder="Measure panel"
                        onChange={(e) =>
                          edit(row.display_key, {
                            [key]: Number(e.target.value),
                          })
                        }
                      />
                    </label>
                  ))}
                  {(["x", "y"] as const).map((axis) => (
                    <label key={axis} className="grid gap-1 text-sm">
                      {axis.toUpperCase()} (mm)
                      <Input
                        type="number"
                        min={-1000000}
                        max={1000000}
                        value={row.position_mm[axis]}
                        onChange={(e) =>
                          edit(row.display_key, {
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
                      disabled={!row.calibrated || !calibrationValid(row)}
                      name={`representative-${output!.clone_group}`}
                      checked={representative}
                      onChange={() => {
                        setDirty(true);
                        setRows(
                          rows.map((r) =>
                            members.some((m) => m.display_key === r.display_key)
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
                {!dirty &&
                  status.issues
                    .filter((i) => i.display_key === row.display_key)
                    .map((issue) => (
                      <p
                        key={issue.message}
                        className="text-sm text-destructive"
                      >
                        {issue.message}
                      </p>
                    ))}
              </section>
            );
          })}
        </fieldset>
        {!valid && (
          <p role="alert" className="text-sm text-destructive">
            Enter valid whole-number dimensions (10–10000 mm) and positions for
            each enabled monitor.
          </p>
        )}
        {enabled && !canRun && (
          <p role="alert" className="text-sm text-destructive">
            Cursor alignment needs at least two calibrated monitors with
            adjoining physical edges.
          </p>
        )}
        <Button
          disabled={busy || !dirty || !valid || (enabled && !canRun)}
          onClick={() => {
            void onSave({
              ...snapshot.settings,
              cursor_correction_enabled: enabled,
              cursor_calibrations: calibrated.map(
                ({ calibrated: _, ...row }) => row,
              ),
            }).then((ok) => {
              if (ok) setDirty(false);
            });
          }}
        >
          Save cursor settings
        </Button>
        <p className="text-xs text-muted-foreground">
          Calibration is shared by all profiles. Pointer speed inside each
          monitor stays unchanged. With alignment active, cross an edge and
          check that the corrected-crossings count increases.
        </p>
      </CardContent>
    </Card>
  );
}
