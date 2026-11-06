"""Owned synthetic receipt controls; Docker/signing/build boundaries never run."""
import argparse
from contextlib import ExitStack
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import apk_builder as builder
import apk_package_inputs as consumer
from apk_contract import Refusal
from apk_source_build import sha
import test_apk_build_inputs as fixtures


class PackageInputs(unittest.TestCase):
    def fixture(self, root):
        helper = fixtures.BuildInputs()
        proof = helper.sdk_proof()
        proof['location_identity'] = [(1, 2, 3, 4)]
        with patch.object(helper, 'sdk_proof', return_value=proof):
            built, receipt = helper.emit(root)
        source = built.source
        source.mkdir()
        (source / 'LICENSE').write_bytes(b'synthetic GPL license fixture\n')
        tool = root / 'tools/apk'
        tool.parent.mkdir()
        tool.write_bytes(b'synthetic tool; never executed')
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
        stack.enter_context(patch('apk_package_inputs.TOOL_SHA', sha(args.apk_tool)))
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
            self.assertEqual(result['build_input_provenance'], dict(
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


if __name__ == '__main__':
    unittest.main()
