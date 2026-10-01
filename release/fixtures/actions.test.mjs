// Execute pinned upstream distributions with local Git/API boundaries only.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, writeFileSync, readFileSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { Lexer, Parser, Evaluator, data } from '@actions/expressions';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const workflow = name => parse(readFileSync(join(root, '.github/workflows', name + '.yml'), 'utf8'));
const pins = {
  PATHS_ACTION: 'ceb8a2b8f2d89434be7ff52d3de7ec3738c5cc9d',
  GRAPHQL_ACTION: 'ddde8ebb2493e79f390e6449c725c21663a67505',
  REQUEST_ACTION: 'b91aabaa861c777dcdb14e2387e30eddf04619ae',
  UPLOAD_ACTION: '34491005a5d7ec239a784e460807ce844fde7962',
  TAG_ACTION: 'a1c7777fcb2fee4f19b0f283ba888afa11678b72',
};
function upstream(key) {
  assert.ok(process.env[key], `Set ${key} to a clone of the pinned upstream action`);
  const path = resolve(process.env[key]);
  assert.equal(execFileSync('git', ['-C', path, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), pins[key]);
  return path;
}
function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'release-actions-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}
function command(exe, args, { cwd, env } = {}) {
  return new Promise((done, fail) => {
    const child = spawn(exe, args, { cwd, env, windowsHide: true });
    let output = '';
    child.stdout.on('data', b => output += b);
    child.stderr.on('data', b => output += b);
    const timer = setTimeout(() => { child.kill(); fail(new Error('Fixture process deadline')); }, 45000);
    child.on('error', fail);
    child.on('close', code => { clearTimeout(timer); done({ code, output }); });
  });
}
async function server(t, handler) {
  const http = createServer(async (req, res) => {
    try {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const value = await handler(req, Buffer.concat(chunks));
      res.writeHead(value.status ?? 200, { 'Content-Type': 'application/json', ...value.headers });
      res.end(JSON.stringify(value.body ?? {}));
    } catch (error) { res.writeHead(500); res.end(JSON.stringify({ message: error.message })); }
  });
  await new Promise(done => http.listen(0, '127.0.0.1', done));
  t.after(() => new Promise(done => http.close(done)));
  return `http://127.0.0.1:${http.address().port}`;
}
async function action(t, key, endpoint, inputs, payload = { pull_request: { number: 7 } }) {
  const actionPath = upstream(key);
  const meta = parse(readFileSync(join(actionPath, 'action.yml'), 'utf8'));
  const dir = fixture(t);
  const event = join(dir, 'event.json');
  writeFileSync(event, JSON.stringify(payload));
  const env = { ...process.env, GITHUB_ACTION: 'fixture', GITHUB_REPOSITORY: 'fixture/repo', GITHUB_EVENT_NAME: 'pull_request_target', GITHUB_EVENT_PATH: event, GITHUB_API_URL: endpoint, GITHUB_TOKEN: 'fixture-only', GITHUB_WORKSPACE: root };
  for (const file of ['OUTPUT', 'ENV', 'PATH', 'STEP_SUMMARY']) {
    env['GITHUB_' + file] = join(dir, file);
    writeFileSync(env['GITHUB_' + file], '');
  }
  for (const [name, input] of Object.entries(meta.inputs ?? {})) {
    if (input.default !== undefined) env['INPUT_' + name.toUpperCase()] = String(input.default).includes('${{') ? 'fixture-only' : String(input.default);
  }
  for (const [name, value] of Object.entries(inputs)) env['INPUT_' + name.toUpperCase()] = String(value);
  const result = await command(process.execPath, [join(actionPath, meta.runs.main)], { cwd: root, env });
  result.outputs = readFileSync(env.GITHUB_OUTPUT, 'utf8');
  return result;
}
function outputMap(text) {
  const lines = text.split(/\r?\n/), outputs = {};
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index], marker = line.indexOf('<<');
    if (marker >= 0) {
      const key = line.slice(0, marker), delimiter = line.slice(marker + 2), content = [];
      while (++index < lines.length && lines[index] !== delimiter) content.push(lines[index]);
      outputs[key] = content.join('\n');
    } else if (line.includes('=')) { const i = line.indexOf('='); outputs[line.slice(0,i)] = line.slice(i+1); }
  }
  return outputs;
}
function outputValue(text, key) { return outputMap(text)[key]; }
function value(input) {
  if (input === null || input === undefined) return new data.Null();
  if (typeof input === 'string') return new data.StringData(input);
  if (typeof input === 'number') return new data.NumberData(input);
  if (typeof input === 'boolean') return new data.BooleanData(input);
  return new data.Dictionary(...Object.entries(input).map(([key, item]) => ({ key, value: value(item) })));
}
function evaluate(expression, contexts) {
  const tokens = new Lexer(expression.replace(/^\$\{\{\s*|\s*\}\}$/g, '')).lex().tokens;
  return new Evaluator(new Parser(tokens, Object.keys(contexts), []).parse(), value(contexts)).evaluate().coerceString();
}

