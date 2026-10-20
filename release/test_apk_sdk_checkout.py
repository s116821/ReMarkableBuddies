"""Actual local Git checkout controls; no build, network or shared-cache writes."""
import os,pathlib,subprocess,tempfile,unittest
from unittest.mock import patch
from apk_contract import Refusal
from apk_source_build import clean_sdk_checkout,git

class SdkCheckout(unittest.TestCase):
 def setUp(self):
  self.temporary=tempfile.TemporaryDirectory();self.addCleanup(self.temporary.cleanup)
  self.root=pathlib.Path(self.temporary.name)
  self.env=dict(os.environ,GIT_AUTHOR_NAME='Fixture',GIT_AUTHOR_EMAIL='fixture@example.invalid',GIT_COMMITTER_NAME='Fixture',GIT_COMMITTER_EMAIL='fixture@example.invalid')
  self.command('init','--quiet')
  (self.root/'Cargo.toml').write_text('[package]\nname="fixture"\n',encoding='utf-8')
  self.command('add','Cargo.toml');self.command('commit','--quiet','-m','fixture')
  self.head=self.command('rev-parse','HEAD').strip()
 def command(self,*args):
  return subprocess.check_output(['git',*args],cwd=self.root,env=self.env,text=True)
 def check(self):clean_sdk_checkout(self.root,self.head)
 def test_absent_and_empty_root_marker(self):
  self.check();(self.root/'.cargo-ok').touch();self.check()
  self.assertEqual(self.command('status','--porcelain').strip(),'?? .cargo-ok')
 def test_tracked_and_untracked_changes(self):
  (self.root/'.cargo-ok').touch()
  (self.root/'Cargo.toml').write_text('changed',encoding='utf-8')
  with self.assertRaisesRegex(Refusal,'clean actual SDK checkout'):self.check()
  self.command('restore','Cargo.toml');(self.root/'extra').write_text('extra',encoding='utf-8')
  with self.assertRaisesRegex(Refusal,'clean actual SDK checkout'):self.check()
 def test_nested_and_renamed_markers(self):
  for name in ('nested/.cargo-ok','.cargo-ok-other'):
   with self.subTest(name=name):
    path=self.root/name;path.parent.mkdir(exist_ok=True);path.touch()
    with self.assertRaisesRegex(Refusal,'clean actual SDK checkout'):self.check()
    path.unlink()
 def test_tracked_marker(self):
  (self.root/'.cargo-ok').touch();self.command('add','.cargo-ok')
  with self.assertRaisesRegex(Refusal,'tracked SDK completion marker'):self.check()
 def test_nonempty_and_directory(self):
  marker=self.root/'.cargo-ok';marker.write_bytes(b'not-empty')
  with self.assertRaisesRegex(Refusal,'invalid SDK completion marker'):self.check()
  marker.unlink();marker.mkdir()
  with self.assertRaisesRegex(Refusal,'invalid SDK completion marker'):self.check()
 def test_hardlink(self):
  target=self.root/'target';target.touch();marker=self.root/'.cargo-ok'
  os.link(target,marker)
  with self.assertRaisesRegex(Refusal,'invalid SDK completion marker'):self.check()
 def test_symlink(self):
  target=self.root/'target';target.touch();marker=self.root/'.cargo-ok'
  try:marker.symlink_to(target)
  except OSError:self.skipTest('host does not permit symlinks')
  with self.assertRaisesRegex(Refusal,'invalid SDK completion marker'):self.check()
 def test_wrong_head_and_nested_root(self):
  with self.assertRaisesRegex(Refusal,'clean actual SDK checkout'):clean_sdk_checkout(self.root,'0'*40)
  nested=self.root/'nested';nested.mkdir()
  with self.assertRaisesRegex(Refusal,'actual SDK checkout root'):clean_sdk_checkout(nested,self.head)
 def test_marker_changed_during_status(self):
  marker=self.root/'.cargo-ok';marker.touch()
  def changing(root,*args):
   result=git(root,*args)
   if args[0]=='status':marker.unlink()
   return result
  with patch('apk_source_build.git',side_effect=changing):
   with self.assertRaisesRegex(Refusal,'SDK completion marker changed'):self.check()
