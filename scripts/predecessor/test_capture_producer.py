"""Reject command provenance that could credit a test harness or another build."""
import copy
import json
from pathlib import Path
import unittest

from capture_producer import select_command, verify_build_identity


class CommandArtifactTests(unittest.TestCase):
    def setUp(self):
        self.workspace = Path(__file__).resolve().parents[2]
        self.artifact = {
            'reason': 'compiler-artifact', 'package_id': 'reader-buddy fixture',
            'target': {'name': 'reader-buddy', 'kind': ['bin'],
                       'src_path': str(self.workspace / 'src/main.rs')},
            'profile': {'test': False}, 'features': ['default', 'notebook'],
            'executable': str(self.workspace / 'target/debug/reader-buddy'),
        }

    def select(self, *records):
        return select_command('\n'.join(json.dumps(r) for r in records), self.workspace)

    def test_accepts_normal_command_and_duplicate_cargo_observations(self):
        self.assertEqual(self.select(self.artifact, self.artifact)['profile']['test'], False)

    def test_refuses_test_harness_wrong_source_and_wrong_executable(self):
        for field in ['test', 'source', 'executable']:
            with self.subTest(field=field):
                value = copy.deepcopy(self.artifact)
                if field == 'test':
                    value['profile']['test'] = True
                elif field == 'source':
                    value['target']['src_path'] = str(self.workspace / 'docs/examples/other.rs')
                else:
                    value['executable'] += '-other'
                with self.assertRaises(ValueError):
                    self.select(value)

    def test_refuses_ambiguous_feature_provenance(self):
        other = copy.deepcopy(self.artifact)
        other['features'] = ['default']
        with self.assertRaises(ValueError):
            self.select(self.artifact, other)

    def test_non_artifact_output_cannot_supply_provenance(self):
        with self.assertRaises(ValueError):
            select_command('test result: ok\n{}\nnull', self.workspace)

    def test_requires_features_and_exact_commit_and_tree(self):
        verify_build_identity('merge', 'merge', 'tree', 'tree', ['feature'], ['feature'])
        cases = [
            ('other', 'merge', 'tree', 'tree', ['feature'], ['feature']),
            ('merge', 'merge', 'tree', 'other-tree', ['feature'], ['feature']),
            ('merge', 'merge', 'tree', 'tree', ['feature'], []),
        ]
        for arguments in cases:
            with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                verify_build_identity(*arguments)


if __name__ == '__main__':
    unittest.main()
