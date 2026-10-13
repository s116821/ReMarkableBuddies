"""Real child-process refusal controls; no device or network access."""
import pathlib,sys,time,unittest,tempfile,hashlib
from apk_contract import Refusal
from apk_source_build import run,source_license,LICENSE_BLOB
from apk_builder import copy_verified
from unittest.mock import patch

class CommandBounds(unittest.TestCase):
 def command(self,code,timeout=3):
  return run([sys.executable,'-c',code],pathlib.Path.cwd(),timeout=timeout)
 def test_success(self):
  self.assertEqual(self.command("print('verified')"),'verified')
 def test_diagnostics_are_bounded(self):
  with self.assertRaisesRegex(Refusal,'output bound'):
   self.command("import sys;sys.stderr.write('x'*17000)")
 def test_failure_does_not_expose_diagnostics(self):
  with self.assertRaisesRegex(Refusal,'command failed') as result:
   self.command("import sys;sys.stderr.write('private sentinel');sys.exit(2)")
  self.assertNotIn('private sentinel',str(result.exception))
 def test_deadline(self):
  start=time.monotonic()
  with self.assertRaisesRegex(Refusal,'deadline'):
   self.command('import time;time.sleep(10)',timeout=0.2)
  self.assertLess(time.monotonic()-start,3)

class FrozenInput(unittest.TestCase):
 def test_exact_bytes(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory);source=root/'source';destination=root/'staged'
   source.write_bytes(b'actual bytes');digest=hashlib.sha256(source.read_bytes()).hexdigest()
   copy_verified(source,destination,12,digest)
   self.assertEqual(destination.read_bytes(),source.read_bytes())
 def test_wrong_digest(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory);source=root/'source';source.write_bytes(b'actual bytes')
   with self.assertRaisesRegex(Refusal,'input hash'):copy_verified(source,root/'staged',12,'0'*64)
 def test_never_overwrites_destination(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory);source=root/'source';destination=root/'staged'
   source.write_bytes(b'new');destination.write_bytes(b'preserved')
   with self.assertRaises(FileExistsError):copy_verified(source,destination,3,hashlib.sha256(b'new').hexdigest())
   self.assertEqual(destination.read_bytes(),b'preserved')

class LicenseAttribution(unittest.TestCase):
 def fixture(self,root,declaration=None):
  raw=(pathlib.Path(__file__).resolve().parents[1]/'LICENSE').read_bytes()
  (root/'LICENSE').write_bytes(raw)
  text='[package]\nname="license-fixture"\n'
  if declaration is not None:text+='license='+repr(declaration)+'\n'
  (root/'Cargo.toml').write_text(text)
 def test_legacy_and_explicit_matching(self):
  with tempfile.TemporaryDirectory() as directory,patch('apk_source_build.git',return_value=LICENSE_BLOB):
   root=pathlib.Path(directory)
   for declaration,expected in [(None,'GPL-3.0'),('GPL-3.0-only','GPL-3.0-only')]:
    self.fixture(root,declaration);self.assertEqual(source_license(root),expected)
 def test_contradictory_or_changed_bytes(self):
  with tempfile.TemporaryDirectory() as directory,patch('apk_source_build.git',return_value=LICENSE_BLOB):
   root=pathlib.Path(directory);self.fixture(root,'GPL-3.0-or-later')
   with self.assertRaisesRegex(Refusal,'contradictory'):source_license(root)
   self.fixture(root);(root/'LICENSE').write_bytes(b'unknown license')
   with self.assertRaisesRegex(Refusal,'license bytes'):source_license(root)

if __name__=='__main__': unittest.main()
