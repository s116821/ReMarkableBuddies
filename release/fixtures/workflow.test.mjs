name: CI
on:
  pull_request:
    branches:
      - main
    types:
      - opened
      - synchronize
      - reopened
      - edited
  push:
    branches:
      - main
permissions:
  contents: read
  pull-requests: read
jobs:
  title:
    name: PR Title
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    outputs:
      application: ${{ steps.paths.outputs.application }}
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}
          fetch-depth: 0
          persist-credentials: false
      - name: Refuse an incomplete PR file inventory
        if: github.event_name == 'pull_request' && (github.event.pull_request.changed_files <= 0 || github.event.pull_request.changed_files >= 3000)
        run: exit 1
      - uses: dorny/paths-filter@c…7444 tokens truncated…> s.name === 'Official tag only after every preceding job passed');
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
