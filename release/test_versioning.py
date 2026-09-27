"""Run the real GitVersion distribution against isolated, offline Git history."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from build import git

ROOT = Path(__file__).resolve().parents[1]


class VersioningTests(unittest.TestCase):
    def test_semantics_paths_reverse_queue_and_github_detached_context(self):
        executable = os.environ.get('GITVERSION') or shutil.which('dotnet-gitversion')
        if not executable and os.environ.get('GITVERSION_PATH'):
            executable = str(Path(os.environ['GITVERSION_PATH']) / ('dotnet-gitversion.exe' if os.name == 'nt' else 'dotnet-gitversion'))
        self.assertTrue(executable, 'Install GitVersion 6.8.2 or set GITVERSION to its executable')
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as temp:
            repo = Path(temp) / 'repository'
            repo.mkdir()
            git(repo, 'init', '-b', 'main')
            git(repo, 'config', 'user.name', 'Offline release fixture')
            git(repo, 'config', 'user.email', 'fixture@example.invalid')
            git(repo, 'config', 'core.autocrlf', 'false')

            def commit(message, path='src/app.txt', content=None):
                file = repo / path
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text(content or message, encoding='utf-8')
                git(repo, 'add', '.')
                git(repo, 'commit', '-m', message)
                return git(repo, 'rev-parse', 'HEAD')

            def version(expected, *, github=False):
                env = {k: v for k, v in os.environ.items() if not k.startswith(('GITHUB_', 'GITVERSION_', 'GitVersion_'))}
                if github:
                    env.update(GITHUB_ACTIONS='true', GITHUB_REF='refs/heads/main', GITHUB_SHA=git(repo, 'rev-parse', 'HEAD'))
                output = Path(temp) / 'version.json'
                result = subprocess.run([executable, str(repo), '/config', str(ROOT / 'GitVersion.yml'), '/output', 'file', '/outputfile', str(output), '/nocache', '/nonormalize'], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                data = json.loads(output.read_text())
                self.assertEqual(data['MajorMinorPatch'], expected)
                return data

            commit('chore(REM-46): baseline')
            git(repo, 'tag', '-a', 'v0.1.17', '-m', 'fixture')
            version('0.1.17')
            feature = commit('feat(REM-46): feature')
            version('0.2.0')
            fix = commit('fix(REM-46): repair')
            version('0.2.1')
            git(repo, 'tag', '-a', 'v0.2.1', '-m', 'later job finished first')
            git(repo, 'checkout', '--detach', feature)
            self.assertEqual(version('0.2.0', github=True)['Sha'], feature)
            git(repo, 'tag', '-a', 'v0.2.0', '-m', 'earlier job recovered')
            git(repo, 'checkout', 'main')
            self.assertEqual(version('0.2.1', github=True)['Sha'], fix)
            for path in ('README.md', 'docs/guide.md', 'docs/deep/guide.md', '.agents/skills/tool/SKILL.md', 'docs/images/proof.png', 'docs/images/deep/diagram.svg', 'docs/licenses/third-party.txt', 'openspec/specs/example/spec.md'):
                commit('docs(REM-46): documentation', path)
                version('0.2.1')
            commit('feat(REM-46): still only documentation', 'docs/guide.md')
            version('0.2.1')
            commit('fix(REM-46): body example\n\nfeat(REM-99): example, not the subject')
            version('0.2.2')
            commit('test(REM-46): executable fixture', 'docs/fixture.json')
            version('0.2.3')
            # A code-to-doc rename is still a code removal, not pure docs.
            (repo / 'docs/renamed.md').parent.mkdir(exist_ok=True)
            git(repo, 'mv', 'src/app.txt', 'docs/renamed.md')
            git(repo, 'commit', '-m', 'refactor(REM-46): rename code')
            version('0.2.4')
            for patch, path in enumerate(('docs/images/helper.py', 'docs/validation/images/data.json', 'docs/licenses/tool.sh'), start=5):
                commit('test(REM-46): executable evidence fixture', path)
                version(f'0.2.{patch}')
            commit('fix(REM-46)!: breaking header')
            version('1.0.0')
            commit('refactor(REM-46): breaking footer\n\nBREAKING CHANGE: incompatible API')
            version('2.0.0')
            previous = (repo / 'src/app.txt').read_text()
            commit('fix(REM-46): temporary change')
            commit('revert(REM-46): undo temporary change', content=previous)
            version('2.0.2')


if __name__ == '__main__':
    unittest.main()
