// Real pinned Actions against disposable Git/loopback; no provider/repository writes.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, writeFileSync, readFileSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
const root = fileURLToPath(new URL('../..', import.meta.url));
const workflow = name => parse(readFileSync(path.join(root, '.github/workflows', name + '.yml'), 'utf8'));
const pins = { PATHS_ACTION: 'ceb8a2b8f2d89434be7ff52d3de7ec3738c5cc9d',
  GRAPHQL_ACTION: 'ddde8ebb2493e79f390e6449c725c21663a67505',
  REQUEST_ACTION: 'b91aabaa861c777dcdb14e2387e30eddf04619ae' };
function owned(t) {
  const folder = mkdtempSync(path.join(tmpdir(), 'release-actions-'));
  t.after(() => rmSync(folder, { recursive: true, force: true }));
  return folder;
}
function outputs(text) {
  const result = {}, lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const marker = lines[i].indexOf('<<');
    if (marker >= 0) {
      const name = lines[i].slice(0, marker), delimiter = lines[i].slice(marker + 2), value = [];
      while (++i < lines.length && lines[i] !== delimiter) value.push(lines[i]);
      result[name] = value.join('\n');
    } else if (lines[i].includes('=')) {
      const split = lines[i].indexOf('=');
      result[lines[i].slice(0, split)] = lines[i].slice(split + 1);
    }
  }
  return result;
}
async function action(t, key, endpoint, inputs, payload = { pull_request: { number: 7 } }, cwd = root, eventName = 'pull_request') {
  const source = process.env[key];
  assert.ok(source, key + ' must identify the exact upstream clone');
  assert.equal(execFileSync('git', ['-C', source, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), pins[key]);
  const folder = owned(t), output = path.join(folder, 'output'), event = path.join(folder, 'event.json');
  writeFileSync(output, ''); writeFileSync(event, JSON.stringify(payload));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    ['PATH', 'SystemRoot', 'SYSTEMROOT', 'TMP', 'TEMP'].includes(key)));
  Object.assign(env, { PATH: process.env.PATH, PATHEXT: process.env.PATHEXT ?? '', GITHUB_ACTION: 'owned-fixture', GITHUB_REPOSITORY: 'fixture/owned', GITHUB_TOKEN: 'owned-fixture-not-a-credential',
    GITHUB_API_URL: endpoint, GITHUB_WORKSPACE: cwd, GITHUB_OUTPUT: output, GITHUB_EVENT_PATH: event,
    GITHUB_EVENT_NAME: eventName, INPUT_MEDIATYPE: '{}', INPUT_LIST_FILES: 'none', INPUT_INITIAL_FETCH_DEPTH: '100' });
  for (const [name, value] of Object.entries(inputs)) env['INPUT_' + name.toUpperCase()] = String(value);
  const result = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(source, 'dist/index.js')], { cwd, env, windowsHide: true });
    let text = '', timedOut = false;
    const timer = setTimeout(() => { timedOut = true; child.kill(); }, 30000);
    child.stdout.on('data', data => text += data); child.stderr.on('data', data => text += data);
    child.on('error', reject);
    child.on('close', code => { clearTimeout(timer); timedOut ? reject(new Error('Owned deadline')) : resolve({ code, text }); });
  });
  return { ...result, outputs: outputs(readFileSync(output, 'utf8')) };
}
async function server(t, handler) {
  const http = createServer(async (req, res) => {
    try {
      const chunks = []; for await (const chunk of req) chunks.push(chunk);
      const response = await handler(req, Buffer.concat(chunks));
      res.writeHead(response.status ?? 200, { 'Content-Type': 'application/json', ...response.headers });
      res.end(JSON.stringify(response.body));
    } catch (error) { res.writeHead(500).end(JSON.stringify({ message: error.message })); }
  });
  await new Promise(resolve => http.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { http.closeAllConnections(); http.close(resolve); }));
  return 'http://127.0.0.1:' + http.address().port;
}

test('ordered workflow gates and fixed artifact handoff', () => {
  const ci = workflow('ci'), pipeline = workflow('pipeline');
  assert.deepEqual(ci.on.push.branches, ['main']);
  assert.equal(ci.on.pull_request_target, undefined);
  assert.equal(ci.jobs.policy.name, 'Changed paths and PR checks');
  assert.equal(ci.jobs.policy.steps.find(s => s.id === 'paths').with['predicate-quantifier'], 'every');
  assert.deepEqual(pipeline.jobs.build.needs, 'candidate');
  assert.deepEqual(pipeline.jobs.candidate.needs, ['checks', 'uploader']);
  assert.deepEqual(pipeline.jobs.integration.needs, ['candidate', 'build']);
  assert.deepEqual(pipeline.jobs.release.needs, ['candidate', 'build', 'integration', 'uploader']);
  assert.equal(pipeline.jobs.release.if, 'inputs.release');
  assert.equal(pipeline.concurrency.queue, 'max');
  assert.equal(pipeline.concurrency['cancel-in-progress'], false);
  for (const [name, job] of Object.entries(pipeline.jobs)) {
    if (name !== 'release') assert.notEqual(job.permissions?.contents, 'write');
  }
  const release = pipeline.jobs.release.steps;
  const tag = release.findIndex(s => s.name === 'Official tag only after every preceding job passed');
  assert.ok(tag > release.findIndex(s => s.name === 'Verify the exact integration-tested upload inventory'));
  assert.ok(!release.some(s => /cross build|npm run build|coordinator\.py/.test(s.run ?? '')));
  const upload = release.find(s => s.uses === './.qualified/uploader');
  assert.equal(upload.with.overwrite, false); assert.equal(upload.with.draft, true);
  assert.equal(upload.with.promote, false); assert.equal(upload.with.check_duplicates, true);
  assert.equal(upload.with.release_id, '${{ steps.retained.outputs.release_id }}');
});

