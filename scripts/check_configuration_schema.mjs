// Development-only standards-engine replay; no runtime dependency or installer.
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve, dirname } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';

const args = process.argv.slice(2);
assert.equal(args.length, 4, 'Use --validator /existing/ajv/dist/2020.js --binary /built/reader-buddy');
assert.equal(args[0], '--validator');
assert.equal(args[2], '--binary');
const validatorPath = resolve(args[1]);
const require = createRequire(import.meta.url);
const { default: Ajv2020 } = require(validatorPath);
const version = JSON.parse(readFileSync(resolve(dirname(validatorPath), '../package.json'), 'utf8')).version;
const embedded = readFileSync(new URL('../src/config/configuration-schema.json', import.meta.url));
const emitted = execFileSync(resolve(args[3]), ['--configuration-schema'], { timeout: 10_000, maxBuffer: 65_537 });
assert.deepEqual(emitted, embedded, 'Actual CLI must emit the exact embedded document');
assert(emitted.length <= 65_536 && emitted.at(-1) === 10 && !emitted.includes(13));
const schema = JSON.parse(emitted.toString('utf8'));
const engine = new Ajv2020({ strict: true, allErrors: true, validateFormats: false, useDefaults: false });
assert(engine.validateSchema(schema), JSON.stringify(engine.errors));
const validate = engine.compile(schema);
const corpus = JSON.parse(readFileSync(new URL('../tests/fixtures/configuration-schema-corpus.json', import.meta.url), 'utf8'));
for (const test of corpus.cases) {
  const value = JSON.parse(test.raw_utf8);
  const before = JSON.stringify(value);
  assert.equal(validate(value), test.schema_valid, `${test.name}: ${JSON.stringify(validate.errors)}`);
  assert.equal(JSON.stringify(value), before, `Defaults must never mutate ${test.name}`);
}
console.log(JSON.stringify({ validator: 'Ajv Draft2020-12', validator_version: version, document_id: schema.$id,
  document_sha256: createHash('sha256').update(emitted).digest('hex'), document_bytes: emitted.length,
  actual_cli: true, schema_meta_validation: true, structural_cases_passed: corpus.cases.length, defaults_mutated: false,
  typed_parser_results: 'Separate Rust original-byte corpus check; schema success alone is not typed/runtime admission.' }));
