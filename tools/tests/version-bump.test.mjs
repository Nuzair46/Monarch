import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import test from 'node:test';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('release bump keeps manifests and both lockfiles synchronized without changing dependencies', () => {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'monarch-version-test-'));
  try {
    const files = ['Cargo.toml', 'Cargo.lock', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'package.json', 'src-tauri/tauri.conf.json'];
    for (const file of files) {
      fs.mkdirSync(path.dirname(path.join(fixture, file)), { recursive: true });
      fs.copyFileSync(path.join(root, file), path.join(fixture, file));
    }
    const original = files.filter((file) => file.endsWith('.lock')).map((file) => fs.readFileSync(path.join(fixture, file), 'utf8'));
    const run = (argument) => execFileSync(process.execPath, [path.join(root, 'tools/version-bump.mjs'), argument], { cwd: fixture, encoding: 'utf8', stdio: 'pipe' });
    const current = JSON.parse(fs.readFileSync(path.join(fixture, 'package.json'), 'utf8')).version;
    const [, major, minor] = current.match(/^(\d+)\.(\d+)\./);
    const next = `${major}.${Number(minor) + 1}.0`;
    run('minor');
    assert.ok(run('--check').includes(`Versions are synced: ${next}`));
    const dependencies = (lock) => lock.split('[[package]]').filter((entry) => !/^name = "monarch(?:-desktop)?"$/m.test(entry)).join('[[package]]');
    ['Cargo.lock', 'src-tauri/Cargo.lock'].forEach((file, i) => {
      assert.equal(dependencies(fs.readFileSync(path.join(fixture, file), 'utf8')), dependencies(original[i]));
    });
    fs.cpSync(path.join(root, 'src'), path.join(fixture, 'src'), { recursive: true });
    // A stale root package lock used to make this fail immediately after a release bump.
    execFileSync('cargo', ['check', '--locked', '--offline'], {
      cwd: fixture, encoding: 'utf8', stdio: 'pipe',
      env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target'), ASDF_RUST_VERSION: process.env.ASDF_RUST_VERSION ?? '1.93.1' },
    });
    const before = files.map((file) => fs.readFileSync(path.join(fixture, file), 'utf8'));
    assert.throws(() => run('not-a-version'));
    assert.deepEqual(files.map((file) => fs.readFileSync(path.join(fixture, file), 'utf8')), before);
  } finally { fs.rmSync(fixture, { recursive: true, force: true }); }
});
