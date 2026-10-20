import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { Parser, Lexer, Evaluator, data } from '@actions/expressions';
const root = fileURLToPath(new URL('../..', import.meta.url));
const workflow = name => parse(readFileSync(path.join(root, '.github/workflows', name + '.yml'), 'utf8'));
const expr = text => String.fromCharCode(36) + '{{ ' + text + ' }}';
const manager = false;

test('PR and main job budgets include every matrix expansion', () => {
  const ci = workflow('ci'), pipeline = workflow('pipeline');
  const count = job => {
    if (!job.strategy) return 1;
    assert.deepEqual(Object.keys(job.strategy.matrix), ['os'], 'Account for every added matrix dimension');
    return job.strategy.matrix.os.length;
  };
  const common = count(ci.jobs.checks) + count(pipeline.jobs.build) + count(pipeline.jobs.integration);
  assert.equal(common + count(ci.jobs.title), manager ? 5 : 4);
  assert.equal(common + count(pipeline.jobs.release), manager ? 5 : 4);
  for (const file of readdirSync(path.join(root, '.github/workflows')).filter(f => /\.ya?ml$/.test(f))) {
    if (['ci.yml','pipeline.yml','pr-title.yml'].includes(file)) continue;
    const triggers = parse(readFileSync(path.join(root, '.github/workflows', file), 'utf8')).on;
    const names = typeof triggers === 'string' ? [triggers] : Array.isArray(triggers) ? triggers : Object.keys(triggers ?? {});
    assert.ok(!names.some(event => ['push','pull_request','pull_request_target'].includes(event)),
      file + ' adds a separate PR/push workflow; incorporate its jobs into the stage budget');
  }
});

