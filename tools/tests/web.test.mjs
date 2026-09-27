import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import ts from 'typescript';

// Exercise the actual standalone TypeScript modules without a browser or new test dependency.
function compiledModule(file, imports = {}) {
  let source = fs.readFileSync(new URL(`../../${file}`, import.meta.url), 'utf8');
  for (const [specifier, url] of Object.entries(imports)) {
    source = source.replaceAll(`'${specifier}'`, JSON.stringify(url)).replaceAll(`"${specifier}"`, JSON.stringify(url));
  }
  const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } });
  return `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`;
}

const { subscriptions } = await import(compiledModule('web/app/subscriptions.ts'));

test('subscriptions dispose registrations that finish after unmount', async () => {
  let resolve;
  let calls = 0;
  const listeners = subscriptions((error) => { throw error; });
  listeners.add(new Promise((done) => { resolve = done; }));
  listeners.dispose();
  resolve(() => calls++);
  await Promise.resolve();
  assert.equal(calls, 1);
  listeners.dispose();
  assert.equal(calls, 1);
});

test('subscriptions dispose existing listeners and surface only live failures', async () => {
  let calls = 0;
  const errors = [];
  const listeners = subscriptions((error) => errors.push(error));
  listeners.add(Promise.resolve(() => calls++));
  listeners.add(Promise.reject('registration failed'));
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(errors, ['registration failed']);
  listeners.dispose();
  listeners.add(Promise.reject('unmounted'));
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(calls, 1);
  assert.equal(errors.length, 1);
});

test('browser mock obeys confirmation, rollback, timeout and custom-shortcut contracts', async () => {
  const mock = await import(compiledModule('web/mock.ts', { '@/app/ui': compiledModule('web/app/ui.ts'), '@/app/display-editor': compiledModule('web/app/display-editor.ts') }));
  const initial = await mock.getSnapshot();
  await mock.toggleDisplay(initial.displays[1].id_key);
  assert.ok((await mock.getSnapshot()).pending_confirmation);
  await assert.rejects(mock.saveProfile('unconfirmed'));
  await assert.rejects(mock.applyProfile('Desk'));
  await mock.rollbackPending();
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  await mock.applyProfile('Focus');
  await mock.confirmCurrentLayout();
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
  await assert.rejects(mock.toggleDisplay(initial.displays[0].id_key));
  await mock.restoreLastLayout();
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  await mock.updateSettings({ ...initial.settings, revert_timeout_secs: 1,
    profile_shortcut_base: null, profile_shortcuts: { Desk: 'Ctrl+Alt+W' } });
  assert.equal((await mock.getSnapshot()).settings.profile_shortcut_base, null);
  await mock.toggleDisplay(initial.displays[1].id_key);
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.deepEqual((await mock.getSnapshot()).layout, initial.layout);
  assert.equal((await mock.getSnapshot()).pending_confirmation, null);
});

test('editor drafts save without mutation, retain unavailable monitors, and apply/revert preferences', async () => {
  const mock = await import(compiledModule('web/mock.ts', { '@/app/ui': compiledModule('web/app/ui.ts'), '@/app/display-editor': compiledModule('web/app/display-editor.ts') }) + '#drafts');
  const {editOutput,changeAttachment} = await import(compiledModule('web/app/display-editor.ts'));
  const initial = await mock.getSnapshot();
  let draft = editOutput(initial.layout,initial.displays[0].id_key,{hdr_enabled:true,scale_percent:150,refresh_rate_mhz:60000});
  await mock.saveProfileLayout('Draft',draft);
  assert.deepEqual((await mock.getSnapshot()).layout,initial.layout);
  assert.equal((await mock.getSnapshot()).pending_confirmation,null);
  await mock.applyProfile('Draft');
  assert.equal((await mock.getSnapshot()).layout.outputs[0].hdr_enabled,true);
  await mock.rollbackPending();
  assert.deepEqual((await mock.getSnapshot()).layout,initial.layout);
  draft = changeAttachment(draft,initial.displays[1].id_key,initial.displays[0].id_key);
  assert.equal(draft.outputs[1].clone_group,draft.outputs[0].clone_group);
  assert.equal(draft.outputs[1].primary,true);
  await mock.applyLayout(draft); await mock.rollbackPending();
  const extended=changeAttachment(draft,initial.displays[1].id_key,'extend');
  assert.equal(extended.outputs[0].clone_group,null);
  assert.ok(extended.outputs[1].position.x>=extended.outputs[0].resolution.width);
  const detached=changeAttachment(draft,initial.displays[0].id_key,'detached');
  assert.equal(detached.outputs[1].clone_group,null);assert.equal(detached.outputs[1].enabled,true);
  const missing=structuredClone(initial.layout);missing.outputs[1].display_key='missing';
  await mock.saveProfileLayout('Disconnected',missing);
  await assert.rejects(mock.applyProfile('Disconnected'),/unavailable/);
  assert.deepEqual((await mock.getSnapshot()).layout,initial.layout);
  const caps=await mock.getDisplayCapabilities();
  assert.ok(caps[0].modes.some((m)=>m.refresh_rate_mhz===59940));
  assert.equal(caps[1].hdr_supported,false);
  assert.ok(caps[2].scaling_unavailable_reason);
  await assert.rejects(mock.applyLayout(editOutput(initial.layout,initial.displays[1].id_key,{hdr_enabled:true})),/HDR/);
});
