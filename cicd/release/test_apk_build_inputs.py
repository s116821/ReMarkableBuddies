"""Synthetic filesystem/completion controls; no compiler, Docker or release call."""
import argparse
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import apk_build_inputs as inputs
from apk_contract import (Refusal, RM2_BINARY, RM2_FEATURE, RM2_PROFILE,
                          RM2_SDK_SHA, SDK_REPOSITORY)
from test_apk_contract import specimen
import apk_source_build

class BuildInputs(unittest.TestCase):
    def sdk_proof(self):
        return {'fixture_only':True,'manifest_sha256':'a'*64,'snapshot':{'tag_oid':'f'*40}}

    def arguments(self, root):
        return argparse.Namespace(source=root/'source', sdk_source=root/'sdk',
            target_dir=root/'cache', output=root/'output', tag='v1.2.3',
            source_sha='1'*40, producer_sha='9'*40)

    def build_fixture(self, root):
        def build(source, tag, target, cache, configuration, **kwargs):
            path = root / ('binary-' + target)
            path.write_bytes(b'synthetic executable control ' + target.encode())
            metadata = specimen()
            metadata['binary'].update(bytes=path.stat().st_size,
                sha256=hashlib.sha256(path.read_bytes()).hexdigest())
            metadata['sdk'] = dict(ref_kind='tagged-git', repository=SDK_REPOSITORY,
                sha=RM2_SDK_SHA, tag='v0.2.0', manifest_sha256='a'*64)
            if kwargs['rm2_resident']:
                metadata['cargo']['features'] = [RM2_FEATURE]
                metadata['build'].update(profile=RM2_PROFILE,
                    argv=['cross','build','--features',RM2_FEATURE])
                metadata['binary']['path'] = RM2_BINARY
            else:
                metadata['build']['profile'] = 'unqualified-development'
                metadata['target'].update(rust_target=target, apk_arch='aarch64',
                    abi='gnu-aarch64', elf_class=64, elf_machine='AArch64',
                    interpreter='/lib/ld-linux-aarch64.so.1')
            return path, metadata, {'fixture_only': True, 'sdk_release_source':self.sdk_proof()}
        return build

    def emit(self, root):
        args = self.arguments(root)
        identity = specimen()['source']
        with patch('apk_build_inputs.producer_identity'), \
             patch('apk_build_inputs.source_snapshot', return_value={'source':identity}), \
             patch('apk_build_inputs.sdk_release_source',return_value=self.sdk_proof()), \
             patch('apk_build_inputs.build', side_effect=self.build_fixture(root)) as build:
            receipt = inputs.produce(args)
        self.assertEqual(build.call_count, 2)
        return args, receipt

    def test_frozen_roundtrip_and_no_reuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args, receipt = self.emit(root)
            self.assertEqual(inputs.verify(args.output,args.tag,args.source_sha,args.producer_sha), receipt)
            self.assertEqual(receipt['eligibility'], 'not-qualified')
            self.assertFalse((args.output/'completion-pending.json').exists())
            with patch('apk_build_inputs.build') as build, self.assertRaisesRegex(Refusal,'fresh output'):
                inputs.produce(args)
            build.assert_not_called()

    def test_tampered_binary_metadata_evidence_inventory_and_identity_refuse(self):
        for case in ['binary','metadata','evidence','extra','missing','source','producer']:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                args, _ = self.emit(Path(directory))
                tag, commit, producer = args.tag, args.source_sha, args.producer_sha
                if case == 'binary': (args.output/'armv7/reader-buddy').write_bytes(b'changed')
                elif case == 'metadata': (args.output/'armv7/build.json').write_bytes(b'{}')
                elif case == 'evidence': (args.output/'aarch64/private-evidence.json').write_bytes(b'{}')
                elif case == 'extra': (args.output/'unexpected').write_bytes(b'x')
                elif case == 'missing': (args.output/inputs.COMPLETION).unlink()
                elif case == 'source': commit = '0'*40
                elif case == 'producer': producer = '0'*40
                with self.assertRaises((Refusal,FileNotFoundError)):
                    inputs.verify(args.output,tag,commit,producer)

    @unittest.skipIf(os.name == 'nt', 'real symlink control requires Linux')
    def test_symlink_payload_refuses_even_when_bytes_match(self):
        with tempfile.TemporaryDirectory() as directory:
            args, _ = self.emit(Path(directory))
            path = args.output/'armv7/reader-buddy'
            held = args.output.parent/'held-binary'
            path.rename(held)
            path.symlink_to(held)
            with self.assertRaisesRegex(Refusal,'regular input'):
                inputs.verify(args.output,args.tag,args.source_sha,args.producer_sha)

    def test_nested_or_source_overlap_refuses_before_source_and_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for relative in ['cache','cache/nested','source/output','sdk/output']:
                args=self.arguments(root)
                args.output=root/relative
                with patch('apk_build_inputs.source_snapshot') as source, \
                     patch('apk_build_inputs.build') as build, self.assertRaises(Refusal):
                    inputs.produce(args)
                source.assert_not_called();build.assert_not_called()
                self.assertFalse(args.target_dir.exists())

    def test_second_build_failure_never_publishes_completion(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);args=self.arguments(root)
            fake=self.build_fixture(root)
            def fail_second(*values,**kwargs):
                if not kwargs['rm2_resident']: raise Refusal('second target failed')
                return fake(*values,**kwargs)
            with patch('apk_build_inputs.producer_identity'), \
                 patch('apk_build_inputs.source_snapshot',return_value={'source':specimen()['source']}), \
                 patch('apk_build_inputs.build',side_effect=fail_second), self.assertRaisesRegex(Refusal,'second target'):
                inputs.produce(args)
            self.assertTrue((args.output/'armv7/build.json').exists())
            self.assertFalse((args.output/inputs.COMPLETION).exists())

    def test_contradictory_metadata_cannot_get_completion(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);args=self.arguments(root)
            fake=self.build_fixture(root)
            def wrong(*values,**kwargs):
                binary,metadata,evidence=fake(*values,**kwargs)
                metadata['sdk']['tag']='v0.1.0'
                return binary,metadata,evidence
            with patch('apk_build_inputs.producer_identity'), \
                 patch('apk_build_inputs.source_snapshot',return_value={'source':specimen()['source']}), \
                 patch('apk_build_inputs.sdk_release_source',return_value=self.sdk_proof()), \
                 patch('apk_build_inputs.build',side_effect=wrong), self.assertRaisesRegex(Refusal,'SDK correspondence'):
                inputs.produce(args)
            self.assertFalse((args.output/inputs.COMPLETION).exists())

    def test_sdk_proof_change_between_targets_or_before_completion_refuses(self):
        for phase in ['between-targets','before-completion']:
            with self.subTest(phase=phase),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);args=self.arguments(root);fake=self.build_fixture(root)
                changed=self.sdk_proof();changed['snapshot']['tag_oid']='e'*40
                def build(*values,**kwargs):
                    binary,metadata,evidence=fake(*values,**kwargs)
                    if phase=='between-targets' and not kwargs['rm2_resident']:
                        evidence['sdk_release_source']=changed
                    return binary,metadata,evidence
                with patch('apk_build_inputs.producer_identity'), \
                     patch('apk_build_inputs.source_snapshot',return_value={'source':specimen()['source']}), \
                     patch('apk_build_inputs.sdk_release_source',return_value=changed), \
                     patch('apk_build_inputs.build',side_effect=build), self.assertRaisesRegex(Refusal,'SDK release source changed'):
                    inputs.produce(args)
                self.assertFalse((args.output/inputs.COMPLETION).exists())