test('actual workflow expressions refuse incomplete observations and skip closed/docs/published work', () => {
  const release = workflow('release');
  const ci = workflow('ci');
  const publish = workflow('publish');
  for (const [event, steps] of [['pull_request_target', release.jobs.admission.steps], ['pull_request', ci.jobs.policy.steps]]) {
    const guard = steps.find(s => s.name === 'Require complete PR file observation').if;
    for (const [count, fails] of [[undefined, true], [0, true], [1, false], [2999, false], [3000, true], [3001, true]]) {
      assert.equal(evaluate(guard, { github: { event_name: event, event: { pull_request: { changed_files: count } } } }), String(fails));
    }
  }
  assert.equal(evaluate(release.jobs.admission.if, { github: { event_name: 'pull_request_target', event: { pull_request: { merged: false } } } }), 'false');
  assert.equal(evaluate(release.jobs.publish.if, { github: { event_name: 'pull_request_target' }, needs: { admission: { outputs: { application: 'false' } } } }), 'false');
  assert.equal(evaluate(publish.jobs.build.if, { needs: { tag: { outputs: { unfinished: 'false' } } } }), 'false');
  assert.equal(publish.concurrency.queue, 'max');
  const uploader = publish.jobs.publish.steps.findIndex(s => s.uses?.startsWith('AButler/'));
  const final = publish.jobs.publish.steps.findIndex(s => s.with?.route?.startsWith('PATCH'));
  assert.ok(uploader > 0 && final > uploader);
  assert.equal(publish.jobs.publish.steps[final].with.draft, false);
  assert.equal(publish.jobs.publish.steps[final].with.release_id, '${{ needs.tag.outputs.release_id }}');
  const inventoryGuard = publish.jobs.publish.steps.find(s => s.name === 'Require same draft and bounded upload inventory').if;
  for (const [count, refused] of [[0, false], [26, false], [27, true], [null, true], ['0', true]]) {
    assert.equal(evaluate(inventoryGuard, { steps: { before_upload: { outputs: { data: JSON.stringify({ repository: { release: { databaseId: 55, tagName: 'v0.2.1', isDraft: true, releaseAssets: { totalCount: count } } } }) } } }, needs: { tag: { outputs: { tag: 'v0.2.1', release_id: '55' } } } }), String(refused));
  }

});

