// Execute only the emitted, source-built maintained Action against a loopback API.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

const action = process.env.UPLOADER_ACTION;
assert.ok(action, 'UPLOADER_ACTION must identify the exact qualified source-built distribution');
const source = JSON.parse(readFileSync(path.join(action, 'provenance.json'), 'utf8'));
assert.equal(source.commit, '29e53e917877a24fad85510ded594ab3c9ca12de');
assert.equal(source.audit.total, 0);

async function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'maintained-uploader-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const assets = new Map(), events = [];
  let partialFailure = false, racingDuplicate = false, missingId = false, alternateId = 55;
  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://localhost');
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const bytes = Buffer.concat(chunks);
      events.push({ method: req.method, path: url.pathname });
      const send = (status, data) => {
        res.writeHead(status, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify(data));
      };
      const current = () => ({ id: alternateId, tag_name: 'v0.4.1', draft: true, prerelease: false,
        upload_url: origin + '/uploads/55{?name,label}' });
      if (req.method === 'GET' && url.pathname === '/repos/fixture/owned/releases/55') {
        send(missingId ? 404 : 200, missingId ? { message: 'missing' } : current());
      } else if (req.method === 'GET' && url.pathname === '/repos/fixture/owned/releases/55/assets') {
        if (racingDuplicate) assets.set('package.bin', Buffer.from('competing bytes'));
        send(200, [...assets.keys()].map((name, index) => ({ id: 90 + index, name, browser_download_url: origin + '/asset' })));
      } else if (req.method === 'POST' && url.pathname === '/uploads/55') {
        const name = url.searchParams.get('name');
        if (assets.has(name)) send(422, { message: 'already exists' });
        else if (partialFailure && name === 'package.bin') send(400, { message: 'owned interrupted upload' });
        else { assets.set(name, bytes); send(201, { id: assets.size + 90, name, browser_download_url: origin + '/asset' }); }
      } else if (req.method === 'POST' && url.pathname === '/repos/fixture/owned/releases') {
        // Known upstream fallback on an absent ID: never claim this cannot create.
        assert.equal(JSON.parse(bytes).draft, true);
        assert.equal(JSON.parse(bytes).target_commitish, 'a'.repeat(40));
        alternateId = 56; missingId = false; send(201, current());
      } else if (req.method === 'GET' && url.pathname === '/repos/fixture/owned/releases/56') {
        send(200, current());
      } else if (req.method === 'GET' && url.pathname.startsWith('/repos/fixture/owned/git/ref/')) {
        send(404, { message: 'Owned missing-ref response; source must retain exact target_commitish' });
      } else if (req.method === 'GET' && url.pathname === '/repos/fixture/owned/releases/56/assets') {
        send(200, []);
      } else { send(400, { message: 'Unexpected route: ' + req.method + ' ' + url.pathname }); }
    } catch (error) { res.writeHead(500).end(JSON.stringify({ message: error.message })); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
  const origin = 'http://127.0.0.1:' + server.address().port;
  async function run(files) {
    const output = path.join(directory, 'output-' + events.length);
    writeFileSync(output, '');
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
      ['PATH', 'SystemRoot', 'SYSTEMROOT', 'TMP', 'TEMP'].includes(key)));
    Object.assign(env, { GITHUB_REPOSITORY: 'fixture/owned', GITHUB_API_URL: origin,
      GITHUB_OUTPUT: output, INPUT_REPO_TOKEN: 'owned-fixture-not-a-credential',
      INPUT_RELEASE_ID: '55', INPUT_TAG: 'v0.4.1', INPUT_TARGET_COMMIT: 'a'.repeat(40),
      INPUT_FILE: files, INPUT_FILE_GLOB: 'true', INPUT_OVERWRITE: 'false',
      INPUT_CHECK_DUPLICATES: 'true', INPUT_DRAFT: 'true', INPUT_PROMOTE: 'false',
      INPUT_PRERELEASE: 'false', INPUT_MAKE_LATEST: 'false' });
    const result = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [path.join(action, 'dist/index.js')], { env, windowsHide: true });
      let output = '';
      const timer = setTimeout(() => { child.kill(); reject(new Error('Owned Action deadline')); }, 30000);
      child.stdout.on('data', b => output += b);
      child.stderr.on('data', b => output += b);
      child.on('error', reject);
      child.on('close', code => { clearTimeout(timer); resolve({ code, output }); });
    });
    return { ...result, outputs: readFileSync(output, 'utf8') };
  }
  function file(name, bytes = 'owned ' + name) {
    const filename = path.join(directory, name);
    writeFileSync(filename, bytes);
    return filename.replaceAll('\\', '/');
  }
  return { directory, assets, events, run, file,
    partial: value => { partialFailure = value; },
    race: () => { racingDuplicate = true; },
    missing: () => { missingId = true; } };
}

test('exact-ID draft upload and missing-only interrupted retry retain bytes without DELETE', async t => {
  const f = await fixture(t);
  const manifest = f.file('manifest.json'), packageFile = f.file('package.bin');
  f.partial(true);
  let result = await f.run(manifest);
  assert.equal(result.code, 0, result.output);
  const retained = Buffer.from(f.assets.get('manifest.json'));
  result = await f.run(packageFile);
  assert.equal(result.code, 1, result.output);
  assert.equal(f.assets.size, 1);
  f.partial(false);
  result = await f.run(packageFile);
  assert.equal(result.code, 0, result.output);
  assert.deepEqual(f.assets.get('manifest.json'), retained);
  assert.deepEqual(f.assets.get('package.bin'), Buffer.from('owned package.bin'));
  assert.ok(!f.events.some(e => ['DELETE', 'PATCH'].includes(e.method)));
  assert.ok(!f.events.some(e => e.path.endsWith('/releases')), 'Existing ID must avoid list/create');
});

test('same-name duplicate/race refuses without replacement or publication', async t => {
  const f = await fixture(t);
  f.race();
  const result = await f.run(f.file('package.bin'));
  assert.equal(result.code, 1, result.output);
  assert.deepEqual(f.assets.get('package.bin'), Buffer.from('competing bytes'));
  assert.ok(!f.events.some(e => ['DELETE', 'POST', 'PATCH'].includes(e.method)));
});

test('unmatched glob fails and leaves the draft unchanged', async t => {
  const f = await fixture(t);
  const result = await f.run(path.join(f.directory, 'absent-*').replaceAll('\\', '/'));
  assert.equal(result.code, 1, result.output);
  assert.equal(f.assets.size, 0);
  assert.ok(!f.events.some(e => ['DELETE', 'POST', 'PATCH'].includes(e.method)));
});

test('absent ID upstream fallback is visible and must fail the independent same-ID proof', async t => {
  const f = await fixture(t);
  f.missing();
  const result = await f.run(f.file('package.bin'));
  assert.equal(result.code, 0, result.output);
  assert.match(result.outputs, /release_id<<[^\r\n]+\r?\n56\r?\n/);
  assert.ok(f.events.some(e => e.method === 'POST' && e.path.endsWith('/releases')));
  assert.ok(!f.events.some(e => ['DELETE', 'PATCH'].includes(e.method)));
  // Final workflow requires the original exact ID 55, so this 56 result cannot finalize.
});