class MaintainedCrossMapping(unittest.TestCase):
    @unittest.skipIf(os.name == 'nt', 'Linux mapping admission control')
    def test_supported_mapping_and_rootless_or_unknown_revision_refusal(self):
        root = Path('/normally-owned/source')
        for revision in ['49cd054', '8c1a8aa', 'unknown']:
            calls = [f'cross 0.2.5 ({revision} 2026-01-01)', 'unix:///var/run/docker.sock', '[name=seccomp]']
            with patch('apk_source_build.run',side_effect=calls):
                if revision == 'unknown':
                    with self.assertRaisesRegex(Refusal,'unreviewed Cross'):
                        apk_source_build.cross_source_mapping(root,Path('/cross'))
                else:
                    mount,user = apk_source_build.cross_source_mapping(root,Path('/cross'))
                    self.assertEqual(mount,str(root))
                    self.assertEqual(user,f'{os.getuid()}:{os.getgid()}')
        calls = ['cross 0.2.5 (8c1a8aa 2026-01-01)', 'unix:///var/run/docker.sock', '[name=rootless]']
        with patch('apk_source_build.run',side_effect=calls), self.assertRaisesRegex(Refusal,'user mapping'):
            apk_source_build.cross_source_mapping(root,Path('/cross'))


if __name__ == '__main__': unittest.main()
