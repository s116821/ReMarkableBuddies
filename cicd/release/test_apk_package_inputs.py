"""Owned synthetic receipt controls; Docker/signing/build boundaries never run."""
import argparse
from contextlib import ExitStack
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import apk_builder as builder
import apk_package_inputs as consumer
from apk_contract import Refusal, RM2_SDK_SHA, SDK_REPOSITORY
from apk_source_build import sha
import test_apk_build_inputs as fixtures


class PackageInputs(unittest.TestCase):
    def fixture(self, root):
        helper = fixtures.BuildInputs()
        proof = dict(root=str(root / 'sdk'), location_identity=[(1, 2, 16877, 10), (1, 3, 16877, 20)],
            manifest_sha256='a' * 64,
            snapshot=dict(source=dict(repository=SDK_REPOSITORY, tag='v0.2.0', sha=RM2_SDK_SHA, epoch=1),
                tag_oid='f' * 40, conversion=dict(autocrlf='false', eol='native', resolved_eol='lf'),
                working=['6' * 40], committed=['6' * 40], file_identity='7' * 64),
            remote_tag={'refs/tags/v0.2.0': 'f' * 40, 'refs/tags/v0.2.0^{}': RM2_SDK_SHA})
        with patch.object(helper, 'sdk_proof', return_value=proof):
            built, receipt = helper.emit(root)
        source = built.source
        source.mkdir()
        (source / 'LICENSE').write_bytes(b'synthetic GPL license fixture\n')
        tool = root / 'tools/apk'
        tool.parent.mkdir()
        tool.write_bytes(b'synthetic tool; never executed')
        tool.chmod(0o755)
        bundle = root / 'bundle.tar.gz'
        bundle.write_bytes(b'synthetic bundle; never extracted')
        args = argparse.Namespace(build_inputs=built.output,
            expected_receipt_sha256=sha(built.output / 'build-input-receipt.json'),
            build_producer_source=root / 'original-producer', build_producer_sha=built.producer_sha,
            executor_sha='8' * 40, source=source, source_sha=built.source_sha,
            tag=built.tag, sdk_source=built.sdk_source, pkgrel=0, apk_tool=tool,
            rm2_native_bundle=bundle, work_dir=root / 'package-work', output=root / 'packages')
        snapshot = dict(source=receipt['source'], working=['same-blob'], committed=['same-blob'],
                        tag_oid='f' * 40, file_identity='7' * 64)
        for arch in ('armv7', 'aarch64'):
            path = built.output / arch / 'private-evidence.json'
            evidence = json.loads(path.read_text())
            evidence.update(source_root=str(source.resolve()), source_license='GPL-3.0-only',
                source_git_admission=dict(tag_oid=snapshot['tag_oid'], tracked_files=1,
                    file_identity_sha256=snapshot['file_identity'], raw_committed_match=True,
                    working_inventory_sha256=hashlib.sha256(b'same-blob').hexdigest(),
                    committed_inventory_sha256=hashlib.sha256(b'same-blob').hexdigest()))
            path.write_bytes(fixtures.inputs.encoded(evidence))
            receipt['files'][f'{arch}/private-evidence.json'] = dict(bytes=path.stat().st_size, sha256=sha(path))
        (built.output / 'build-input-receipt.json').write_bytes(fixtures.inputs.encoded(receipt))
        args.expected_receipt_sha256 = sha(built.output / 'build-input-receipt.json')
        return args, receipt, proof, snapshot

    def controls(self, args, proof, snapshot, phase=None):
        stack = ExitStack()
        self.identities = stack.enter_context(patch('apk_package_inputs.producer_identity'))
        stack.enter_context(patch('apk_package_inputs.source_snapshot', return_value=snapshot))
        stack.enter_context(patch('apk_package_inputs.source_license', return_value='GPL-3.0-only'))
        stack.enter_context(patch('apk_package_inputs.sdk_release_source', return_value=proof))
        stack.enter_context(patch('apk_package_inputs.RM2_BUNDLE_SHA', sha(args.rm2_native_bundle)))
        stack.enter_context(patch('apk_builder.TOOL_SHA', sha(args.apk_tool)))
        stack.enter_context(patch('apk_package_inputs.run', return_value=builder.IMAGE))
        self.builds = [stack.enter_context(patch(name, side_effect=AssertionError('unexpected build')))
                       for name in ('apk_builder.build', 'apk_build_inputs.build', 'apk_source_build.build')]
        self.phase = stack.enter_context(patch('apk_builder.phase', side_effect=phase))
        return stack

    def phase_control(self, args, mutation=None):
        calls = []
        def phase(argv, cwd, command, **kwargs):
            calls.append(argv)
            self.assertEqual(kwargs['timeout'], 120)
            self.assertIn('--read-only', argv)
            self.assertIn('none', argv)
            if len(calls) == 1:
                self.assertEqual(kwargs['private_diagnostics'], args.work_dir / 'generation-diagnostics')
                self.assertEqual(argv[-1], '/modules/apk_generate.py')
                self.assertIn('1536m', argv)
                artifacts = args.work_dir / 'artifacts'
                names = [f'remarkable-buddies-{args.tag[1:]}-r0.{arch}.apk' for arch in ('armv7', 'aarch64')]
                records = {}
                for name in names + ['buddy-index.adb', 'fixture.pub']:
                    p = artifacts / name
                    p.write_bytes(('synthetic artifact ' + name).encode())
                    records[name] = dict(bytes=p.stat().st_size, sha256=sha(p))
                result = dict(status='generation-self-checks-only', eligibility='not-qualified',
                              private_key_exported=False, fixture_public_key_spki_sha256='a' * 64,
                              artifact_files=records)
            else:
                self.assertEqual(kwargs['private_diagnostics'], args.work_dir / 'observation-diagnostics')
                self.assertEqual(argv[-3:], ['/modules/apk_observe.py', '--expected-spki', 'a' * 64])
                self.assertIn('768m', argv)
                self.assertFalse(any('target=/output' in part for part in argv))
                self.assertTrue(any('target=/artifacts,readonly' in part for part in argv))
                result = dict(status='public-key-only-observed', private_key_available=False)
            if mutation is not None:
                mutation(len(calls))
            return json.dumps(result)
        return phase, calls

    def test_positive_reuses_exact_bytes_and_separates_provenance_and_phases(self):
        with tempfile.TemporaryDirectory() as directory:
            args, receipt, proof, snapshot = self.fixture(Path(directory))
            phase, calls = self.phase_control(args)
            with self.controls(args, proof, snapshot, phase):
                result = consumer.produce(args)
            self.assertEqual(len(calls), 2)
            self.assertEqual(result['eligibility'], 'not-qualified')
            provenance = dict(result['build_input_provenance'])
            correspondence = provenance.pop('sdk_proof_correspondence')
            self.assertEqual(correspondence['classification'], 'historical-proof-equal')
            self.assertEqual(correspondence['differences'], [])
            self.assertFalse(correspondence['continuous_custody_claimed'])
            private_path = args.work_dir / correspondence['private_evidence_name']
            self.assertEqual(sha(private_path), correspondence['private_evidence_sha256'])
            self.assertEqual(private_path.stat().st_size, correspondence['private_evidence_bytes'])
            retained = json.loads(private_path.read_text())
            self.assertEqual(retained['historical'], consumer.normalized(proof))
            self.assertEqual(retained['admitted_current'], consumer.normalized(proof))
            self.assertEqual(provenance, dict(
                receipt_sha256=args.expected_receipt_sha256,
                original_producer_commit=args.build_producer_sha, packaging_executor_commit=args.executor_sha))
            for arch in ('armv7', 'aarch64'):
                for name in ('reader-buddy', 'build.json'):
                    self.assertEqual((args.work_dir / 'inputs' / arch / name).read_bytes(),
                                     (args.build_inputs / arch / name).read_bytes())
            self.assertTrue((args.output / 'test-key-build-receipt.json').is_file())
            self.assertFalse(list(args.work_dir.rglob('*.pem')))
            for build in self.builds:
                build.assert_not_called()
            self.assertIn(((args.build_producer_source.resolve(), args.build_producer_sha), {}),
                          [(call.args, call.kwargs) for call in self.identities.call_args_list])
            self.assertTrue(any(call.args[1] == args.executor_sha and
                                call.args[0] != args.build_producer_source.resolve()
                                for call in self.identities.call_args_list))

    def test_last_copy_mutation_refuses_exclusive_completion(self):
        with tempfile.TemporaryDirectory() as directory:
            args, receipt, proof, snapshot = self.fixture(Path(directory))
            phase, _ = self.phase_control(args)
            original_copy = builder.copy_verified
            def copy_then_change(source, destination, size, digest):
                original_copy(source, destination, size, digest)
                if destination.parent == args.output:
                    (args.build_inputs / 'armv7/reader-buddy').write_bytes(b'changed during final copy')
            with self.controls(args, proof, snapshot, phase), \
                 patch('apk_builder.copy_verified', side_effect=copy_then_change), self.assertRaises(Refusal):
                consumer.produce(args)
            self.assertTrue(args.output.exists())
            self.assertFalse((args.output / 'test-key-build-receipt.json').exists())

    def test_historical_git_ctime_only_records_complete_correspondence_without_rebuild(self):
        with tempfile.TemporaryDirectory() as directory:
            args, receipt, historical, snapshot = self.fixture(Path(directory))
            originals = {p: p.read_bytes() for p in args.build_inputs.rglob('*') if p.is_file()}
            current = consumer.normalized(historical)
            current['location_identity'][1][3] += 1
            phase, calls = self.phase_control(args)
            with self.controls(args, current, snapshot, phase):
                result = consumer.produce(args)
            self.assertEqual(len(calls), 2)
            bound = result['build_input_provenance']['sdk_proof_correspondence']
            record_path = args.work_dir / bound['private_evidence_name']
            record = json.loads(record_path.read_text())
            self.assertEqual(record['historical'], consumer.normalized(historical))
            self.assertEqual(record['admitted_current'], current)
            self.assertEqual(record['classification'], 'historical-git-ctime-only-revalidated')
            self.assertFalse(record['continuous_custody_claimed'])
            self.assertEqual(record['differences'], [dict(path='$["location_identity"][1][3]',
                components=['location_identity', 1, 3], kind='value', historical=20, current=21)])
            self.assertEqual(bound['classification'], record['classification'])
            self.assertEqual(bound['differences'], record['differences'])
            self.assertEqual(bound['private_evidence_sha256'], sha(record_path))
            self.assertEqual(bound['private_evidence_bytes'], record_path.stat().st_size)
            self.assertFalse(bound['continuous_custody_claimed'])
            self.assertEqual({p.name for p in args.output.iterdir()}, set(result['artifacts']) | {'test-key-build-receipt.json'})
            for path, raw in originals.items(): self.assertEqual(path.read_bytes(), raw)
            for build in self.builds: build.assert_not_called()

    def test_every_other_historical_identity_class_refuses_before_generation(self):
        def set_value(value, path, replacement):
            for key in path[:-1]: value = value[key]
            value[path[-1]] = replacement
        cases = [
            ('root-path', ['root'], '/different-sdk'),
            ('manifest', ['manifest_sha256'], 'b' * 64),
            ('repository', ['snapshot', 'source', 'repository'], 'https://example.invalid/sdk'),
            ('tag', ['snapshot', 'source', 'tag'], 'v9.9.9'),
            ('commit', ['snapshot', 'source', 'sha'], '0' * 40),
            ('epoch', ['snapshot', 'source', 'epoch'], 2),
            ('epoch-bool-alias', ['snapshot', 'source', 'epoch'], True),
            ('local-tag', ['snapshot', 'tag_oid'], '0' * 40),
            ('remote-tag', ['remote_tag', 'refs/tags/v0.2.0'], '0' * 40),
            ('remote-peel', ['remote_tag', 'refs/tags/v0.2.0^{}'], '0' * 40),
            ('raw-working', ['snapshot', 'working', 0], '0' * 40),
            ('committed', ['snapshot', 'committed', 0], '0' * 40),
            ('file-identity', ['snapshot', 'file_identity'], '0' * 64),
            ('autocrlf', ['snapshot', 'conversion', 'autocrlf'], 'true'),
            ('eol', ['snapshot', 'conversion', 'eol'], 'crlf'),
            ('resolved-eol', ['snapshot', 'conversion', 'resolved_eol'], 'crlf'),
        ]
        for row, column, name in ((0, 0, 'root-dev'), (0, 1, 'root-inode'), (0, 2, 'root-mode'),
                                  (0, 3, 'root-ctime'), (1, 0, 'git-dev'), (1, 1, 'git-inode'), (1, 2, 'git-mode')):
            old = [[1, 2, 16877, 10], [1, 3, 16877, 20]][row][column]
            cases.append((name, ['location_identity', row, column], old + 1))
        for value in (-1, True, 21.0, '21', None):
            cases.append(('invalid-ctime-' + repr(value), ['location_identity', 1, 3], value))
        actions = [(name, lambda value, path=path, replacement=replacement: set_value(value, path, replacement))
                   for name, path, replacement in cases]
        actions.extend([
            ('missing-root-field', lambda value: value.pop('manifest_sha256')),
            ('extra-root-field', lambda value: value.update(extra=True)),
            ('missing-nested-field', lambda value: value['snapshot'].pop('file_identity')),
            ('extra-nested-field', lambda value: value['snapshot'].update(extra=True)),
            ('path-string-alias', lambda value: value.update({'location_identity[1][3]': 21})),
            ('missing-remote-key', lambda value: value['remote_tag'].pop('refs/tags/v0.2.0^{}')),
            ('extra-remote-key', lambda value: value['remote_tag'].update(extra='0' * 40)),
            ('location-list-length', lambda value: value['location_identity'].pop()),
            ('location-record-length', lambda value: value['location_identity'][1].append(0)),
            ('location-list-type', lambda value: value.update(location_identity={})),
            ('raw-list-length', lambda value: value['snapshot']['working'].append('0' * 40)),
            ('ctime-and-second-difference', lambda value: (set_value(value, ['location_identity', 1, 3], 21),
                                                          value.update(manifest_sha256='b' * 64))),
        ])
        for name, change in actions:
            with self.subTest(case=name), tempfile.TemporaryDirectory() as directory:
                args, receipt, historical, snapshot = self.fixture(Path(directory))
                current = consumer.normalized(historical)
                change(current)
                with self.controls(args, current, snapshot), self.assertRaisesRegex(Refusal, 'SDK proof correspondence'):
                    consumer.produce(args)
                self.phase.assert_not_called()
                self.assertFalse(args.work_dir.exists())
                self.assertFalse(args.output.exists())

    def test_current_git_ctime_is_strict_at_every_later_boundary(self):
        for boundary, expected_phases in ((2, 0), (3, 1), (4, 2), (5, 2)):
            with self.subTest(observation=boundary), tempfile.TemporaryDirectory() as directory:
                args, receipt, historical, snapshot = self.fixture(Path(directory))
                current = consumer.normalized(historical)
                current['location_identity'][1][3] = 21
                observations = []
                def observe(*values):
                    observations.append(values)
                    result = copy.deepcopy(current)
                    if len(observations) >= boundary: result['location_identity'][1][3] = 22
                    return result
                phase, calls = self.phase_control(args)
                with self.controls(args, current, snapshot, phase), \
                     patch('apk_package_inputs.sdk_release_source', side_effect=observe), \
                     self.assertRaisesRegex(Refusal, 'independent SDK proof changed'):
                    consumer.produce(args)
                self.assertEqual(len(calls), expected_phases)
                self.assertEqual(len(observations), boundary)
                self.assertFalse((args.output / 'test-key-build-receipt.json').exists())
                record = json.loads((args.work_dir / 'sdk-proof-correspondence.json').read_text())
                self.assertEqual(record['admitted_current']['location_identity'][1][3], 21)
                for build in self.builds: build.assert_not_called()

    def test_correspondence_record_change_refuses_before_observation(self):
        with tempfile.TemporaryDirectory() as directory:
            args, receipt, proof, snapshot = self.fixture(Path(directory))
            def mutate(count):
                if count == 1: (args.work_dir / 'sdk-proof-correspondence.json').write_bytes(b'{}')
            phase, calls = self.phase_control(args, mutate)
            with self.controls(args, proof, snapshot, phase), self.assertRaisesRegex(Refusal, 'SDK correspondence evidence changed'):
                consumer.produce(args)
            self.assertEqual(len(calls), 1)
            self.assertFalse((args.output / 'test-key-build-receipt.json').exists())

    def test_retained_target_proof_type_alias_or_extra_field_refuses(self):
        for case in ('bool', 'float', 'extra-field'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                args, receipt, proof, snapshot = self.fixture(Path(directory))
                path = args.build_inputs / 'aarch64/private-evidence.json'
                record = json.loads(path.read_text())
                if case == 'extra-field': record['sdk_release_source']['extra'] = True
                else: record['sdk_release_source']['snapshot']['source']['epoch'] = True if case == 'bool' else 1.0
                path.write_bytes(fixtures.inputs.encoded(record))
                receipt['files']['aarch64/private-evidence.json'] = dict(bytes=path.stat().st_size, sha256=sha(path))
                marker = args.build_inputs / 'build-input-receipt.json'
                marker.write_bytes(fixtures.inputs.encoded(receipt))
                args.expected_receipt_sha256 = sha(marker)
                with self.controls(args, proof, snapshot), self.assertRaises(Refusal):
                    consumer.produce(args)
                self.phase.assert_not_called()
                self.assertFalse(args.work_dir.exists())

    def test_admission_refuses_tampering_before_any_phase(self):
        for case in ('pin', 'binary', 'metadata', 'evidence', 'extra', 'missing', 'noncanonical',
                     'producer', 'source', 'snapshot', 'source-proof', 'source-license',
                     'proof', 'tool', 'bundle', 'image', 'executor'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                args, receipt, proof, snapshot = self.fixture(Path(directory))
                with self.controls(args, proof, snapshot):
                    if case == 'pin': args.expected_receipt_sha256 = '0' * 64
                    elif case in ('binary', 'metadata', 'evidence'):
                        name = {'binary': 'reader-buddy', 'metadata': 'build.json', 'evidence': 'private-evidence.json'}[case]
                        (args.build_inputs / 'armv7' / name).write_bytes(b'changed')
                    elif case == 'extra': (args.build_inputs / 'extra').write_bytes(b'x')
                    elif case == 'missing': (args.build_inputs / 'build-input-receipt.json').unlink()
                    elif case == 'noncanonical':
                        p = args.build_inputs / 'build-input-receipt.json'
                        p.write_text(json.dumps(receipt, indent=2))
                        args.expected_receipt_sha256 = sha(p)
                    elif case == 'producer': args.build_producer_sha = '0' * 40
                    elif case == 'source': args.source_sha = '0' * 40
                    elif case == 'snapshot': snapshot['working'] = ['different']
                    elif case == 'source-proof': snapshot['file_identity'] = '0' * 64
                    elif case == 'source-license':
                        import apk_package_inputs
                        apk_package_inputs.source_license.return_value = 'GPL-3.0'
                    elif case == 'proof': proof['location_identity'] = [(4, 3, 2, 1)]
                    elif case == 'tool': args.apk_tool.write_bytes(b'changed')
                    elif case == 'bundle': args.rm2_native_bundle.write_bytes(b'changed')
                    elif case == 'image':
                        import apk_package_inputs
                        apk_package_inputs.run.return_value = 'wrong image'
                    elif case == 'executor':
                        import apk_package_inputs
                        apk_package_inputs.producer_identity.side_effect = Refusal('executor refused')
                    with self.assertRaises((Refusal, FileNotFoundError)):
                        consumer.produce(args)
                    self.phase.assert_not_called()
                    self.assertFalse(args.output.exists())

    def test_output_overlap_existing_and_symlink_refuse(self):
        for case in ('equal', 'nested', 'source', 'sdk', 'retained', 'existing', 'symlink'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                args, receipt, proof, snapshot = self.fixture(root)
                if case == 'equal': args.output = args.work_dir
                elif case == 'nested': args.output = args.work_dir / 'inside'
                elif case == 'source': args.output = args.source / 'inside'
                elif case == 'sdk': args.work_dir = args.sdk_source / 'inside'
                elif case == 'retained': args.output = args.build_inputs / 'inside'
                elif case == 'existing': args.output.mkdir()
                elif case == 'symlink':
                    link = root / 'linked-inputs'
                    try: link.symlink_to(args.build_inputs, target_is_directory=True)
                    except OSError: continue
                    args.build_inputs = link
                with self.controls(args, proof, snapshot), self.assertRaises(Refusal):
                    consumer.produce(args)
                self.phase.assert_not_called()

    def test_phase_failure_or_late_mutation_never_completes(self):
        for case in ('generation', 'between-phases', 'observation', 'binary', 'proof', 'source', 'tool', 'bundle',
                     'staged', 'module', 'extra-dir', 'receipt', 'artifact', 'private-key'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                args, receipt, proof, snapshot = self.fixture(Path(directory))
                def mutate(count):
                    if case == 'generation' and count == 1: raise Refusal('generation failed')
                    if case == 'between-phases' and count == 1:
                        (args.work_dir / 'modules/apk_generate.py').write_bytes(b'changed')
                    if count != 2: return
                    if case == 'observation': raise Refusal('observation failed')
                    elif case == 'binary': (args.build_inputs / 'armv7/reader-buddy').write_bytes(b'changed')
                    elif case == 'proof': proof['new-field'] = 'must not be dropped'
                    elif case == 'source': snapshot['new-field'] = 'changed'
                    elif case == 'tool': args.apk_tool.write_bytes(b'changed')
                    elif case == 'bundle': args.rm2_native_bundle.write_bytes(b'changed')
                    elif case == 'staged': (args.work_dir / 'inputs/armv7/build.json').write_bytes(b'changed')
                    elif case == 'module': (args.work_dir / 'modules/apk_generate.py').write_bytes(b'changed')
                    elif case == 'extra-dir': (args.work_dir / 'inputs/extra-directory').mkdir()
                    elif case == 'receipt': (args.build_inputs / 'build-input-receipt.json').write_bytes(b'{}')
                    elif case == 'artifact': (args.work_dir / 'artifacts/fixture.pub').write_bytes(b'changed')
                phase, calls = self.phase_control(args, mutate)
                with self.controls(args, proof, copy.deepcopy(snapshot), phase):
                    if case == 'private-key':
                        def bad_observation(*values, **kwargs):
                            value = json.loads(phase(*values, **kwargs))
                            if value['status'] == 'public-key-only-observed':
                                value['private_key_available'] = True
                            return json.dumps(value)
                        self.phase.side_effect = bad_observation
                    # Use the same snapshot object returned at every boundary.
                    if case == 'source':
                        import apk_package_inputs
                        apk_package_inputs.source_snapshot.return_value = snapshot
                        def source_change(count):
                            if count == 2: apk_package_inputs.source_snapshot.return_value = dict(snapshot, changed=True)
                        phase, calls = self.phase_control(args, source_change)
                        self.phase.side_effect = phase
                    with self.assertRaises((Refusal, FileNotFoundError)):
                        consumer.produce(args)
                    self.assertFalse((args.output / 'test-key-build-receipt.json').exists())
                    self.assertTrue(args.work_dir.exists())
                    if case in ('generation', 'between-phases'):
                        self.assertEqual(len(calls), 1)
                    for build in self.builds: build.assert_not_called()

    def test_original_cli_still_builds_both_targets_then_uses_shared_tail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args, receipt, proof, snapshot = self.fixture(root)
            old = argparse.Namespace(source=args.source, target_dir=root / 'old-cache', output=root / 'old-output',
                tag=args.tag, pkgrel=0, sdk_tag=None, sdk_tag_source=None,
                apk_tool=args.apk_tool, rm2_native_bundle=args.rm2_native_bundle)
            fake_build = fixtures.BuildInputs().build_fixture(root)
            def original_build(source, tag, target, cache, configuration, sdk_tag, **kwargs):
                return fake_build(source, tag, target, cache, configuration, **kwargs)
            with patch('apk_builder.source_identity', return_value=receipt['source']), \
                 patch('apk_builder.source_license', return_value='GPL-3.0-only'), \
                 patch('apk_builder.RM2_BUNDLE_SHA', sha(args.rm2_native_bundle)), \
                 patch('apk_builder.TOOL_SHA', sha(args.apk_tool)), \
                 patch('apk_builder.run', return_value=builder.IMAGE), \
                 patch('apk_builder.build', side_effect=original_build) as build, \
                 patch('apk_builder.generate_and_observe', return_value='shared tail') as shared:
                self.assertEqual(builder.produce(old), 'shared tail')
            self.assertEqual(build.call_count, 2)
            shared.assert_called_once()
            self.assertEqual([call.args[2] for call in build.call_args_list],
                             ['armv7-unknown-linux-gnueabihf', 'aarch64-unknown-linux-gnu'])

    @unittest.skipUnless(os.name == 'posix', 'real POSIX mode admission')
    def test_shared_tool_admission_refuses_both_entry_points_before_image_or_build(self):
        for mode in (0o664, 0o744, 0o701):
            with self.subTest(mode=oct(mode)), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                args, receipt, proof, snapshot = self.fixture(root)
                args.apk_tool.chmod(mode)
                old = argparse.Namespace(source=args.source, target_dir=root/'old-cache', output=root/'old-output',
                    tag=args.tag, pkgrel=0, sdk_tag=None, sdk_tag_source=None,
                    apk_tool=args.apk_tool, rm2_native_bundle=None)
                with self.controls(args, proof, snapshot), \
                     patch('apk_package_inputs.run') as inspect, \
                     self.assertRaisesRegex(Refusal, 'readable executable pinned APK tool'):
                    consumer.produce(args)
                inspect.assert_not_called();self.phase.assert_not_called()
                with patch('apk_builder.TOOL_SHA', sha(args.apk_tool)), \
                     patch('apk_builder.source_identity', return_value=receipt['source']), \
                     patch('apk_builder.source_license', return_value='GPL-3.0-only'), \
                     patch('apk_builder.run') as inspect, patch('apk_builder.build') as build, \
                     self.assertRaisesRegex(Refusal, 'readable executable pinned APK tool'):
                    builder.produce(old)
                inspect.assert_not_called();build.assert_not_called()
                self.assertFalse(args.work_dir.exists());self.assertFalse(args.output.exists())
                self.assertEqual(args.apk_tool.stat().st_mode & 0o777, mode)

    @unittest.skipUnless(os.name == 'posix', 'real POSIX equivalent modes')
    def test_equivalent_tool_modes_are_accepted_without_chmod_or_exact_whitelist(self):
        for mode in (0o755, 0o775, 0o555, 0o705, 0o745):
            with self.subTest(mode=oct(mode)), tempfile.TemporaryDirectory() as directory:
                args, receipt, proof, snapshot = self.fixture(Path(directory))
                args.apk_tool.chmod(mode)
                with patch('apk_builder.TOOL_SHA', sha(args.apk_tool)):
                    self.assertEqual(builder.admit_apk_tool(args.apk_tool), args.apk_tool.resolve())
                self.assertEqual(args.apk_tool.stat().st_mode & 0o777, mode)

    @unittest.skipUnless(os.name == 'posix', 'real POSIX later-mode loss')
    def test_tool_mode_loss_after_generation_or_observer_refuses_completion(self):
        for when in (1, 2):
            with self.subTest(phase=when), tempfile.TemporaryDirectory() as directory:
                args, receipt, proof, snapshot = self.fixture(Path(directory))
                def mutate(count):
                    if count == when: args.apk_tool.chmod(0o664)
                phase, calls = self.phase_control(args, mutate)
                with self.controls(args, proof, snapshot, phase), \
                     self.assertRaisesRegex(Refusal, 'readable executable pinned APK tool'):
                    consumer.produce(args)
                self.assertEqual(len(calls), when)
                self.assertFalse((args.output/'test-key-build-receipt.json').exists())

    @unittest.skipUnless(os.name == 'posix', 'real POSIX last-copy mode loss')
    def test_tool_mode_loss_during_final_copies_refuses_completion(self):
        with tempfile.TemporaryDirectory() as directory:
            args, receipt, proof, snapshot = self.fixture(Path(directory))
            phase, calls = self.phase_control(args)
            original_copy = builder.copy_verified
            def copy_then_strip(source, destination, size, digest):
                original_copy(source, destination, size, digest)
                if destination.parent == args.output: args.apk_tool.chmod(0o664)
            with self.controls(args, proof, snapshot, phase), \
                 patch('apk_builder.copy_verified', side_effect=copy_then_strip), \
                 self.assertRaisesRegex(Refusal, 'readable executable pinned APK tool'):
                consumer.produce(args)
            self.assertEqual(len(calls), 2)
            self.assertFalse((args.output/'test-key-build-receipt.json').exists())


if __name__ == '__main__':
    unittest.main()