test('paths-filter actual bundle sees later pages and both sides of a rename', async t => {
  let files = ['README.md', 'docs/images/proof.png', 'docs/images/deep/diagram.svg', 'docs/licenses/third-party.txt'].map(filename => ({ filename, status: 'modified' }));
  let multipage = false;
  let requests = 0;
  const endpoint = await server(t, req => {
    requests++;
    const url = new URL(req.url, 'http://localhost');
    assert.equal(url.pathname, '/repos/fixture/repo/pulls/7/files');
    if (multipage && url.searchParams.get('page') !== '2') return { body: [{ filename: 'docs/guide.md', status: 'modified' }], headers: { Link: `<${endpoint}/repos/fixture/repo/pulls/7/files?page=2>; rel="next"` } };
    return { body: files };
  });
  const run = () => action(t, 'PATHS_ACTION', endpoint, { token: 'fixture-only', filters: readFileSync(join(root, '.github/application-paths.yml'), 'utf8'), 'predicate-quantifier': 'every' });
  let result = await run();
  assert.equal(result.code, 0, result.output);
  assert.equal(outputValue(result.outputs, 'application'), 'false');
  files = [{ filename: 'docs/fixture.json', status: 'modified' }];
  multipage = true;
  result = await run();
  assert.equal(result.code, 0, result.output);
  assert.equal(outputValue(result.outputs, 'application'), 'true');
  assert.equal(requests, 3);
  multipage = false;
  files = [{ filename: 'docs/moved.md', previous_filename: 'src/old.rs', status: 'renamed' }];
  result = await run();
  assert.equal(result.code, 0, result.output);
  assert.equal(outputValue(result.outputs, 'application'), 'true');
  for (const filename of ['docs/images/helper.py', 'docs/validation/images/data.json', 'docs/licenses/tool.sh']) {
    files = [{ filename, status: 'modified' }];
    result = await run();
    assert.equal(result.code, 0, result.output);
    assert.equal(outputValue(result.outputs, 'application'), 'true', filename);
  }
});

