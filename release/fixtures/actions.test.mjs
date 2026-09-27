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
  RELEASE_ACTION: '339a81892b84b4eeb0f6e744e4574d79d0d9b8dd',
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
  const env = { ...process.env, GITHUB_REPOSITORY: 'fixture/repo', GITHUB_EVENT_NAME: 'pull_request_target', GITHUB_EVENT_PATH: event, GITHUB_API_URL: endpoint, GITHUB_TOKEN: 'fixture-only', GITHUB_WORKSPACE: root };
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
function outputValue(text, key) {
  const lines = text.split(/\r?\n/);
  const first = lines.findIndex(line => line.startsWith(key + '<<'));
  return first < 0 ? undefined : lines[first + 1];
}
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
  const updates = publish.jobs.publish.steps.filter(s => s.uses?.startsWith('ncipollo/'));
  assert.equal(updates[0].with.draft, true);
  assert.ok(updates[0].with.artifacts);
  assert.equal(updates[1].with.draft, false);
  assert.equal(updates[1].with.artifacts, undefined);
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

test('release-action actual bundle keeps failed uploads draft, recovers, and skips published assets', async t => {
  let release;
  let failUpload = false;
  const events = [];
  const endpoint = await server(t, (req, bytes) => {
    const path = new URL(req.url, 'http://localhost').pathname;
    events.push([req.method, path, release?.draft]);
    // GitHub documents this endpoint as published releases only; drafts use list.
    // https://docs.github.com/en/rest/releases/releases#get-a-release-by-tag-name
    if (req.method === 'GET' && path.includes('/releases/tags/')) return release && !release.draft ? { body: release } : { status: 404 };
    if (req.method === 'GET' && path.endsWith('/releases')) return { body: release ? [release] : [] };
    if (req.method === 'GET' && path.endsWith('/assets')) return { body: [] };
    if (req.method === 'POST' && path.endsWith('/releases')) {
      assert.equal(release, undefined, 'existing draft must be reused, never recreated');
      release = { ...JSON.parse(bytes), id: 5, upload_url: `${endpoint}/upload/5{?name,label}`, html_url: 'https://example.invalid/release', assets: [] };
      return { status: 201, body: release };
    }
    if (req.method === 'PATCH' && path.endsWith('/releases/5')) { Object.assign(release, JSON.parse(bytes)); return { body: release }; }
    if (req.method === 'POST' && path === '/upload/5') {
      assert.equal(release.draft, true, 'public release before all uploads completed');
      if (failUpload) return { status: 422, body: { message: 'fixture upload failure' } };
      return { status: 201, body: { id: 9, browser_download_url: 'https://example.invalid/asset' } };
    }
    return { status: 404, body: { message: 'unexpected fixture route: ' + path } };
  });
  const common = { token: 'fixture-only', tag: 'v0.2.1', commit: 'a'.repeat(40), allowUpdates: true, updateOnlyUnreleased: true, skipIfReleaseExists: true, draft: true, omitBodyDuringUpdate: true };
  let result = await action(t, 'RELEASE_ACTION', endpoint, common);
  assert.equal(result.code, 0, result.output);
  assert.equal(outputValue(result.outputs, 'id'), '5');
  const assets = fixture(t);
  writeFileSync(join(assets, 'package.zip'), 'fixture archive');
  failUpload = true;
  result = await action(t, 'RELEASE_ACTION', endpoint, { ...common, artifacts: join(assets, '*.zip').replaceAll('\\', '/'), artifactErrorsFailBuild: true });
  assert.notEqual(result.code, 0, result.output);
  assert.equal(release.draft, true);
  failUpload = false;
  result = await action(t, 'RELEASE_ACTION', endpoint, { ...common, artifacts: join(assets, '*.zip').replaceAll('\\', '/'), artifactErrorsFailBuild: true });
  assert.equal(result.code, 0, result.output);
  assert.equal(release.draft, true);
  const uploadCount = events.filter(([method, path]) => method === 'POST' && path.startsWith('/upload/')).length;
  result = await action(t, 'RELEASE_ACTION', endpoint, { ...common, draft: false, makeLatest: 'legacy' });
  assert.equal(result.code, 0, result.output);
  assert.equal(release.draft, false);
  assert.equal(events.filter(([method, path]) => method === 'POST' && path.startsWith('/upload/')).length, uploadCount);
  const writes = events.filter(([method]) => method !== 'GET').length;
  result = await action(t, 'RELEASE_ACTION', endpoint, common);
  assert.equal(result.code, 0, result.output);
  assert.equal(outputValue(result.outputs, 'id'), undefined);
  assert.equal(events.filter(([method]) => method !== 'GET').length, writes);
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
  const env = { ...process.env, GITHUB_WORKSPACE: source.replaceAll('\\', '/'), GITHUB_ACTOR: 'fixture', GITHUB_SHA: sha, GITHUB_REPOSITORY: 'fixture/repo', INPUT_TAG: 'v0.2.1', INPUT_COMMIT_SHA: sha, INPUT_GITHUB_TOKEN: '', INPUT_FORCE_PUSH_TAG: 'false', INPUT_TAG_EXISTS_ERROR: 'false', GIT_CONFIG_GLOBAL: join(directory, 'gitconfig').replaceAll('\\', '/'), GIT_CONFIG_NOSYSTEM: '1' };
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