test('actual paths Action: docs, mixed, unknown, executable, later page and renamed source', async t => {
  let files = [{ filename: 'docs/guide.md', status: 'modified' }], paged = false;
  const endpoint = await server(t, req => {
    const url = new URL(req.url, 'http://localhost');
    assert.equal(url.pathname, '/repos/fixture/owned/pulls/7/files');
    if (paged && url.searchParams.get('page') !== '2') return { body: [{ filename: 'README.md', status: 'modified' }],
      headers: { Link: '<' + endpoint + '/repos/fixture/owned/pulls/7/files?page=2>; rel="next"' } };
    return { body: files };
  });
  const filters = readFileSync(path.join(root, '.github/application-paths.yml'), 'utf8');
  const run = () => action(t, 'PATHS_ACTION', endpoint, { token: 'fixture-only', filters, 'predicate-quantifier': 'every' });
  let result = await run(); assert.equal(result.code, 0, result.text); assert.equal(result.outputs.application, 'false');
  files = [{ filename: 'release/README.md', status: 'modified' }];
  result = await run(); assert.equal(result.code, 0, result.text); assert.equal(result.outputs.application, 'false');
  for (const filename of ['src/main.rs', 'unknown/file', 'docs/fixture.json', 'docs/images/helper.py', 'docs/licenses/tool.sh']) {
    files = [{ filename: 'README.md', status: 'modified' }, { filename, status: 'modified' }];
    result = await run(); assert.equal(result.code, 0, result.text); assert.equal(result.outputs.application, 'true', filename);
  }
  files = [{ filename: 'unknown', status: 'modified' }]; paged = true;
  result = await run(); assert.equal(result.code, 0, result.text); assert.equal(result.outputs.application, 'true');
  paged = false; files = [{ filename: 'docs/moved.md', previous_filename: 'src/old.rs', status: 'renamed' }];
  result = await run(); assert.equal(result.code, 0, result.text); assert.equal(result.outputs.application, 'true');
});

test('actual paths Action push base/ref sees one merged source commit', async t => {
  const repo = owned(t), git = (...args) => execFileSync('git', args, { cwd: repo, encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Owned Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(path.join(repo, 'README.md'), 'base'); git('add', '.'); git('commit', '-m', 'docs(REM-56): base');
  const before = git('rev-parse', 'HEAD');
  mkdirSync(path.join(repo, 'src')); writeFileSync(path.join(repo, 'src/app.rs'), 'source');
  git('add', '.'); git('commit', '-m', 'fix(REM-56): merged source'); const sha = git('rev-parse', 'HEAD');
  const result = await action(t, 'PATHS_ACTION', 'http://127.0.0.1:1', { token: '', base: before, ref: sha,
    filters: readFileSync(path.join(root, '.github/application-paths.yml'), 'utf8'), 'predicate-quantifier': 'every' },
    { before, after: sha, repository: { default_branch: 'main' } }, repo, 'push');
  assert.equal(result.code, 0, result.text); assert.equal(result.outputs.application, 'true');
});

test('actual exact GraphQL lookup and release-ID request Actions avoid draft pagination', async t => {
  let draft = true, failedLookup = false; const events = [];
  const endpoint = await server(t, (req, bytes) => {
    const url = new URL(req.url, 'http://localhost'); events.push(req.method + ' ' + url.pathname);
    if (url.pathname === '/graphql') {
      const data = JSON.parse(bytes); assert.equal(data.variables.tag, 'v0.4.1');
      assert.match(data.query, /release\(tagName: \$tag\)/);
      return { body: failedLookup ? { errors: [{ message: 'owned lookup failure' }] } : { data: { repository: {
        release: { databaseId: 55, tagName: 'v0.4.1', isDraft: draft, releaseAssets: { totalCount: 0 } } } } } };
    }
    if (req.method === 'GET' && url.pathname === '/repos/fixture/owned/releases/55')
      return { body: { id: 55, tag_name: 'v0.4.1', draft, assets: [] } };
    if (req.method === 'PATCH' && url.pathname === '/repos/fixture/owned/releases/55') {
      assert.equal(JSON.parse(bytes).draft, false); draft = false;
      return { body: { id: 55, tag_name: 'v0.4.1', draft, assets: [] } };
    }
    throw new Error('Unexpected route including release listing: ' + req.url);
  });
  const release = workflow('pipeline').jobs.release.steps;
  const query = release.find(s => s.id === 'lookup').with.query;
  let result = await action(t, 'GRAPHQL_ACTION', endpoint, { query, variables: 'owner: fixture\nrepo: owned\ntag: v0.4.1' });
  assert.equal(result.code, 0, result.text); assert.equal(JSON.parse(result.outputs.data).repository.release.databaseId, 55);
  const read = release.find(s => s.id === 'before_upload');
  result = await action(t, 'REQUEST_ACTION', endpoint, { route: read.with.route, repo: 'fixture/owned', release_id: '55' });
  assert.equal(result.code, 0, result.text); assert.equal(JSON.parse(result.outputs.data).id, 55);
  const finish = release.at(-1);
  result = await action(t, 'REQUEST_ACTION', endpoint, { route: finish.with.route, repo: 'fixture/owned', release_id: '55', draft: 'false', make_latest: 'legacy', body: 'owned completion' });
  assert.equal(result.code, 0, result.text); assert.equal(draft, false);
  failedLookup = true;
  result = await action(t, 'GRAPHQL_ACTION', endpoint, { query, variables: 'owner: fixture\nrepo: owned\ntag: v0.4.1' });
  assert.equal(result.code, 1, result.text);
  assert.ok(!events.some(e => e === 'GET /repos/fixture/owned/releases'));
});
