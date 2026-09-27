import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import ts from "typescript";

// Exercise the actual standalone TypeScript modules without a browser or new test dependency.
function compiledModule(file, imports = {}) {
  if (file === "web/app/display-editor.ts")
    imports = {
      "./arrangement": compiledModule("web/app/arrangement.ts"),
      ...imports,
    };
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

test("profile audio is saved independently, applied with displays, and reverted on timeout", async () => {
  const mock = await import(
    compiledModule("web/mock.ts", {
      "@/app/ui": compiledModule("web/app/ui.ts"),
      "@/app/display-editor": compiledModule("web/app/display-editor.ts"),
    }) + "#audio"
  );
  const before = await mock.getSnapshot();
  await mock.setProfileAudio("Focus", "headphones");
  const saved = await mock.getSnapshot();
  assert.deepEqual(saved.audio, before.audio);
  assert.deepEqual(saved.layout, before.layout);
  assert.deepEqual(
    saved.profiles.find((p) => p.name === "Focus").layout,
    before.profiles.find((p) => p.name === "Focus").layout,
  );
  await mock.applyProfile("Focus");
  assert.equal((await mock.getSnapshot()).audio.defaults.console, "headphones");
  assert.equal(
    (await mock.getSnapshot()).audio.defaults.communications,
    before.audio.defaults.communications,
  );
  await assert.rejects(mock.setProfileAudio("Focus", null));
  await mock.rollbackPending();
  assert.deepEqual(
    (await mock.getSnapshot()).audio.defaults,
    before.audio.defaults,
  );
  assert.deepEqual((await mock.getSnapshot()).layout, before.layout);
  await mock.updateSettings({ ...before.settings, revert_timeout_secs: 1 });
  await mock.applyProfile("Focus");
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.deepEqual(
    (await mock.getSnapshot()).audio.defaults,
    before.audio.defaults,
  );
  await mock.applyProfile("Focus");
  await mock.confirmCurrentLayout();
  await mock.setProfileAudio("Focus", null);
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
  await mock.applyProfile("Focus");
  assert.equal((await mock.getSnapshot()).audio.defaults.console, "headphones");
  await mock.confirmCurrentLayout();
});

test("unavailable HDMI output may be saved and resolves after enabling its display", async () => {
  const mock = await import(
    compiledModule("web/mock.ts", {
      "@/app/ui": compiledModule("web/app/ui.ts"),
      "@/app/display-editor": compiledModule("web/app/display-editor.ts"),
    }) + "#hdmi-audio"
  );
  const before = await mock.getSnapshot();
  await mock.setProfileAudio("Desk", "display-hdmi");
  assert.equal(
    (await mock.getSnapshot()).profiles.find((p) => p.name === "Desk")
      .audio_output.id,
    "display-hdmi",
  );
  await assert.rejects(mock.applyProfile("Desk"), /unavailable/);
  assert.deepEqual((await mock.getSnapshot()).layout, before.layout);
  assert.deepEqual(
    (await mock.getSnapshot()).audio.defaults,
    before.audio.defaults,
  );
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
  await assert.rejects(mock.setProfileAudio("Desk", "Speakers"));
  await mock.toggleDisplay(before.displays[2].id_key);
  await mock.confirmCurrentLayout();
  await mock.saveProfile("HDMI", "display-hdmi");
  assert.equal((await mock.getSnapshot()).audio.defaults.console, "speakers");
  await mock.restoreLastLayout();
  assert.equal(
    (await mock.getSnapshot()).audio.devices.find(
      (d) => d.id === "display-hdmi",
    ).available,
    false,
  );
  await mock.applyProfile("HDMI");
  assert.equal(
    (await mock.getSnapshot()).audio.defaults.console,
    "display-hdmi",
  );
  await mock.confirmCurrentLayout();
  await mock.restoreLastLayout();
  assert.deepEqual(
    (await mock.getSnapshot()).audio.defaults,
    before.audio.defaults,
  );
  assert.deepEqual((await mock.getSnapshot()).layout, before.layout);
});

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
    initial.capabilities,
  );
  assert.equal(draft.outputs[1].clone_group, draft.outputs[0].clone_group);
  assert.equal(draft.outputs[1].primary, true);
  await mock.applyLayout(draft);
  await mock.rollbackPending();
  const extended = changeAttachment(
    draft,
    initial.displays[1].id_key,
    "extend",
    initial.capabilities,
  );
  assert.equal(extended.outputs[0].clone_group, null);
  assert.ok(
    extended.outputs[1].position.x >= extended.outputs[0].resolution.width,
  );
  const detached = changeAttachment(
    draft,
    initial.displays[0].id_key,
    "detached",
    initial.capabilities,
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

function duplicate(snapshot, member = 1, target = 0) {
  return editor.changeAttachment(
    snapshot.layout,
    snapshot.layout.outputs[member].display_key,
    snapshot.layout.outputs[target].display_key,
    snapshot.capabilities,
  );
}

test("duplication mirrors one desktop while preserving each monitor's supported refresh, HDR and rotation", async () => {
  const snapshot = await fixture();
  snapshot.layout.outputs[0].hdr_enabled = true;
  snapshot.layout.outputs[1].hdr_enabled = false;
  snapshot.layout.outputs[1].rotation = "landscape_flipped";
  const before = structuredClone(snapshot);
  const layout = duplicate(snapshot);
  const [primary, second] = layout.outputs;
  assert.ok(editor.sameSource(primary, second));
  assert.deepEqual(second.position, primary.position);
  assert.deepEqual(second.resolution, primary.resolution);
  assert.equal(primary.primary, true);
  assert.equal(second.primary, true);
  assert.equal(primary.refresh_rate_mhz, 144000);
  assert.equal(second.refresh_rate_mhz, 60000);
  assert.equal(primary.hdr_enabled, true);
  assert.equal(second.hdr_enabled, false);
  assert.equal(second.rotation, "landscape_flipped");
  assert.equal(editor.layoutError(layout, snapshot.capabilities), null);
  assert.deepEqual(snapshot, before);
  const changed = editor.editOutput(layout, second.display_key, {
    refresh_rate_mhz: 59940,
  });
  assert.equal(changed.outputs[0].refresh_rate_mhz, 144000);
});

test("duplication chooses a common resolution and scale instead of copying unsupported primary settings", async () => {
  const snapshot = await fixture();
  const [a, b] = snapshot.layout.outputs;
  a.scale_percent = 200;
  b.scale_percent = 100;
  b.refresh_rate_mhz = 59940;
  snapshot.capabilities[0].scale_percent = 200;
  snapshot.capabilities[1].scale_percentages = [100];
  snapshot.capabilities[1].modes = snapshot.capabilities[1].modes.filter(
    (m) => m.resolution.width === 1920,
  );
  const layout = duplicate(snapshot);
  for (const output of layout.outputs.filter((o) => o.enabled)) {
    assert.deepEqual(output.resolution, { width: 1920, height: 1080 });
    assert.equal(output.scale_percent, 100);
  }
  assert.equal(layout.outputs[0].refresh_rate_mhz, 60000);
  assert.equal(layout.outputs[1].refresh_rate_mhz, 59940);
  assert.deepEqual(
    editor.sharedResolutionChoices(
      layout,
      layout.outputs[1],
      snapshot.capabilities,
    ),
    [{ width: 1920, height: 1080 }],
  );
  assert.deepEqual(
    editor.sharedScaleChoices(layout, layout.outputs[1], snapshot.capabilities),
    [100],
  );
  assert.equal(
    editor.canPreserveScaling(layout, layout.outputs[1], snapshot.capabilities),
    false,
  );
  assert.equal(editor.layoutError(layout, snapshot.capabilities), null);
});

test("joining an existing clone group checks every member and leaves other displays extended", async () => {
  const snapshot = await fixture();
  snapshot.layout = duplicate(snapshot);
  const third = snapshot.layout.outputs[2];
  third.rotation = "landscape";
  third.resolution = { width: 1920, height: 1080 };
  snapshot.capabilities[2].scale_percent = 100;
  snapshot.capabilities[2].scale_percentages = [100];
  const extra = {
    ...structuredClone(third),
    enabled: true,
    display_key: "fourth",
    position: { x: 2560, y: 0 },
  };
  snapshot.layout.outputs.push(extra);
  snapshot.capabilities.push({
    ...structuredClone(snapshot.capabilities[2]),
    display_key: extra.display_key,
  });
  const layout = editor.fitDesktop(duplicate(snapshot, 2));
  const group = layout.outputs.slice(0, 3);
  assert.ok(group.every((o) => editor.sameSource(o, group[0])));
  assert.ok(group.every((o) => o.resolution.width === 1920));
  assert.equal(layout.outputs[3].clone_group, null);
  assert.equal(layout.outputs[3].enabled, true);
  assert.equal(editor.layoutError(layout, snapshot.capabilities), null);
});

test("clone capability intersections respect each target's rotation", async () => {
  for (const rotation of ["portrait", "portrait_flipped"]) {
    const snapshot = await fixture();
    snapshot.layout.outputs[1].rotation = rotation;
    snapshot.capabilities[1].modes = [
      { resolution: { width: 1080, height: 1920 }, refresh_rate_mhz: 59940 },
    ];
    const layout = duplicate(snapshot);
    assert.deepEqual(layout.outputs[1].resolution, {
      width: 1920,
      height: 1080,
    });
    assert.equal(layout.outputs[1].rotation, rotation);
    assert.equal(layout.outputs[1].refresh_rate_mhz, 59940);
    assert.deepEqual(
      editor.sharedResolutionChoices(
        layout,
        layout.outputs[1],
        snapshot.capabilities,
      ),
      [{ width: 1080, height: 1920 }],
    );
    assert.equal(editor.layoutError(layout, snapshot.capabilities), null);
  }
});

test("missing, conflicting or incompatible clone capabilities never mutate the layout", async () => {
  for (const failure of ["missing", "replacement", "resolution", "scaling"]) {
    const snapshot = await fixture();
    if (failure === "missing") snapshot.capabilities.splice(1, 1);
    if (failure === "replacement") {
      snapshot.layout.outputs[1].identity = { edid_serial: "original" };
      snapshot.capabilities[1].identity = { edid_serial: "replacement" };
    }
    if (failure === "resolution")
      snapshot.capabilities[1].modes = [
        { resolution: { width: 1024, height: 768 }, refresh_rate_mhz: 60000 },
      ];
    if (failure === "scaling") {
      snapshot.capabilities[0].scale_percent = 200;
      snapshot.capabilities[0].scale_percentages = [200];
      snapshot.capabilities[1].scale_percentages = [100];
    }
    const before = structuredClone(snapshot);
    assert.throws(
      () => duplicate(snapshot),
      /shared resolution|shared scaling/,
    );
    assert.deepEqual(snapshot, before);
  }
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
    snapshot.capabilities,
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

test("display settings apply independently of position drafts and resizing keeps neighbours joined", async () => {
  const snapshot = await fixture();
  const key = snapshot.layout.outputs[1].display_key;
  const originalY = snapshot.layout.outputs[1].position.y;
  const offsets = { [key]: { x: 0, y: 120 } };
  const preview = editor.arrangementDraft(snapshot.layout, offsets);
  assert.equal(preview.outputs[1].position.y, originalY + 120);
  const settings = editor.editOutput(
    snapshot.layout,
    snapshot.layout.outputs[0].display_key,
    { hdr_enabled: true },
  );
  assert.equal(settings.outputs[1].position.y, originalY);
  const after = editor.arrangementDraft(settings, offsets);
  assert.equal(after.outputs[0].hdr_enabled, true);
  assert.equal(after.outputs[1].position.y, originalY + 120);
  assert.deepEqual(editor.arrangementDraft(settings, {}), settings);
  for (const width of [1920, 3440]) {
    const resized = editor.editOutput(
      snapshot.layout,
      snapshot.layout.outputs[0].display_key,
      { resolution: { width, height: 1440 } },
    );
    assert.ok(editor.desktopGeometryError(resized));
    const fitted = editor.fitDesktop(resized);
    assert.equal(editor.desktopGeometryError(fitted), null);
    assert.equal(fitted.outputs[1].position.x, width);
    assert.equal(
      fitted.outputs[0].refresh_rate_mhz,
      snapshot.layout.outputs[0].refresh_rate_mhz,
    );
  }
});

test("dropping a monitor closes gaps and avoids overlap without changing its size", () => {
  const fixed = { x: 0, y: 0, width: 3440, height: 1440 };
  const moving = { x: 440, y: -1080, width: 2560, height: 1080 };
  for (const y of [-1100, -1060]) {
    const position = arrangement.connectedPosition(moving, [fixed], {
      x: 440,
      y,
    });
    assert.deepEqual(position, { x: 440, y: -1080 });
    assert.equal(
      arrangement.rectanglesOverlap({ ...moving, ...position }, fixed),
      false,
    );
    assert.equal(
      arrangement.edgesTouch({ ...moving, ...position }, fixed),
      true,
    );
  }
});
