import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
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
  const pr = count(ci.jobs.checks) + count(pipeline.jobs.build) + count(pipeline.jobs.integration);
  assert.equal(pr, manager ? 4 : 3);
  assert.equal(pr + count(pipeline.jobs.release), manager ? 5 : 4);
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
  assert.deepEqual(Object.keys(ci.jobs), ['checks','application']);
  assert.deepEqual(Object.keys(pipeline.jobs), ['build','integration','release']);
  assert.deepEqual(ci.on.push.branches, ['main']);
  assert.equal(ci.on.pull_request_target, undefined);
  assert.deepEqual(ci.jobs.application.needs, 'checks');
  assert.equal(ci.jobs.application.if, "needs.checks.outputs.application == 'true'");
  assert.equal(pipeline.jobs.build.needs, undefined);
  assert.deepEqual(pipeline.jobs.integration.needs, 'build');
  assert.deepEqual(pipeline.jobs.release.needs, ['build','integration']);
  assert.equal(pipeline.jobs.release.if, 'inputs.release');
  assert.equal(pipeline.concurrency.queue, 'max');
  assert.equal(pipeline.concurrency['cancel-in-progress'], false);
  for (const job of [ci.jobs.checks, ...Object.values(pipeline.jobs)]) {
    assert.equal(job['continue-on-error'], undefined);
    assert.ok(!/always\(|cancelled\(|failure\(/.test(job.if ?? ''));
  }
  assert.equal(ci.jobs.checks.strategy, undefined);
  assert.equal(ci.jobs.checks['runs-on'], manager ? 'windows-latest' : 'ubuntu-latest');
  if (manager) {
    assert.deepEqual(pipeline.jobs.integration.strategy.matrix.os, ['ubuntu-latest','windows-latest']);
    assert.equal(pipeline.jobs.build['runs-on'], 'ubuntu-latest');
    const steps = pipeline.jobs.build.steps;
    const freeze = steps.findIndex(s => s.id === 'freeze');
    const sourceChecks = ci.jobs.checks.steps.filter(s => /^npm (run lint|test|run test:release)$/.test(s.run ?? ''));
    for (const check of sourceChecks)
      assert.ok(steps.findIndex(s => s.run === check.run) >= 0 && steps.findIndex(s => s.run === check.run) < freeze);
    const wired = steps.findIndex(s => /unittest discover -s tests\/wired/.test(s.run ?? ''));
    assert.ok(wired >= 0 && wired < freeze);
  } else {
    assert.equal(pipeline.jobs.integration.strategy, undefined);
  }
  assert.equal(ci.jobs.checks.defaults.run.shell, 'bash');
  assert.equal(pipeline.jobs.integration.defaults.run.shell, 'bash');
});

test('docs-only runs retain title checks without compiling or entering the release queue', () => {
  const ci = workflow('ci'), pipeline = workflow('pipeline'), title = workflow('pr-title');
  assert.deepEqual(title.on.pull_request['branches-ignore'], ['main']);
  const steps = ci.jobs.checks.steps;
  assert.equal(steps.find(s => s.id === 'paths').with['predicate-quantifier'], 'every');
  assert.equal(ci.jobs.checks.outputs.application, expr('steps.paths.outputs.application'));
  const titleSteps = steps.filter(s => s.uses?.startsWith('amannn/'));
  assert.equal(titleSteps.length, 2);
  assert.ok(titleSteps.some(s => s.if.includes("outputs.application == 'true'")));
  assert.ok(titleSteps.some(s => s.if.includes("outputs.application != 'true'")));
  for (const step of steps) {
    if (/cargo |npm |node --test|pip install/.test(step.run ?? '') ||
        /rust-toolchain|setup-node|\.github\/actions\/release-tools/.test(step.uses ?? ''))
      assert.ok(step.if?.startsWith("steps.paths.outputs.application == 'true'"), step.name ?? step.run ?? step.uses);
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
