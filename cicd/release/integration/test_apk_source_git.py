"""Real Git source admission with a disposable owned repository."""
import os,pathlib,shutil,unittest,tempfile
from unittest.mock import patch
from apk_contract import Refusal
from apk_source_build import run,source_snapshot,SOURCE_PROBE

class GitSourceAdmission(unittest.TestCase):
 def test_actual_git_raw_bytes_index_mode_and_dirty_status(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory).resolve()
   repo=root/'source';repo.mkdir()
   profile=root/'home';profile.mkdir()
   # This synthetic repository belongs to a fresh user profile, independent of
   # CI checkout trust settings. System Git and the production checks still run.
   with patch.dict(os.environ,HOME=str(profile),XDG_CONFIG_HOME=str(profile/'.config')):

    def git(*args,env=None):return run(['git',*args],repo,env=env)
    git('init','--quiet')
    git('remote','add','origin','https://github.com/s116821/ReMarkableBuddies')
    attributes=repo/'.gitattributes';attributes.write_bytes(b'input.txt -text\n')
    source=repo/'input.txt';source.write_bytes(b'original\n')
    git('add','.');git('commit','--quiet','-m','fixture',env=dict(os.environ,GIT_AUTHOR_NAME='Fixture',
       GIT_COMMITTER_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.com',GIT_COMMITTER_EMAIL='fixture@example.com'))
    git('tag','v1.2.3')
    config=(repo/'.git/config').read_bytes()
    baseline=source_snapshot(repo,'v1.2.3')
    self.assertEqual(baseline['working'],baseline['committed'])
    shell=shutil.which('sh') if os.name!='nt' else pathlib.Path(shutil.which('git')).resolve().parents[1]/'bin/sh.exe'
    if shell and pathlib.Path(shell).is_file():
     user=run([str(shell),'-c','printf "%s:%s" "$(id -u)" "$(id -g)"'],repo)
     probe=[str(shell),'-c',SOURCE_PROBE,'source-admission',user,repo.as_posix(),
            baseline['source']['sha'],'v1.2.3',baseline['tag_oid']]
     self.assertEqual(run(probe,repo).splitlines()[2:],baseline['working'])
     probe[4]='0:0' if user!='0:0' else '1000:1000'
     events=[]
     with self.assertRaisesRegex(Refusal,'actual build/tool command failed'):
      run(probe,repo,source_probe_lines=events)
     self.assertIn('buddy-source: identity '+user,events)
     self.assertTrue(any(line.startswith('buddy-source: workspace ') for line in events))
     self.assertTrue(any(line.startswith('buddy-source: git-dir ') for line in events))
     self.assertNotIn('buddy-source: stage git-config',events)
     probe[4]=user
    else:
     probe=None
    source.write_bytes(b'original\r\n')
    self.assertNotEqual(git('hash-object','--no-filters','input.txt'),git('rev-parse','HEAD:input.txt'))
    if probe:
     with self.assertRaisesRegex(Refusal,'actual build/tool command failed'):run(probe,repo)
    with self.assertRaisesRegex(Refusal,'clean exact source'):source_snapshot(repo,'v1.2.3')
    source.write_bytes(b'changed\n')
    with self.assertRaisesRegex(Refusal,'clean exact source'):source_snapshot(repo,'v1.2.3')
    source.write_bytes(b'original\n')
    git('update-index','--assume-unchanged','input.txt')
    with self.assertRaisesRegex(Refusal,'unsupported tracked source index state'):source_snapshot(repo,'v1.2.3')
    git('update-index','--no-assume-unchanged','input.txt')
    git('update-index','--chmod=+x','input.txt')
    with self.assertRaisesRegex(Refusal,'clean exact source'):source_snapshot(repo,'v1.2.3')
    self.assertEqual((repo/'.git/config').read_bytes(),config)