test('four stages preserve platform coverage and block dependent jobs on failure', () => {
  const ci = workflow('ci'), pipeline = workflow('pipeline');
  assert.deepEqual(Object.keys(ci.jobs), ['title','checks','application']);
  assert.deepEqual(Object.keys(pipeline.jobs), ['build','integration','release']);
  assert.deepEqual(ci.on.push.branches, ['main']);
  assert.equal(ci.on.pull_request_target, undefined);
  assert.equal(ci.jobs.title.if, "github.event_name == 'pull_request'");
  assert.equal(ci.jobs.checks.needs, 'title');
  assert.equal(ci.jobs.checks.if, "!cancelled() && (github.event_name == 'push' || needs.title.result == 'success') && needs.title.outputs.body_only != 'true'");
  assert.deepEqual(ci.jobs.application.needs, ['title','checks']);
  assert.equal(ci.jobs.application.if, "!cancelled() && needs.checks.result == 'success' && needs.checks.outputs.application == 'true'");
  assert.equal(pipeline.jobs.build.needs, undefined);
  assert.deepEqual(pipeline.jobs.integration.needs, 'build');
  assert.deepEqual(pipeline.jobs.release.needs, ['build','integration']);
  assert.equal(pipeline.jobs.release.if, 'inputs.release');
  assert.equal(pipeline.concurrency.queue, 'max');
  assert.equal(pipeline.concurrency['cancel-in-progress'], false);
  assert.equal(ci.jobs.checks['continue-on-error'], undefined);
  for (const job of [ci.jobs.title, ...Object.values(pipeline.jobs)]) {
    assert.equal(job['continue-on-error'], undefined);
    assert.ok(!/always\(|cancelled\(|failure\(/.test(job.if ?? ''));
  }
  assert.equal(ci.jobs.checks.strategy, undefined);
  assert.equal(ci.jobs.checks['runs-on'], 'ubuntu-latest');
  if (manager) {
    assert.deepEqual(pipeline.jobs.integration.strategy.matrix.os, ['ubuntu-latest','windows-latest']);
    assert.equal(pipeline.jobs.build['runs-on'], 'ubuntu-latest');
  } else {
    assert.equal(pipeline.jobs.integration.strategy, undefined);
  }
  assert.equal(ci.jobs.checks.defaults.run.shell, 'bash');
  assert.equal(pipeline.jobs.integration.defaults.run.shell, 'bash');
});

function evaluate(text, context, canceled = false) {
  const source = String(text).replace(/^\$\{\{\s*|\s*\}\}$/g, '');
  const functions = new Map([['cancelled', {
    name: 'cancelled', minArgs: 0, maxArgs: 0,
    call: () => new data.BooleanData(canceled)
  }]]);
  const ast = new Parser(new Lexer(source).lex().tokens, Object.keys(context), [...functions.values()]).parse();
  return new Evaluator(ast, JSON.parse(JSON.stringify(context), data.reviver), functions).evaluate();
}
function bodyOnly(event) {
  const step = workflow('ci').jobs.title.steps.find(s => s.id === 'metadata');
  const match = step.run.match(/jq -r '([\s\S]*?)' "\$GITHUB_EVENT_PATH"/);
  assert.ok(match, 'Exercise the actual workflow jq predicate');
  const result = spawnSync(process.env.METADATA_JQ || 'jq', ['-r', match[1]], {
    input: JSON.stringify(event), encoding: 'utf8', windowsHide: true
  });
  assert.ifError(result.error); assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout.trim(), /^(true|false)$/);
  return result.stdout.trim();
}
test('structured body-only predicate refuses base, mixed, missing and unknown details', () => {
  for (const from of ['', 'old body', '{"title":"not an event key","base":{}}'])
    assert.equal(bodyOnly({action:'edited',changes:{body:{from}}}), 'true');
  const base = {ref:{from:'old-base'},sha:{from:'a'.repeat(40)}};
  for (const changes of [undefined, null, {}, [], 'body', {title:{from:'old'}}, {base},
    {body:{from:'old'},title:{from:'old'}}, {body:{from:'old'},base},
    {body:{from:'old'},unknown:true}, {unknown:true}, {body:{}},
    {body:{from:null}}, {body:{from:7}}, {body:{from:true}},
    {body:{from:'old',unknown:true}}, {body:'old'}])
    assert.equal(bodyOnly({action:'edited',changes}), 'false', JSON.stringify(changes));
  assert.equal(bodyOnly({action:'synchronize',changes:{body:{from:'old'}}}), 'false');
});
test('actual names and guards isolate metadata without masking normal application results', () => {
  const ci = workflow('ci');
  const metadataIndex = ci.jobs.title.steps.findIndex(s => s.id === 'metadata');
  assert.ok(metadataIndex >= 0 && metadataIndex < ci.jobs.title.steps.findIndex(s => s.uses?.startsWith('amannn/')));
  assert.equal(ci.jobs.title.outputs.body_only, expr('steps.metadata.outputs.body_only'));
  for (const body_only of ['true','false','']) {
    const context = {github:{event_name:'pull_request'}, needs:{
      title:{result:'success',outputs:{body_only,application:'true'}},
      checks:{result:body_only==='true'?'skipped':'success',outputs:{application:'true'}}
    }};
    assert.equal(evaluate(ci.jobs.checks.name,context).coerceString(), body_only==='true'?'PR metadata (no source checks)':'Checks');
    assert.equal(evaluate(ci.jobs.application.name,context).coerceString(), body_only==='true'?'PR metadata (no artifact jobs)':'application');
    assert.equal(evaluate(ci.jobs.checks.if,context).coerceString(), body_only==='true'?'false':'true');
    assert.equal(evaluate(ci.jobs.application.if,context).coerceString(), body_only==='true'?'false':'true');
    context.needs.title.result='failure';
    assert.equal(evaluate(ci.jobs.checks.if,context).coerceString(),'false');
    if (body_only==='true') assert.notEqual(evaluate(ci.jobs.checks.name,context).coerceString(),'Checks');
  }
});
test('main skipped-title and failed or canceled prerequisites keep real stage gates', () => {
  const ci = workflow('ci');
  const context = {github:{event_name:'push'},needs:{
    title:{result:'skipped',outputs:{}},checks:{result:'success',outputs:{application:'true'}}
  }};
  assert.equal(evaluate(ci.jobs.checks.if,context).coerceString(),'true');
  assert.equal(evaluate(ci.jobs.application.if,context).coerceString(),'true');
  assert.equal(evaluate(ci.jobs.checks.name,context).coerceString(),'Checks');
  assert.equal(evaluate(ci.jobs.application.name,context).coerceString(),'application');
  for (const result of ['failure','cancelled','skipped']) {
    context.needs.checks.result=result;
    assert.equal(evaluate(ci.jobs.application.if,context).coerceString(),'false');
  }
  assert.equal(evaluate(ci.jobs.checks.if,context,true).coerceString(),'false');
  assert.equal(evaluate(ci.jobs.application.if,context,true).coerceString(),'false');
});

test('common source and dependency checks have exactly one owning job', () => {
  const checks = workflow('ci').jobs.checks;
  const pipeline = workflow('pipeline');
  const common = /cargo (?:fmt|clippy|test)|npm (?:run lint|test|run test:release|audit)|unittest discover|node --test release\/fixtures\/workflow/;
  const sourceChecks = checks.steps.filter(step => common.test(step.run ?? ''));
  assert.ok(sourceChecks.length >= 4);
  assert.equal(checks.steps.filter(step => step.run === 'npm audit --prefix release/fixtures').length, 1);
  const tools = checks.steps.find(step => step.uses === './.github/actions/release-tools');
  assert.notEqual(tools.with?.['audit-dependencies'], 'false');
  for (const [jobName, job] of Object.entries(pipeline.jobs)) {
    for (const step of job.steps) {
      assert.ok(!common.test(step.run ?? ''), jobName + ' repeats a Checks-owned suite: ' + step.run);
      if (step.uses === './.github/actions/release-tools')
        assert.equal(step.with?.['audit-dependencies'], 'false', jobName + ' repeats the locked Python dependency audit');
    }
  }
  const action = parse(readFileSync(path.join(root, '.github/actions/release-tools/action.yml'), 'utf8'));
  assert.equal(action.inputs['audit-dependencies'].default, 'true');
  const audit = action.runs.steps.find(step => /pip_audit/.test(step.run ?? ''));
  assert.equal(audit.if, "inputs.audit-dependencies == 'true'");
});

test('docs-only runs retain title checks without compiling or entering the release queue', () => {
  const ci = workflow('ci'), pipeline = workflow('pipeline'), title = workflow('pr-title');
  assert.deepEqual(title.on.pull_request['branches-ignore'], ['main']);
  const steps = ci.jobs.checks.steps;
  assert.equal(steps.find(s => s.id === 'paths').with['predicate-quantifier'], 'every');
  assert.equal(ci.jobs.checks.outputs.application, expr("github.event_name == 'pull_request' && needs.title.outputs.application || steps.paths.outputs.application"));
  assert.equal(steps.find(s => s.id === 'paths').if, "github.event_name == 'push'");
  assert.equal(ci.jobs.title.outputs.application, expr('steps.paths.outputs.application'));
  assert.equal(ci.jobs.title.steps.find(s => s.id === 'paths').if, undefined, 'PR admission must actually classify its changed files');
  const titleSteps = ci.jobs.title.steps.filter(s => s.uses?.startsWith('amannn/'));
  assert.equal(titleSteps.length, 2);
  assert.ok(titleSteps.some(s => s.if.includes("outputs.application == 'true'")));
  assert.ok(titleSteps.some(s => s.if.includes("outputs.application != 'true'")));
  for (const step of steps) {
    if (/cargo |npm |node --test|pip install/.test(step.run ?? '') ||
        /rust-toolchain|setup-node|\.github\/actions\/release-tools/.test(step.uses ?? ''))
      assert.ok(step.if?.startsWith("(needs.title.outputs.application == 'true' || steps.paths.outputs.application == 'true')"), step.name ?? step.run ?? step.uses);
  }
  assert.equal(ci.concurrency, undefined, 'Docs runs must not enter the application queue');
  assert.ok(pipeline.concurrency);
  assert.equal(ci.jobs.application.with.release, expr("github.event_name == 'push' && github.ref == 'refs/heads/main'"));
});

test('only release may publish and it consumes the exact build artifacts without rebuilding', () => {
  const ci = workflow('ci'), pipeline = workflow('pipeline');
  assert.notEqual(ci.jobs.checks.permissions?.contents, 'write');
  for (const [name, job] of Object.entries(pipeline.jobs))
    if (name !== 'release') assert.notEqual(job.permissions?.contents, 'write');
  const build = pipeline.jobs.build, integration = pipeline.jobs.integration, release = pipeline.jobs.release;
  const freeze = build.steps.findIndex(s => s.id === 'freeze');
  const compile = build.steps.findIndex(s => /release\/build.py build/.test(s.run ?? ''));
  assert.ok(freeze >= 0 && compile > freeze);
  assert.match(build.steps[freeze].run, /version --print-tag/);
  assert.ok(!build.steps.some(s => /--push|PATCH |POST /.test(s.run ?? '')));
  assert.ok(!integration.steps.some(s => /release\/build.py build|cargo build|npm run build/.test(s.run ?? '')));
  for (const job of [integration, release]) {
    const downloads = job.steps.filter(s => s.uses?.startsWith('actions/download-artifact'));
    assert.ok(downloads.some(s => s.with['artifact-ids'] === expr('needs.build.outputs.artifact_id')));
    assert.ok(downloads.some(s => s.with['artifact-ids'] === expr('needs.build.outputs.uploader_artifact_id')));
  }
  assert.equal(release.env.TAG, expr('needs.build.outputs.tag'));
  assert.equal(release.env.SHA, expr('needs.build.outputs.sha'));
  const tag = release.steps.findIndex(s => s.name === 'Official tag only after every preceding job passed');
  assert.ok(tag > release.steps.findIndex(s => s.name === 'Verify the exact integration-tested upload inventory'));
  assert.ok(!release.steps.some(s => /cross build|npm run build|coordinator\.py/.test(s.run ?? '')));
  const upload = release.steps.find(s => s.uses === './.qualified/uploader');
  assert.equal(upload.with.overwrite, false); assert.equal(upload.with.draft, true);
  assert.equal(upload.with.promote, false); assert.equal(upload.with.check_duplicates, true);
  assert.equal(upload.with.release_id, expr('steps.retained.outputs.release_id'));
});

if (manager) test('Windows integration stops at each failed native command', () => {
  const integration = workflow('pipeline').jobs.integration;
  const step = integration.steps.find(s => s.if === "runner.os == 'Windows'" && /npm run test:hosts/.test(s.run ?? ''));
  assert.ok(step);
  const commands = step.run.trim().split('\n').map(s => s.trim());
  assert.deepEqual(commands.slice(0, 4), ['npm run test:hosts','npm run test:package','npm run test:wired-hosts','npm run test:wired-hosts -- --packaged']);
  assert.ok(commands.every(command => /^npm run test:[a-z-]+(?: -- --packaged)?$/.test(command)));
  assert.equal(step.shell ?? integration.defaults.run.shell, 'bash');
  for (let failAt=1; failAt<=commands.length; failAt++) {
    const setup = 'calls=0\nnpm() { calls=$((calls+1)); printf "called:%s\\n" "$calls"; if test "$calls" -eq ' + failAt + '; then return 17; fi; }\n';
    const result = spawnSync(process.env.CI_BASH || 'bash', ['--noprofile','--norc','-e','-o','pipefail','-c',setup+step.run],
      {encoding:'utf8', windowsHide:true});
    assert.ifError(result.error);
    assert.equal(result.status, 17, result.stderr);
    assert.equal(result.stdout.trim().split('\n').length, failAt, 'Commands after a failure must not run');
  }
});