test('actual publisher distributions recover a page-three draft by ID and keep partial uploads private', async t => {
  const publish = workflow('publish');
  const assets = fixture(t);
  const upload = publish.jobs.publish.steps.find(s => s.uses?.startsWith('AButler/'));
  const fileNames = upload.with.files.split(';').map(path => path.split('/').at(-1));
  assert.equal(new Set(fileNames).size, fileNames.length);
  for (const name of fileNames) writeFileSync(join(assets, name), 'fixture package ' + name);
  let mode = 'draft', failedAsset = true, built = 0;
  let uploaded = [], created = 0, published = 0, queries = 0;
  const oldDraft = { id: 55, tag_name: 'v0.2.1', draft: true };
  // A list-based publisher would have to reach page three, which is intentionally
  // not available to this composition: its upstream direct lookup must suffice.
  const releaseListing = [...Array.from({ length: 200 }, (_, i) => ({ id: i + 100, draft: false })), oldDraft];
  assert.equal(releaseListing.indexOf(oldDraft), 200);
  const endpoint = await server(t, (req, bytes) => {
    const url = new URL(req.url, 'http://fixture');
    if (url.pathname === '/graphql') {
      queries++;
      const request = JSON.parse(bytes);
      assert.equal(request.variables.tag, 'v0.2.1');
      assert.match(request.query, /release\(tagName: \$tag\)/);
      if (mode === 'error') return { body: { errors: [{ message: 'lookup refused' }] } };
      let observed = { databaseId: 55, tagName: 'v0.2.1', isDraft: oldDraft.draft, releaseAssets: { totalCount: uploaded.length } };
      if (mode === 'absent') observed = null;
      if (mode === 'zero-release') observed = 0;
      if (mode === 'false-release') observed = false;
      if (mode === 'missing-repository') return { body: { data: { repository: null } } };
      if (mode === 'missing-release') return { body: { data: { repository: {} } } };
      if (mode === 'wrong-tag') observed.tagName = 'v0.9.9';
      if (mode === 'bad-id') observed.databaseId = '55';
      if (mode === 'fraction-id') observed.databaseId = 55.5;
      if (mode === 'boolean-id') observed.databaseId = true;
      if (mode === 'missing-id') delete observed.databaseId;
      if (mode === 'bad-draft') observed.isDraft = 'true';
      if (mode === 'missing-count') delete observed.releaseAssets.totalCount;
      if (mode === 'null-count') observed.releaseAssets.totalCount = null;
      if (mode === 'string-count') observed.releaseAssets.totalCount = '1';
      if (mode === 'boolean-count') observed.releaseAssets.totalCount = true;
      if (mode === 'fraction-count') observed.releaseAssets.totalCount = 1.5;
      if (mode === 'too-many') observed.releaseAssets.totalCount = 27;
      return { body: { data: { repository: { release: observed } } } };
    }
    if (req.method === 'POST' && url.pathname === '/repos/fixture/repo/releases') {
      created++;
      assert.equal(mode, 'absent', 'duplicate POST for existing old draft');
      const body = JSON.parse(bytes);
      assert.equal(body.tag_name, oldDraft.tag_name); assert.equal(body.draft, true);
      mode = 'draft'; oldDraft.draft = true;
      return { status: 201, body: oldDraft };
    }
    if (req.method === 'GET' && url.pathname === '/repos/fixture/repo/releases/55') return { body: { ...oldDraft, upload_url: `${endpoint}/upload/55{?name,label}`, html_url: 'https://example.invalid/55' } };
    if (req.method === 'GET' && url.pathname === '/repos/fixture/repo/releases/55/assets') return { body: uploaded };
    if (req.method === 'DELETE' && url.pathname.startsWith('/repos/fixture/repo/releases/assets/')) {
      uploaded = uploaded.filter(a => a.id !== Number(url.pathname.split('/').at(-1)));
      return { status: 204 };
    }
    if (req.method === 'POST' && url.pathname === '/upload/55') {
      assert.equal(oldDraft.draft, true, 'asset upload to public release');
      if (failedAsset && uploaded.length === 1) return { status: 422, body: { message: 'partial upload failure' } };
      const asset = { id: uploaded.length + 10, name: url.searchParams.get('name') };
      uploaded.push(asset); return { status: 201, body: asset };
    }
    if (req.method === 'PATCH' && url.pathname === '/repos/fixture/repo/releases/55') {
      assert.deepEqual(new Set(uploaded.map(a => a.name)), new Set(fileNames));
      const body = JSON.parse(bytes); assert.equal(body.draft, false); assert.equal(body.make_latest, 'legacy');
      published++; oldDraft.draft = false; return { body: oldDraft };
    }
    throw new Error('Unexpected route (release listing is forbidden): ' + req.url);
  });
  const run = async () => {
    const contexts = { github: { repository: 'fixture/repo', repository_owner: 'fixture', event: { repository: { name: 'repo' } } }, secrets: { GITHUB_TOKEN: 'fixture-only' }, steps: { identity: { outputs: { tag: 'v0.2.1', sha: 'a'.repeat(40) } }, create: { outputs: { data: '', status: '' } } }, needs: {} };
    const interpolate = input => typeof input === 'string' ? input.replace(/\$\{\{[\s\S]*?\}\}/g, expr => evaluate(expr, contexts)) : input;
    const execute = async step => {
      if (step.if && evaluate(step.if, contexts) !== 'true') return true;
      if (step.run === 'exit 1') return false;
      const key = step.uses?.startsWith('octokit/graphql') ? 'GRAPHQL_ACTION' : step.uses?.startsWith('octokit/request') ? 'REQUEST_ACTION' : step.uses?.startsWith('AButler/') ? 'UPLOAD_ACTION' : undefined;
      if (!key) return true; // Builds/package integrity and native tag tests are separate.
      const inputs = Object.fromEntries(Object.entries(step.with).map(([k,v]) => [k, interpolate(v)]));
      if (key === 'UPLOAD_ACTION') inputs.files = fileNames.map(name => join(assets, name).replaceAll('\\', '/')).join(';');
      const result = await action(t, key, endpoint, inputs);
      if (result.code !== 0) return false;
      if (step.id) contexts.steps[step.id] = { outputs: outputMap(result.outputs) };
      return true;
    };
    for (const step of publish.jobs.tag.steps.filter(s => s.id === 'lookup' || s.id === 'create' || s.run === 'exit 1')) if (!await execute(step)) return false;
    const outputs = Object.fromEntries(Object.entries(publish.jobs.tag.outputs).map(([k,v]) => [k, interpolate(v)]));
    contexts.needs.tag = { outputs };
    if (evaluate(publish.jobs.build.if, contexts) !== 'true') return true;
    built++;
    for (const step of publish.jobs.publish.steps) if (!await execute(step)) return false;
    return true;
  };
  for (mode of ['error','missing-repository','missing-release','zero-release','false-release','wrong-tag','bad-id','fraction-id','boolean-id','missing-id','bad-draft','missing-count','null-count','string-count','boolean-count','fraction-count','too-many']) {
    assert.equal(await run(), false, mode); assert.equal(built, 0, mode); assert.equal(created, 0, mode); assert.equal(uploaded.length, 0, mode);
  }
  mode = 'draft';
  assert.equal(await run(), false, 'partial upload must fail');
  assert.equal(oldDraft.draft, true); assert.equal(uploaded.length, 1); assert.equal(created, 0); assert.equal(published, 0);
  failedAsset = false;
  assert.equal(await run(), true, 'old same-ID draft retry must finish');
  assert.equal(oldDraft.draft, false); assert.equal(created, 0); assert.equal(published, 1);
  const builtBefore = built, queriesBefore = queries;
  assert.equal(await run(), true, 'published skip');
  assert.equal(built, builtBefore); assert.equal(published, 1); assert.equal(queries, queriesBefore + 1);
  uploaded = []; mode = 'absent';
  assert.equal(await run(), true, 'explicit absent release can create'); assert.equal(created, 1); assert.equal(published, 2);
});


