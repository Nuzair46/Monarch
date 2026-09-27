import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import ts from "typescript";

// Exercise the actual standalone TypeScript modules without a browser or new test dependency.
function compiledModule(file, imports = {}) {
  let source = fs.readFileSync(
    new URL(`../../${file}`, import.meta.url),
    "utf8",
  );
  for (const [specifier, url] of Object.entries(imports)) {
    source = source
      .replaceAll(`'${specifier}'`, JSON.stringify(url))
      .replaceAll(`"${specifier}"`, JSON.stringify(url));
  }
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: {
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.ESNext,
    },
  });
  return `data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`;
}

const { subscriptions } = await import(
  compiledModule("web/app/subscriptions.ts")
);

test("subscriptions dispose registrations that finish after unmount", async () => {
  let resolve;
  let calls = 0;
  const listeners = subscriptions((error) => {
    throw error;
  });
  listeners.add(
    new Promise((done) => {
      resolve = done;
    }),
  );
  listeners.dispose();
  resolve(() => calls++);
  await Promise.resolve();
  assert.equal(calls, 1);
  listeners.dispose();
  assert.equal(calls, 1);
});

test("subscriptions dispose existing listeners and surface only live failures", async () => {
  let calls = 0;
  const errors = [];
  const listeners = subscriptions((error) => errors.push(error));
  listeners.add(Promise.resolve(() => calls++));
  listeners.add(Promise.reject("registration failed"));
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(errors, ["registration failed"]);
  listeners.dispose();
  listeners.add(Promise.reject("unmounted"));
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(calls, 1);
  assert.equal(errors.length, 1);
});

test("browser mock obeys confirmation, rollback, timeout and custom-shortcut contracts", async () => {
  const mock = await import(
    compiledModule("web/mock.ts", {
      "@/app/ui": compiledModule("web/app/ui.ts"),
      "@/app/display-editor": compiledModule("web/app/display-editor.ts"),
    })
  );
  const initial = await mock.getSnapshot();
  await mock.toggleDisplay(initial.displays[1].id_key);
  assert.ok((await mock.getSnapshot()).pending_confirmation);
  await assert.rejects(mock.saveProfile("unconfirmed"));
  await assert.rejects(mock.applyProfile("Desk"));
  await mock.rollbackPending();
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  await mock.applyProfile("Focus");
  await mock.confirmCurrentLayout();
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
  await assert.rejects(mock.toggleDisplay(initial.displays[0].id_key));
  await mock.restoreLastLayout();
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  await mock.updateSettings({
    ...initial.settings,
    revert_timeout_secs: 1,
    profile_shortcut_base: null,
    profile_shortcuts: { Desk: "Ctrl+Alt+W" },
  });
  assert.equal((await mock.getSnapshot()).settings.profile_shortcut_base, null);
  await mock.toggleDisplay(initial.displays[1].id_key);
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
});

test("display drafts remain local until applied and support reverting preferences", async () => {
  const mock = await import(
    compiledModule("web/mock.ts", {
      "@/app/ui": compiledModule("web/app/ui.ts"),
      "@/app/display-editor": compiledModule("web/app/display-editor.ts"),
    }) + "#drafts"
  );
  const { editOutput, changeAttachment } = await import(
    compiledModule("web/app/display-editor.ts")
  );
  const initial = await mock.getSnapshot();
  let draft = editOutput(initial.layout, initial.displays[0].id_key, {
    hdr_enabled: true,
    scale_percent: 150,
    refresh_rate_mhz: 60000,
  });
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
  await mock.applyLayout(draft);
  assert.equal((await mock.getSnapshot()).layout.outputs[0].hdr_enabled, true);
  await mock.rollbackPending();
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  draft = changeAttachment(
    draft,
    initial.displays[1].id_key,
    initial.displays[0].id_key,
  );
  assert.equal(draft.outputs[1].clone_group, draft.outputs[0].clone_group);
  assert.equal(draft.outputs[1].primary, true);
  await mock.applyLayout(draft);
  await mock.rollbackPending();
  const extended = changeAttachment(
    draft,
    initial.displays[1].id_key,
    "extend",
  );
  assert.equal(extended.outputs[0].clone_group, null);
  assert.ok(
    extended.outputs[1].position.x >= extended.outputs[0].resolution.width,
  );
  const detached = changeAttachment(
    draft,
    initial.displays[0].id_key,
    "detached",
  );
  assert.equal(detached.outputs[1].clone_group, null);
  assert.equal(detached.outputs[1].enabled, true);
  const missing = structuredClone(initial.layout);
  missing.outputs[1].display_key = "missing";
  await assert.rejects(mock.applyLayout(missing), /unavailable/);
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  const caps = await mock.getDisplayCapabilities();
  assert.ok(caps[0].modes.some((m) => m.refresh_rate_mhz === 59940));
  assert.equal(caps[1].hdr_supported, false);
  assert.ok(caps[2].scaling_unavailable_reason);
  await assert.rejects(
    mock.applyLayout(
      editOutput(initial.layout, initial.displays[1].id_key, {
        hdr_enabled: true,
      }),
    ),
    /HDR/,
  );
});

