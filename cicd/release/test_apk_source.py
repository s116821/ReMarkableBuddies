"""Real child-process refusal controls; no device or network access."""
import json,os,pathlib,shutil,sys,time,unittest,tempfile,hashlib
from apk_contract import Refusal
from apk_source_build import run,source_license,LICENSE_BLOB,build,git_conversion,container_source_snapshot,source_snapshot,runtime,RUNTIMES,IMAGES,SOURCE_PROBE
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

class GitConversion(unittest.TestCase):
 def test_defaults_native_resolution_and_explicit_precedence(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory)
   with patch('apk_source_build.git',side_effect=['false','native']):
    value=git_conversion(root)
   self.assertEqual(value['resolved_eol'],'crlf' if sys.platform=='win32' else 'lf')
   with patch('apk_source_build.git',side_effect=['input','crlf']):
    self.assertEqual(git_conversion(root),dict(autocrlf='input',eol='crlf',resolved_eol='crlf'))
   with patch('apk_source_build.git',side_effect=['unknown','native']):
    with self.assertRaisesRegex(Refusal,'unsupported source Git'):git_conversion(root)

 def test_readonly_container_mapping_and_raw_inventory(self):
  oid='1'*40
  host=dict(source=dict(sha='2'*40,tag='v1.2.3'),tag_oid='3'*40,working=[oid],committed=[oid],
            conversion=dict(autocrlf='true',eol='native',resolved_eol='crlf'))
  for output,committed,message in (
   ('false\nnative\n'+oid,oid,None),
   ('false\nnative\n'+oid,'4'*40,'exact committed raw source bytes'),
   ('false\nnative\n'+'5'*40,oid,'raw source inventory mismatch'),
  ):
   with self.subTest(output=output,committed=committed),patch('docker_owned.phase',return_value=output) as phase:
    host['committed']=[committed]
    args=(pathlib.Path('source'),'image',('/mnt/c/source','1000:1000'),host)
    if message:
     with self.assertRaisesRegex(Refusal,message):container_source_snapshot(*args)
    else:
     self.assertEqual(container_source_snapshot(*args),dict(autocrlf='false',eol='native',resolved_eol='lf'))
    argv=phase.call_args.args[0]
    for option,value in (('--user','1000:1000'),('--userns','host'),('--workdir','/mnt/c/source'),('--network','none'),('--pull','never')):
     self.assertEqual(argv[argv.index(option)+1],value)
    self.assertTrue(argv[argv.index('--mount')+1].endswith(',target=/mnt/c/source,readonly'))
    self.assertNotIn('--env',argv)
    self.assertNotIn('XDG_CONFIG_HOME',' '.join(argv))

 def test_diagnostic_receipt_preserves_early_refusal_and_filters_stderr(self):
  with tempfile.TemporaryDirectory() as directory:
   root=pathlib.Path(directory);repo=root/'source';repo.mkdir()
   destination=root/'receipt.json'
   host=dict(source=dict(sha='2'*40,tag='v1.2.3'),tag_oid='3'*40)
   def phase(args,cwd,command,**options):
    return command(['docker','run','--name','owned-probe',*args],cwd,**options)
   def local_command(argv,cwd,**options):
    code=("import sys; sys.stderr.write('private raw stderr\\n"
          "buddy-source: stage ownership\\nbuddy-source: identity 1000:1000\\n"
          "buddy-source: workspace 0:0:755\\nbuddy-source: manifest 0:0:777\\n"
          "buddy-source: invalid path payload\\n'); sys.exit(1)")
    return run([sys.executable,'-c',code],cwd,**options)
   with patch('docker_owned.phase',side_effect=phase),patch('apk_source_build.run',side_effect=local_command):
    with self.assertRaisesRegex(Refusal,'actual build/tool command failed'):
     container_source_snapshot(repo,'image',('/source','1000:1000'),host,destination)
   receipt=json.loads(destination.read_text())
   self.assertEqual(receipt['transport'],'refused')
   self.assertEqual(receipt['admission'],'refused')
   self.assertEqual(receipt['argv'][:4],['docker','run','--name','owned-probe'])
   self.assertEqual(receipt['events'],['buddy-source: stage ownership','buddy-source: identity 1000:1000',
                    'buddy-source: workspace 0:0:755','buddy-source: manifest 0:0:777'])
   self.assertNotIn('private raw stderr',destination.read_text())

 def test_actual_git_raw_bytes_index_mode_and_dirty_status(self):
  with tempfile.TemporaryDirectory() as directory:
   repo=pathlib.Path(directory).resolve()
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

