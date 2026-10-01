"""Actual GitVersion regression: ancestry must win over equal commit timestamps."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class EqualDateVersioningTests(unittest.TestCase):
    def test_same_second_fix_and_earlier_detached_job_use_only_their_ancestry(self):
        executable = os.environ.get('GITVERSION') or shutil.which('dotnet-gitversion')
        if not executable and os.environ.get('GITVERSION_PATH'):
            executable = str(Path(os.environ['GITVERSION_PATH']) / ('dotnet-gitversion.exe' if os.name == 'nt' else 'dotnet-gitversion'))
        self.assertTrue(executable, 'Install GitVersion 6.8.2 or set GITVERSION')
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as temp:
            repo = Path(temp) / 'repository'
            repo.mkdir()
            env = {k: v for k, v in os.environ.items() if not k.startswith(('GITHUB_', 'GITVERSION_', 'GitVersion_'))}
            env.update(GIT_AUTHOR_DATE='2026-01-01T00:00:00Z', GIT_COMMITTER_DATE='2026-01-01T00:00:00Z')
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, env=env, text=True).strip()
            git('init', '-b', 'main')
            git('config', 'user.name', 'Offline release fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            git('config', 'core.autocrlf', 'false')
            for index, message in enumerate(('chore(REM-46): baseline', 'feat(REM-46): feature', 'fix(REM-46): repair')):
                (repo / 'app.txt').write_text(message)
                git('add', '.')
                git('commit', '-m', message)
                if index == 0:
                    git('tag', '-a', 'v0.1.17', '-m', 'fixture')
            def version(expected):
                output = Path(temp) / 'version.json'
                context = dict(env, GITHUB_ACTIONS='true', GITHUB_REF='refs/heads/main', GITHUB_SHA=git('rev-parse', 'HEAD'))
                result = subprocess.run([executable, str(repo), '/config', str(ROOT / 'GitVersion.yml'), '/output', 'file', '/outputfile', str(output), '/nocache', '/nonormalize'], env=context, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                data = json.loads(output.read_text())
                self.assertEqual(data['Sha'], git('rev-parse', 'HEAD'))
                self.assertEqual(data['MajorMinorPatch'], expected, json.dumps(data, indent=2))
            with self.subTest(context='same-second feature then fix'):
                version('0.2.1')
            git('tag', '-a', 'v0.2.1', '-m', 'later job finished first')
            git('checkout', '--detach', 'HEAD~1')
            with self.subTest(context='earlier job after descendant tag exists'):
                version('0.2.0')