test("cursor calibration remains global across profile apply and validates physical dimensions", async () => {
  const mock = await import(
    compiledModule("web/mock.ts", {
      "@/app/ui": compiledModule("web/app/ui.ts"),
      "@/app/display-editor": compiledModule("web/app/display-editor.ts"),
    }) + "#cursor"
  );
  const initial = await mock.getSnapshot();
  assert.equal(initial.settings.cursor_correction_enabled, false);
  const cursor_calibrations = initial.displays.slice(0, 2).map((d, i) => ({
    display_key: d.id_key,
    width_mm: 600,
    height_mm: 340,
    position_mm: { x: i * 600, y: 0 },
    clone_representative: false,
  }));
  await mock.updateSettings({
    ...initial.settings,
    cursor_correction_enabled: true,
    cursor_calibrations,
  });
  await mock.applyProfile("Focus");
  await mock.confirmCurrentLayout();
  assert.deepEqual(
    (await mock.getSnapshot()).settings.cursor_calibrations,
    cursor_calibrations,
  );
  await assert.rejects(
    mock.updateSettings({
      ...initial.settings,
      cursor_calibrations: [{ ...cursor_calibrations[0], width_mm: 0 }],
    }),
    /Invalid cursor/,
  );
  assert.equal(
    (await mock.getSnapshot()).settings.cursor_correction_enabled,
    true,
  );
  await mock.updateSettings({ ...initial.settings, cursor_calibrations });
  assert.equal(
    (await mock.getSnapshot()).settings.cursor_correction_enabled,
    false,
  );
});

test("profile capabilities never borrow a replacement monitor on the saved port", async () => {
  const { capabilityMatches } = await import(
    compiledModule("web/app/display-editor.ts")
  );
  const saved = {
    display_key: "same-port",
    identity: { device_path: "port", edid_serial: "original" },
  };
  assert.equal(
    capabilityMatches(saved, {
      display_key: "same-port",
      identity: { device_path: "port", edid_serial: "replacement" },
    }),
    false,
  );
  assert.equal(
    capabilityMatches(saved, {
      display_key: "same-port",
      identity: saved.identity,
    }),
    true,
  );
  assert.equal(
    capabilityMatches(saved, {
      display_key: "other-port",
      identity: saved.identity,
    }),
    false,
  );
});

const editor = await import(compiledModule("web/app/display-editor.ts"));
const arrangement = await import(compiledModule("web/app/arrangement.ts"));
const calibration = await import(
  compiledModule("web/app/cursor-calibration.ts", {
    "./display-editor": compiledModule("web/app/display-editor.ts"),
    "./arrangement": compiledModule("web/app/arrangement.ts"),
  })
);
async function fixture() {
  const mock = await import(
    compiledModule("web/mock.ts", {
      "@/app/ui": compiledModule("web/app/ui.ts"),
      "@/app/display-editor": compiledModule("web/app/display-editor.ts"),
    }) + "#arrangement-tests"
  );
  return structuredClone(await mock.getSnapshot());
}

test("resolution and refresh are independent, with rates constrained by resolution", async () => {
  const snapshot = await fixture();
  const output = snapshot.layout.outputs[0];
  const cap = snapshot.capabilities[0];
  assert.equal(editor.resolutionChoices(cap).length, 2);
  const changed = editor.editOutput(snapshot.layout, output.display_key, {
    resolution: { width: 1920, height: 1080 },
  });
  assert.equal(changed.outputs[0].refresh_rate_mhz, output.refresh_rate_mhz);
  assert.deepEqual(
    editor.refreshChoices(cap, changed.outputs[0]),
    [59940, 60000],
  );
  assert.match(editor.layoutError(changed, snapshot.capabilities), /refresh/);
  const rate = editor.editOutput(changed, output.display_key, {
    refresh_rate_mhz: 59940,
  });
  assert.deepEqual(rate.outputs[0].resolution, changed.outputs[0].resolution);
  assert.equal(rate.outputs[0].refresh_rate_mhz, 59940);
});

