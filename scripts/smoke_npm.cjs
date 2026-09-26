// Smoke-test the launcher against the native binary on each release runner.
'use strict';
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { targetFor } = require('../npm/bin/muse-acp.cjs');

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'muse-acp-npm-smoke-'));
try {
  const target = targetFor(process.platform, process.arch, true);
  const binary = process.platform === 'win32' ? 'muse-acp.exe' : 'muse-acp';
  const native = path.join(root, 'native', target);
  fs.mkdirSync(native, { recursive: true });
  fs.mkdirSync(path.join(root, 'bin'));
  fs.copyFileSync(path.join(__dirname, '../npm/bin/muse-acp.cjs'), path.join(root, 'bin/muse-acp.cjs'));
  fs.copyFileSync(path.resolve(process.argv[2] || path.join('target', target, 'release', binary)), path.join(native, binary));
  const result = spawnSync(process.execPath, [path.join(root, 'bin/muse-acp.cjs'), '--selftest'], { encoding: 'utf8', timeout: 30000 });
  assert.equal(result.status, 0, result.stderr || String(result.error));
  assert.match(result.stdout, /selftest: static literals OK/);
  process.stdout.write(result.stdout);
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
