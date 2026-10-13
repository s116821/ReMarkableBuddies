"""Real child-process refusal controls; no device or network access."""
import pathlib,sys,time,unittest,tempfile,hashlib
from apk_contract import Refusal
from apk_source_build import run,source_license,LICENSE_BLOB,build
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
 def test_private_diagnostics_survive_success_and_deadline(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory)
   self.assertEqual(run([sys.executable,'-c',"print('compiler success')"],root,private_diagnostics=root/'success'),'compiler success')
   self.assertEqual((root/'success/stdout.bin').read_bytes(),b'compiler success\n' if sys.platform!='win32' else b'compiler success\r\n')
   with self.assertRaisesRegex(Refusal,'deadline'):
    run([sys.executable,'-c',"import sys,time;sys.stderr.write('private compiler failure');sys.stderr.flush();time.sleep(10)"],root,timeout=0.2,private_diagnostics=root/'refused')
   self.assertEqual((root/'refused/stderr.bin').read_bytes(),b'private compiler failure')
   self.assertLessEqual(sum(path.stat().st_size for path in (root/'refused').iterdir()),16384)
   with self.assertRaises(FileExistsError):
    run([sys.executable,'-c',"print('overwrite')"],root,private_diagnostics=root/'success')
 def test_stdout_and_combined_diagnostic_caps(self):
  for code in ["print('x'*17000)","import sys;sys.stdout.write('x'*9000);sys.stderr.write('y'*9000)"]:
   with self.subTest(code=code),self.assertRaisesRegex(Refusal,'bound'):self.command(code)
 def test_descendant_cannot_write_after_refusal(self):
  with tempfile.TemporaryDirectory() as directory:
   marker=pathlib.Path(directory)/'late-marker'
   child="import time,pathlib;time.sleep(0.7);pathlib.Path("+repr(str(marker))+").write_text('late')"
   parent="import subprocess,sys,time;subprocess.Popen([sys.executable,'-c',"+repr(child)+"]);time.sleep(10)"
   with self.assertRaisesRegex(Refusal,'deadline'):self.command(parent,timeout=0.2)
   time.sleep(1)
   self.assertFalse(marker.exists(),'owned descendant continued after refusal')

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

class FreshBuildAdmission(unittest.TestCase):
 def test_existing_caller_output_refused_before_source_or_commands(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory);source=root/'source';source.mkdir();target=root/'caller-output';target.mkdir()
   cached=target/'reader-buddy';cached.write_bytes(b'caller supplied cached executable')
   configuration=pathlib.Path(__file__).with_name('Cross.apk.toml')
   with patch('apk_source_build.source_identity',side_effect=AssertionError('source commands ran')):
    with self.assertRaisesRegex(Refusal,'fresh external target'):build(source,'v1.2.3','armv7-unknown-linux-gnueabihf',target,configuration)
   self.assertEqual(cached.read_bytes(),b'caller supplied cached executable')
 def test_runtime_replacement_or_compiler_switch_refuses(self):
  # Real file changes, with compiler/runner transport mocked. No source-build
  # result or real executable authority is inferred from this fixture.
  for fault in ['runtime-replacement','compiler-switch']:
   with self.subTest(fault=fault),tempfile.TemporaryDirectory() as directory:
    root=pathlib.Path(directory);source=root/'source';source.mkdir();(source/'.git').mkdir();target=root/'fresh-output'
    configuration=pathlib.Path(__file__).with_name('Cross.apk.toml')
    fake_cross=root/'cross';fake_cross.write_bytes(b'controlled tool fixture')
    source_id=dict(repository='https://github.com/s116821/ReMarkableBuddies',tag='v1.2.3',sha='1'*40,epoch=1)
    cargo=dict(sdk=None,features=[],lock_sha256='2'*64,metadata_identity_sha256='3'*64)
    compiler=dict(rustc_fields=dict(release='1.98.1',host='x86_64-unknown-linux-gnu',**{'commit-hash':'4'*40}),cross_version='cross controlled fixture',cross_binary_sha256='5'*64,image=dict(digests=[]))
    changed=dict(compiler,cross_binary_sha256='6'*64)
    def cross_call(argv,*args,**kwargs):
     file=target/'armv7-unknown-linux-gnueabihf/release/reader-buddy'
     if argv[2]=='build':
      file.parent.mkdir(parents=True);raw=bytearray(64);raw[:6]=b'\x7fELF\x01\x01';raw[18:20]=(40).to_bytes(2,'little');file.write_bytes(raw)
      return ''
     if fault=='runtime-replacement':file.write_bytes(b'changed after runtime')
     return 'reader-buddy 1.2.3'
    identities=[compiler,changed if fault=='compiler-switch' else compiler]
    with patch('apk_source_build.shutil.which',return_value=str(fake_cross)),patch('apk_source_build.source_identity',return_value=source_id),patch('apk_source_build.source_license',return_value='GPL-3.0'),patch('apk_source_build.cargo_configuration',return_value=[]),patch('apk_source_build.cargo_identity',return_value=cargo),patch('apk_source_build.git',return_value=''),patch('apk_compiler.identity',side_effect=identities),patch('docker_owned.cross_phase',side_effect=cross_call),patch('abi.qualify',return_value={'elf':{'interpreter':'/lib/ld-linux-armhf.so.3'}}):
     message='executed binary file identity changed' if fault=='runtime-replacement' else 'actual Cross compiler/image identity changed'
     with self.assertRaisesRegex(Refusal,message):build(source,'v1.2.3','armv7-unknown-linux-gnueabihf',target,configuration)

if __name__=='__main__': unittest.main()