test("dragging snaps edges, moves clone members together and rebases the primary at Save", async () => {
  const snapshot = await fixture();
  const [a, b] = snapshot.layout.outputs;
  const rect = {
    key: b.display_key,
    label: "2",
    name: "Second",
    ...b.resolution,
    ...b.position,
  };
  const origin = {
    key: a.display_key,
    label: "1",
    name: "Primary",
    ...a.resolution,
    ...a.position,
  };
  assert.deepEqual(
    arrangement.snapPosition(
      rect,
      [origin],
      { x: a.resolution.width + 5, y: 4 },
      10,
    ),
    { x: a.resolution.width, y: 0 },
  );
  const cloned = editor.changeAttachment(
    snapshot.layout,
    b.display_key,
    a.display_key,
  );
  const moved = editor.editOutput(cloned, a.display_key, {
    position: { x: -100, y: 60 },
  });
  assert.deepEqual(moved.outputs[0].position, moved.outputs[1].position);
  const rebased = editor.rebaseLayout(moved);
  assert.deepEqual(rebased.outputs[0].position, { x: 0, y: 0 });
  assert.deepEqual(rebased.outputs[1].position, { x: 0, y: 0 });
  const overlapping = editor.editOutput(snapshot.layout, b.display_key, {
    position: { x: 0, y: 0 },
  });
  assert.match(
    editor.layoutError(overlapping, snapshot.capabilities),
    /overlap/,
  );
});

test("ultrawide calibration keeps physical edges adjoining with either primary and portrait", async () => {
  for (const primaryOnRight of [false, true]) {
    const snapshot = await fixture();
    snapshot.displays = snapshot.displays.slice(0, 2);
    snapshot.layout.outputs = snapshot.layout.outputs.slice(0, 2);
    const [left, right] = snapshot.layout.outputs;
    left.resolution = { width: 2560, height: 1080 };
    right.resolution = { width: 3440, height: 1440 };
    left.scale_percent = right.scale_percent = 100;
    left.primary = !primaryOnRight;
    right.primary = primaryOnRight;
    left.position = { x: primaryOnRight ? -2560 : 0, y: 0 };
    right.position = { x: primaryOnRight ? 0 : 2560, y: 0 };
    snapshot.capabilities
      .slice(0, 2)
      .forEach((c) => (c.physical_size_mm = { width: 800, height: 340 }));
    const rows = calibration.seedCalibration(snapshot);
    assert.ok(rows.every((r) => r.calibrated));
    assert.equal(rows[1].position_mm.x - rows[0].position_mm.x, 800);
    assert.equal(
      calibration.calibrationConnections(
        calibration.calibrationSurfaces(snapshot, rows),
      ).boundaries,
      1,
    );
    left.rotation = "portrait";
    left.resolution = { width: 1080, height: 2560 };
    if (primaryOnRight) left.position.x = -1080;
    else right.position.x = 1080;
    const portrait = calibration.seedCalibration(snapshot);
    assert.equal(portrait[1].position_mm.x - portrait[0].position_mm.x, 340);
    assert.equal(
      calibration.calibrationConnections(
        calibration.calibrationSurfaces(snapshot, portrait),
      ).boundaries,
      1,
    );
  }
});

test("calibration retains manual placements, reports gaps/overlaps and never borrows unknown dimensions", async () => {
  const snapshot = await fixture();
  const rows = calibration.seedCalibration(snapshot);
  rows[1].position_mm.x += 20;
  assert.equal(
    calibration.calibrationConnections(
      calibration.calibrationSurfaces(snapshot, rows),
    ).boundaries,
    0,
  );
  assert.deepEqual(
    calibration.seedCalibration(snapshot, rows)[1].position_mm,
    rows[1].position_mm,
  );
  const reset = calibration.seedCalibration(snapshot, rows, true);
  assert.equal(
    calibration.calibrationConnections(
      calibration.calibrationSurfaces(snapshot, reset),
    ).boundaries,
    1,
  );
  rows[1].position_mm.x = 20;
  assert.equal(
    calibration.calibrationConnections(
      calibration.calibrationSurfaces(snapshot, rows),
    ).overlapping.size,
    2,
  );
  snapshot.capabilities[1].physical_size_mm = null;
  const unknown = calibration.seedCalibration(snapshot);
  assert.equal(unknown[1].calibrated, false);
  assert.equal(unknown[1].width_mm, 0);
});

test("cursor calibration represents clone groups once and honors an explicit physical representative", async () => {
  const snapshot = await fixture();
  const [first, second] = snapshot.layout.outputs;
  snapshot.layout = editor.changeAttachment(
    snapshot.layout,
    second.display_key,
    first.display_key,
  );
  snapshot.layout.outputs[0].identity = {
    device_path: "port-b",
    edid_serial: "serial-b",
  };
  snapshot.layout.outputs[1].identity = {
    device_path: "port-a",
    edid_serial: "serial-a",
  };
  const rows = calibration.seedCalibration(snapshot);
  let surfaces = calibration.calibrationSurfaces(snapshot, rows);
  assert.equal(surfaces.length, 1);
  assert.equal(surfaces[0].key, second.display_key);
  rows[0].clone_representative = true;
  surfaces = calibration.calibrationSurfaces(snapshot, rows);
  assert.equal(surfaces.length, 1);
  assert.equal(surfaces[0].key, first.display_key);
});