class DirectRuntime(unittest.TestCase):
 def test_pinned_runtime_uses_selected_file_without_compiler(self):
  target='armv7-unknown-linux-gnueabihf';runner,prefix,loader,rs,rh,ls,lh=RUNTIMES[target]
  pins='\n'.join([runner+' '+str(rs),loader+' '+str(ls),rh+'  '+runner,lh+'  '+loader])
  commands=[]
  def command(argv,*args,**kwargs):
   commands.append(argv)
   if argv[:3]==['docker','image','inspect']:return __import__('json').dumps(dict(id='pinned',os='linux',arch='amd64',digests=[IMAGES[target]]))
   return pins if '/bin/sh' in argv else 'reader-buddy 1.2.3'
  def phase(args,cwd,callback,**kwargs):return callback(['docker','run',*args],cwd,**kwargs)
  with tempfile.TemporaryDirectory() as directory,patch('apk_source_build.run',side_effect=command),patch('docker_owned.phase',side_effect=phase):
   output,evidence=runtime(pathlib.Path('/fresh/reader-buddy'),pathlib.Path('.'),target,IMAGES[target],pathlib.Path(directory)/'runtime')
  self.assertEqual(output,'reader-buddy 1.2.3')
  execution=evidence['actual_container_argv'][1]
  self.assertIn('type=bind,source='+str(pathlib.Path('/fresh/reader-buddy'))+',target=/inputs/reader-buddy,readonly',execution)
  self.assertEqual(execution[-4:],['-L',prefix,'/inputs/reader-buddy','--version'])
  self.assertFalse(any(value in ('cargo','cross','build') for argv in commands for value in argv))
 def test_missing_or_changed_pin_refuses_before_execution(self):
  target='armv7-unknown-linux-gnueabihf'
  image=__import__('json').dumps(dict(id='pinned',os='linux',arch='amd64',digests=[IMAGES[target]]))
  with tempfile.TemporaryDirectory() as directory,patch('apk_source_build.run',return_value=image),patch('docker_owned.phase',return_value='wrong pins') as calls:
   with self.assertRaisesRegex(Refusal,'runner/loader identity'):runtime(pathlib.Path('/fresh/bin'),pathlib.Path('.'),target,IMAGES[target],pathlib.Path(directory)/'runtime')
   self.assertEqual(calls.call_count,1)
   control=__import__('json').loads((pathlib.Path(directory)/'runtime.control.json').read_text());self.assertEqual(control['stage'],'before-observation');self.assertIsNone(control['before']);self.assertIsNone(control['after'])
   with self.assertRaises(FileExistsError):runtime(pathlib.Path('/fresh/bin'),pathlib.Path('.'),target,IMAGES[target],pathlib.Path(directory)/'runtime')
 def test_image_change_after_execution_refuses(self):
  target='armv7-unknown-linux-gnueabihf';runner,prefix,loader,rs,rh,ls,lh=RUNTIMES[target]
  pins='\n'.join([runner+' '+str(rs),loader+' '+str(ls),rh+'  '+runner,lh+'  '+loader])
  images=[__import__('json').dumps(dict(id=value,os='linux',arch='amd64',digests=[IMAGES[target]])) for value in ('before','after')]
  with tempfile.TemporaryDirectory() as directory,patch('apk_source_build.run',side_effect=images),patch('docker_owned.phase',side_effect=[pins,'reader-buddy 1.2.3',pins]):
   with self.assertRaisesRegex(Refusal,'runtime image/runner/loader changed'):runtime(pathlib.Path('/fresh/bin'),pathlib.Path('.'),target,IMAGES[target],pathlib.Path(directory)/'runtime')
   control=__import__('json').loads((pathlib.Path(directory)/'runtime.control.json').read_text());self.assertEqual(control['status'],'refused');self.assertEqual(control['stage'],'after-observation');self.assertEqual(control['after']['image']['id'],'after')

 def test_timeout_retains_completed_observation_without_fabricated_after(self):
  target='armv7-unknown-linux-gnueabihf';runner,prefix,loader,rs,rh,ls,lh=RUNTIMES[target]
  pins='\n'.join([runner+' '+str(rs),loader+' '+str(ls),rh+'  '+runner,lh+'  '+loader])
  image=__import__('json').dumps(dict(id='pinned',os='linux',arch='amd64',digests=[IMAGES[target]]))
  with tempfile.TemporaryDirectory() as directory,patch('apk_source_build.run',return_value=image),patch('docker_owned.phase',side_effect=[pins,Refusal('build/tool deadline')]):
   with self.assertRaisesRegex(Refusal,'deadline'):runtime(pathlib.Path('/fresh/bin'),pathlib.Path('.'),target,IMAGES[target],pathlib.Path(directory)/'runtime')
   file=pathlib.Path(directory)/'runtime.control.json';control=__import__('json').loads(file.read_text())
   self.assertEqual(control['stage'],'execution');self.assertEqual(control['before']['image']['id'],'pinned');self.assertIsNone(control['after']);self.assertLessEqual(file.stat().st_size,8192)

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
  raw=(pathlib.Path(__file__).resolve().parents[2]/'LICENSE').read_bytes()
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
  for fault in ['runtime-replacement','compiler-switch','git-conversion-switch','container-refusal']:
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
    def runtime_call(binary,*args):
     if fault=='runtime-replacement':binary.write_bytes(b'changed after runtime')
     return 'reader-buddy 1.2.3',dict(actual_container_argv=[[],['docker','run','observed-runtime'],[]])
    identities=[compiler,changed if fault=='compiler-switch' else compiler]
    def cargo_call(repo,target,sdk_tag,observations):
     observations.append(dict(workspace_root=str(source),packages=[],workspace_members=[]))
     return cargo
    host=dict(source=source_id,conversion={'autocrlf':'false'})
    after=dict(host,conversion={'autocrlf':'true'}) if fault=='git-conversion-switch' else host
    def container_call(*args):
     if fault=='container-refusal':raise Refusal('ordinary container Git ownership')
     return {'autocrlf':'false'}
    with patch('apk_source_build.shutil.which',return_value=str(fake_cross)), \
         patch('apk_source_build.runtime',side_effect=runtime_call), \
         patch('apk_source_build.source_snapshot',side_effect=[host,after]), \
         patch('apk_source_build.source_license',return_value='GPL-3.0'), \
         patch('apk_source_build.cargo_configuration',return_value=[]), \
         patch('apk_source_build.cross_source_mapping',return_value=('/source','1000:1000')), \
         patch('apk_source_build.container_source_snapshot',side_effect=container_call), \
         patch('apk_source_build.cargo_identity',side_effect=cargo_call), \
         patch('apk_compiler.identity',side_effect=identities) as compiler_probe, \
         patch('docker_owned.cross_phase',side_effect=cross_call) as cross_build, \
         patch('abi.qualify',return_value={'elf':{'interpreter':'/lib/ld-linux-armhf.so.3'}}):
     messages={'runtime-replacement':'executed binary file identity changed',
               'compiler-switch':'actual Cross compiler/image identity changed',
               'git-conversion-switch':'source Git admission identity changed',
               'container-refusal':'ordinary container Git ownership'}
     with self.assertRaisesRegex(Refusal,messages[fault]):
      build(source,'v1.2.3','armv7-unknown-linux-gnueabihf',target,configuration)
     if fault=='container-refusal':
      compiler_probe.assert_not_called();cross_build.assert_not_called()
      self.assertFalse(target.exists())

if __name__=='__main__': unittest.main()
