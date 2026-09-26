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

  // Verify new-session startup through the shipped launcher with the profile
  // that older adapters passed unchanged to a host without an auto reviewer.
  const config = path.join(root, 'config');
  fs.mkdirSync(path.join(config, 'muse'), { recursive: true });
  const settingsPath = path.join(config, 'muse/settings.json');
  const settings = JSON.stringify({ schema_version: 1, permissions: { schema_version: 1, default_profile: ':auto-review' } });
  fs.writeFileSync(settingsPath, settings);
  const log = path.join(root, 'host.log');
  const startup = spawnSync(process.execPath, [path.join(root, 'bin/muse-acp.cjs')], {
    encoding: 'utf8', timeout: 30000,
    env: {
      ...process.env,
      XDG_CONFIG_HOME: config,
      MUSE_CLI: path.join(__dirname, '../tests/fixtures', process.platform === 'win32' ? 'fake_serve.cmd' : 'fake_serve.py'),
      MUSE_SERVE_ARGS: '',
      MUSE_APPROVAL_MODE: '',
      FAKE_SCENARIO: 'quiet',
      FAKE_CHECK_HOST_CONFIG: '1',
      FAKE_LOG: log,
      FAKE_FRAMES: log + '.frames',
    },
    input: [
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: 1 } },
      { jsonrpc: '2.0', id: 2, method: 'session/new', params: { cwd: root } },
    ].map(frame => JSON.stringify(frame) + '\n').join(''),
  });
  assert.equal(startup.status, 0, startup.stderr || String(startup.error));
  const response = startup.stdout.trim().split('\n').map(line => JSON.parse(line)).find(frame => frame.id === 2);
  assert.ok(response?.result?.sessionId, JSON.stringify(response));
  const observed = JSON.parse(fs.readFileSync(log + '.config', 'utf8'));
  assert.equal(observed.settings.permissions.default_profile, ':ask-me');
  assert.equal(fs.readFileSync(settingsPath, 'utf8'), settings);
  assert.equal(fs.existsSync(observed.root), false, 'host settings must be cleaned up');
  process.stdout.write('npm launcher: new session with saved :auto-review profile OK\n');
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