test('tag action publishes exact SHA, reuses without moving, and workflow rejects conflict', async t => {
  const directory = fixture(t);
  const source = join(directory, 'source');
  const remote = join(directory, 'remote.git');
  mkdirSync(source);
  const git = (...args) => execFileSync('git', args, { cwd: source, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git('init', '-b', 'main');
  git('config', 'user.name', 'Release fixture');
  git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(source, 'app.txt'), 'first');
  git('add', '.'); git('commit', '-m', 'fix(REM-46): first');
  const sha = git('rev-parse', 'HEAD');
  git('init', '--bare', remote); git('remote', 'add', 'origin', remote); git('push', '-u', 'origin', 'main');
  const env = { ...process.env, GITHUB_WORKSPACE: source.replaceAll('\\', '/'), GITHUB_ACTOR: 'fixture', GITHUB_SHA: sha, GITHUB_ACTION: 'fixture', GITHUB_REPOSITORY: 'fixture/repo', INPUT_TAG: 'v0.2.1', INPUT_COMMIT_SHA: sha, INPUT_GITHUB_TOKEN: '', INPUT_FORCE_PUSH_TAG: 'false', INPUT_TAG_EXISTS_ERROR: 'false', GIT_CONFIG_GLOBAL: join(directory, 'gitconfig').replaceAll('\\', '/'), GIT_CONFIG_NOSYSTEM: '1' };
  for (const name of ['GITHUB_OUTPUT', 'GITHUB_ENV']) { env[name] = join(directory, name).replaceAll('\\', '/'); writeFileSync(env[name], ''); }
  const shell = process.env.BASH ?? (process.platform === 'win32' ? 'C:/Program Files/Git/bin/bash.exe' : 'bash');
  let result = await command(shell, [join(upstream('TAG_ACTION'), 'entrypoint.sh').replaceAll('\\', '/')], { cwd: source, env });
  assert.equal(result.code, 0, result.output);
  assert.equal(git('ls-remote', 'origin', 'refs/tags/v0.2.1^{}').split(/\s/)[0], sha);
  writeFileSync(join(source, 'app.txt'), 'second'); git('add', '.'); git('commit', '-m', 'fix(REM-46): second');
  const next = git('rev-parse', 'HEAD');
  result = await command(shell, [join(upstream('TAG_ACTION'), 'entrypoint.sh').replaceAll('\\', '/')], { cwd: source, env: { ...env, INPUT_COMMIT_SHA: next } });
  assert.equal(result.code, 0, result.output); // Upstream skip alone is not proof.
  const guard = workflow('publish').jobs.tag.steps.find(s => s.name === 'Verify remote tag before any app build').run;
  result = await command(shell, ['-c', guard], { cwd: source, env: { ...env, TAG: 'v0.2.1', SHA: next } });
  assert.notEqual(result.code, 0, 'Conflicting tag must prevent compilation');
  result = await command(shell, ['-c', guard], { cwd: source, env: { ...env, TAG: 'v0.2.1', SHA: sha } });
  assert.equal(result.code, 0, result.output);
});
