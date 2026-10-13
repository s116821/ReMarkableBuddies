"""Actual developer source-build admission. No external artifacts or release writes."""
import hashlib,json,os,re,shutil,subprocess,tomllib,threading,time,queue
from pathlib import Path
from apk_contract import REPOSITORY,SDK_REPOSITORY,TARGETS,Refusal,require,stable
from process_guard import OwnedProcess

OVERRIDES=['RUSTFLAGS','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS']
UNRECORDED_OVERRIDES={'CARGO_ENCODED_RUSTFLAGS','RUSTC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_BUILD_RUSTC','CARGO_BUILD_RUSTC_WRAPPER','CARGO_BUILD_RUSTFLAGS','CROSS_RUNNER'}
LICENSE_BLOB='f288702d2fa16d3cdf0035b15a9fcbc552cd88e7'
LICENSE_BYTES={'3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986','230184f60bae2feaf244f10a8bac053c8ff33a183bcc365b4d8b876d2b7f4809'}
LICENSE_DECLARATION='https://github.com/s116821/RemarkableBuddiesDocs/blob/d34c14d5d25eee671f7441a69f4feef7bd2c5d69/docs/licensing.md'
IMAGES={
 'armv7-unknown-linux-gnueabihf':'ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf@sha256:1ab143450cd888657baa3563ad663203bc5887629e338ef77718ef515d56fb7a',
 'aarch64-unknown-linux-gnu':'ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:99294ae75048aa07d00bb4f1a18a017e0812ff25753768d21d5938bf7562b40a',
}
RUNTIMES={
 'armv7-unknown-linux-gnueabihf':('/usr/bin/qemu-arm-static','/usr/arm-linux-gnueabihf','/usr/arm-linux-gnueabihf/lib/ld-linux-armhf.so.3',28227984,'9edc1bfb9213599f9dc8c5af6e0a4ab36afa1f43defa07ddd558ccc310fe2023',1285388,'269cf41f9fe4557c032becbca48f913998333e5ff20de67845b96a153b790cde'),
 'aarch64-unknown-linux-gnu':('/usr/bin/qemu-aarch64-static','/usr/aarch64-linux-gnu','/usr/aarch64-linux-gnu/lib/ld-linux-aarch64.so.1',32969136,'fafbcc92255151273ce0030a6f0975700ab7d9c25086cc0e4438784ce21c4ebf',202784,'142f02374792bf5f62293efc3500d472dbbe280dc658cc60480eff73ad46f33c'),
}

def runtime(binary,repo,target,image,diagnostics):
 from docker_owned import phase
 runner,prefix,loader,runner_size,runner_sha,loader_size,loader_sha=RUNTIMES[target]
 deadline=time.monotonic()+120;actual_argv=[]
 def remaining():
  value=deadline-time.monotonic();require(value>0,'complete runtime deadline');return value
 def recorded_command(argv,*args,**kwargs):
  if argv[:2]==['docker','run']:actual_argv.append(list(argv))
  return run(argv,*args,**kwargs)
 def observe():
  identity=json.loads(run(['docker','image','inspect',image,'--format','{"id":{{json .Id}},"os":{{json .Os}},"arch":{{json .Architecture}},"digests":{{json .RepoDigests}}}'],repo,timeout=remaining()))
  require(identity['os']=='linux' and identity['arch']=='amd64' and image in identity['digests'],'runtime immutable image identity')
  raw=phase(['--rm','--network','none','--cap-drop','ALL','--read-only','--entrypoint','/bin/sh',image,'-c',"stat -Lc '%n %s' "+runner+' '+loader+' && sha256sum '+runner+' '+loader],repo,recorded_command,timeout=remaining())
  expected=[runner+' '+str(runner_size),loader+' '+str(loader_size),runner_sha+'  '+runner,loader_sha+'  '+loader]
  require(raw.splitlines()==expected,'runtime runner/loader identity mismatch')
  return dict(image=identity,runner=runner,sysroot=prefix,loader=loader,runner_sha256=runner_sha,loader_sha256=loader_sha,runner_bytes=runner_size,loader_bytes=loader_size)
 before=observe()
 output=phase(['--rm','--network','none','--cap-drop','ALL','--read-only','--memory','512m','--pids-limit','32','--mount',f'type=bind,source={binary},target=/inputs/reader-buddy,readonly','--env','LD_BIND_NOW=1','--entrypoint',runner,image,'-L',prefix,'/inputs/reader-buddy','--version'],repo,recorded_command,timeout=remaining(),private_diagnostics=diagnostics)
 after=observe();require(after==before,'runtime image/runner/loader changed');remaining()
 return output,dict(before=before,after=after,actual_container_argv=actual_argv)

def run(argv,cwd,env=None,timeout=120,stdout_limit=16384,combined_limit=16384,return_streams=False,private_diagnostics=None):
 if private_diagnostics is not None:
  private_diagnostics=Path(private_diagnostics)
  require(combined_limit==16384,'private compiler diagnostic bound')
  private_diagnostics.mkdir(exist_ok=False)
 guard=OwnedProcess(argv,cwd=cwd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 process=guard.process
 events=queue.Queue(maxsize=8)
 stopped=threading.Event()
 def deliver(item):
  while not stopped.is_set():
   try: events.put(item,timeout=0.1);return
   except queue.Full: continue
 def drain(stream,channel):
  try:
   while True:
    block=stream.read1(4096)
    deliver((channel,block))
    if stopped.is_set(): break
    if not block: break
  finally: stream.close()
 readers=[threading.Thread(target=drain,args=(stream,channel),daemon=True) for channel,stream in enumerate((process.stdout,process.stderr))]
 for reader in readers: reader.start()
 buffers=[bytearray(),bytearray()];closed=set();deadline=time.monotonic()+timeout
 try:
  while len(closed)<2:
   remaining=deadline-time.monotonic()
   require(remaining>0,'build/tool deadline')
   try: channel,block=events.get(timeout=min(remaining,0.1))
   except queue.Empty: continue
   if not block: closed.add(channel);continue
   # Metadata can be larger than diagnostics; both are bounded while reading.
   limit=stdout_limit if channel==0 else 16*1024
   require(len(buffers[channel])+len(block)<=limit,'build/tool output bound')
   require(combined_limit is None or sum(len(value) for value in buffers)+len(block)<=combined_limit,'build/tool combined diagnostic bound')
   buffers[channel].extend(block)
  require(guard.exit_status(deadline)==0,'actual build/tool command failed')
  if return_streams:return bytes(buffers[0]),bytes(buffers[1])
  return buffers[0].decode('utf-8').strip()
 finally:
  stopped.set()
  try:
   try:guard.terminate_and_verify()
   finally:guard.close()
  finally:
   for reader in readers: reader.join(timeout=1)
   if private_diagnostics is not None:
    try:
     for name,value in zip(('stdout.bin','stderr.bin'),buffers):
      with (private_diagnostics/name).open('xb') as file:file.write(value)
    except OSError:raise Refusal('private compiler diagnostic retention failed') from None

def git(repo,*args): return run(['git',*args],repo)
def git_conversion(repo):
 autocrlf=git(repo,'config','--default','false','--get','core.autocrlf').lower()
 eol=git(repo,'config','--default','native','--get','core.eol').lower()
 require(autocrlf in {'false','true','input'} and eol in {'lf','crlf','native'},'unsupported source Git conversion semantics')
 return dict(autocrlf=autocrlf,eol=eol,resolved_eol=('crlf' if os.name=='nt' else 'lf') if eol=='native' else eol)
def container_git_conversion(repo,image,container_source,config_home,conversion):
 from docker_owned import phase
 # Same source path and temporary XDG config as the selected Cross invocation;
 # readonly probe must detect local/worktree settings overriding that config.
 raw=phase(['--rm','--network','none','--cap-drop','ALL','--read-only','--mount',f'type=bind,source={repo},target={container_source},readonly','--workdir',container_source,'--env','XDG_CONFIG_HOME='+config_home,'--entrypoint','/bin/sh',image,'-c','git config --default false --get core.autocrlf && git config --default native --get core.eol'],repo,run,timeout=30)
 values=raw.splitlines()
 require(len(values)==2 and values[0].lower()==conversion['autocrlf'],'container Git conversion semantics mismatch')
 effective=values[1].lower();require(effective in {'native','lf','crlf'},'unsupported container Git eol')
 require(conversion['autocrlf']!='false' or ('lf' if effective=='native' else effective)==conversion['resolved_eol'],'container Git conversion semantics mismatch')
 return dict(autocrlf=values[0].lower(),eol=effective,resolved_eol='lf' if effective=='native' else effective)
def sha(path):
 with path.open('rb') as source: return hashlib.file_digest(source,'sha256').hexdigest()

def source_license(repo):
 package=tomllib.loads((repo/'Cargo.toml').read_text())['package']
 license_id=package.get('license')
 license_file=repo/'LICENSE'
 require(license_file.is_file() and not license_file.is_symlink() and 0<license_file.stat().st_size<=65536,'source license file')
 require(git(repo,'hash-object','LICENSE')==git(repo,'rev-parse','HEAD:LICENSE')==LICENSE_BLOB,'known committed source license blob')
 require(sha(license_file) in LICENSE_BYTES,'known source license bytes')
 require(license_id is None or license_id in {'GPL-3.0','GPL-3.0-only'},'contradictory source license declaration')
 # Preserve the reviewed existing project attribution verbatim. The GitHub
 # classifier alone is not an owner grant; the exact project declaration and
 # license bytes are part of this initial fixed mapping.
 return license_id or 'GPL-3.0'

def cargo_configuration(repo):
 roots=[repo,*repo.parents,Path(os.environ.get('CARGO_HOME',Path.home()/'.cargo'))]
 paths=set()
 for root in roots:
  directory=root if root==roots[-1] else root/'.cargo'
  for name in ('config','config.toml'):
   path=directory/name
   if path.exists():paths.add(path.resolve())
 records=[]
 for path in sorted(paths):
  require(path.is_file() and not path.is_symlink() and path.stat().st_size<=65536,'Cargo configuration bound')
  raw=path.read_bytes();config=tomllib.loads(raw.decode())
  require(set(config)<= {'net','registries','registry','source','http','credential-alias','alias','build'},'unrecorded Cargo compiler/profile/environment configuration')
  require(set(config.get('build',{}))<= {'jobs','target-dir','incremental'},'unrecorded Cargo compiler configuration')
  # Do not persist configuration values that can contain credentials.
  records.append(dict(path=str(path),sha256=hashlib.sha256(raw).hexdigest()))
 return records

def source_identity(repo,tag):
 require(stable(tag),'stable existing tag required')
 require(git(repo,'remote','get-url','origin') in {REPOSITORY,REPOSITORY+'.git','git@github.com:s116821/ReMarkableBuddies.git','ssh://git@github.com/s116821/ReMarkableBuddies.git'},'canonical source repository origin')
 require(git(repo,'rev-parse','--is-shallow-repository')=='false','complete source')
 require(not git(repo,'status','--porcelain','--untracked-files=normal'),'clean exact source')
 head=git(repo,'rev-parse','HEAD')
 require(git(repo,'rev-parse',f'refs/tags/{tag}^{{commit}}')==head,'tag/source mismatch')
 return dict(repository=REPOSITORY,tag=tag,sha=head,epoch=int(git(repo,'show','-s','--format=%ct',head)))

def cargo_identity(repo,target,sdk_tag=None,private_observations=None):
 data=json.loads(run(['cargo','metadata','--locked','--format-version','1','--filter-platform',target],repo,stdout_limit=16*1024*1024,combined_limit=None))
 if private_observations is not None: private_observations.append(data)
 resolve=data.get('resolve') or {};root_id=resolve.get('root')
 by_id={node['id']:node for node in resolve.get('nodes',[])}
 require(root_id in by_id,'Cargo root identity')
 reachable=set();pending=[root_id]
 while pending:
  identity=pending.pop()
  if identity in reachable:continue
  require(identity in by_id,'resolved dependency identity')
  reachable.add(identity);pending.extend(by_id[identity]['dependencies'])
 packages=[p for p in data['packages'] if p['name']=='remarkable-open-sdk' and p['id'] in reachable]
 require(len(packages)<=1,'ambiguous SDK package')
 sdk=None
 if packages:
  package=packages[0];source=package.get('source','')
  require(source.split('?',1)[0].split('#',1)[0]=='git+'+SDK_REPOSITORY,'canonical SDK git source')
  fragment=source.rsplit('#',1)[-1]
  require(re.fullmatch('[0-9a-f]{40}',fragment),'resolved SDK source')
  manifest=Path(package['manifest_path']);root=manifest.parent
  require(git(root,'rev-parse','HEAD')==fragment and not git(root,'status','--porcelain','--untracked-files=normal'),'clean actual SDK checkout')
  sdk=dict(ref_kind='commit-pinned',repository=SDK_REPOSITORY,sha=fragment,tag=None,manifest_sha256=sha(manifest))
  if sdk_tag:
   require(stable(sdk_tag) and git(root,'rev-parse',f'refs/tags/{sdk_tag}^{{commit}}')==fragment,'actual SDK tag resolution')
   sdk.update(ref_kind='tagged-git',tag=sdk_tag)
 else: require(sdk_tag is None,'SDK tag for absent SDK')
 nodes=[n for n in resolve.get('nodes',[]) if n['id']==root_id]
 require(len(nodes)==1,'Cargo root identity')
 features=sorted(set(nodes[0]['features']))
 lock=sha(repo/'Cargo.lock')
 identities=[]
 for package in data['packages']:
  source=package.get('source')
  if source is None:
   require(Path(package['manifest_path']).resolve().is_relative_to(repo.resolve()),'external unrecorded path dependency')
   source='SOURCE_ROOT/'+Path(package['manifest_path']).resolve().relative_to(repo.resolve()).as_posix()
  identities.append(dict(name=package['name'],version=package['version'],source=source,manifest_sha256=sha(Path(package['manifest_path']))))
 graph=[dict(id=node['id'],features=sorted(node['features']),dependencies=sorted(node['dependencies'])) for node in resolve.get('nodes',[])]
 # Cargo IDs may contain the actual source root. Normalize only that exact
 # path URL prefix, preserving every remaining package/source identity.
 root_uri=repo.resolve().as_uri()
 for node in graph:
  node['id']=node['id'].replace('path+'+root_uri+'#','path+SOURCE_ROOT#')
  node['dependencies']=[dep.replace('path+'+root_uri+'#','path+SOURCE_ROOT#') for dep in node['dependencies']]
 normalized=dict(sdk=sdk,features=features,packages=sorted(identities,key=lambda p:(p['name'],p['version'],p['source'])),graph=sorted(graph,key=lambda n:n['id']),lock_sha256=lock,target=target)
 digest=hashlib.sha256(json.dumps(normalized,sort_keys=True,separators=(',',':')).encode()).hexdigest()
 return dict(sdk=sdk,features=features,lock_sha256=lock,metadata_identity_sha256=digest)

def elf_header(path):
 with path.open('rb') as source: raw=source.read(64)
 require(raw[:4]==b'\x7fELF' and raw[5]==1,'little-endian ELF')
 cls={1:32,2:64}.get(raw[4]);machine=int.from_bytes(raw[18:20],'little')
 require((cls,machine) in [(32,40),(64,183)],'supported actual ELF')
 return cls,'ARM' if machine==40 else 'AArch64'

def build(repo,tag,target,target_dir,cross_configuration,sdk_tag=None):
 from apk_compiler import identity as compiler_identity
 from docker_owned import cross_phase
 from abi import qualify
 repo=repo.resolve();target_dir=target_dir.resolve();configuration=cross_configuration.resolve()
 require(target in TARGETS and configuration.is_file(),'build target/configuration')
 require(not target_dir.exists() and not target_dir.is_relative_to(repo),'fresh external target root required')
 require((repo/'.git').is_dir(),'initial Cross source requires standalone full Git checkout')
 config=tomllib.loads(configuration.read_text());image=config['target'][target]['image']
 require(image==IMAGES[target],'reviewed immutable build image')
 configuration_hash=sha(configuration);helper_hash=sha(Path(__file__))
 cross=Path(shutil.which('cross') or '')
 require(cross.is_file(),'actual cross executable')
 private_cargo=[];before_source=source_identity(repo,tag);license_id=source_license(repo)
 configurations=cargo_configuration(repo);conversion=git_conversion(repo);before_cargo=cargo_identity(repo,target,sdk_tag,private_cargo)
 require(all(not os.environ.get(k) for k in OVERRIDES),'unreviewed build override')
 require(not any(value and (key in UNRECORDED_OVERRIDES or key.startswith(('CARGO_PROFILE_','CROSS_')) or key=='DOCKER_OPTS' or key=='RUSTUP_TOOLCHAIN' or (key.startswith('CARGO_TARGET_') and key.endswith(('_RUSTFLAGS','_LINKER','_RUNNER')))) for key,value in os.environ.items()),'unrecorded compiler/linker/runner/toolchain override')
 require(not any(k.startswith('VERGEN_') for k in os.environ),'version override')
 target_dir.mkdir(exist_ok=False)
 env=dict(os.environ,READER_BUDDY_RELEASE_TAG=tag,READER_BUDDY_RELEASE_SHA=before_source['sha'],CROSS_CONFIG=str(configuration),LD_BIND_NOW='1',SOURCE_DATE_EPOCH=str(before_source['epoch']))
 env.update({key:'' for key in OVERRIDES})
 before_compiler=compiler_identity(repo,target,image,cross,run,sha)
 # Use exactly the observed stable Linux counterpart chosen by the pinned
 # Cross image platform. A source/compiler/container switch refuses below.
 argv=[str(cross),'+stable','build','--locked','--release','--target',target,'--target-dir',str(target_dir),'--bin','reader-buddy']
 container_source=repo.as_posix()
 if os.name=='nt':container_source='/mnt/'+container_source[0].lower()+container_source[2:]
 import tempfile
 with tempfile.TemporaryDirectory(prefix='apk-git-config-',dir=repo/'.git') as config_home:
  config_home=Path(config_home);(config_home/'git').mkdir()
  git(repo,'config','--file',str(config_home/'git/config'),'safe.directory',container_source)
  git(repo,'config','--file',str(config_home/'git/config'),'core.autocrlf',conversion['autocrlf'])
  git(repo,'config','--file',str(config_home/'git/config'),'core.eol',conversion['resolved_eol'])
  env['XDG_CONFIG_HOME']=container_source+'/.git/'+config_home.name
  before_conversion=container_git_conversion(repo,image,container_source,env['XDG_CONFIG_HOME'],conversion)
  cross_phase(argv,repo,env,run,timeout=900,private_diagnostics=target_dir/'compiler-diagnostics')
  binary=target_dir/target/'release/reader-buddy'
  require(binary.is_file() and not binary.is_symlink(),'new build executable')
  with binary.open('rb') as checked:
   before=os.fstat(checked.fileno());require(1<=before.st_size<=134217728,'actual executable bound')
   executed_hash=hashlib.file_digest(checked,'sha256').hexdigest();checked.seek(0)
   output,runtime_evidence=runtime(binary,repo,target,image,target_dir/'runtime-diagnostics')
   runtime_argv=runtime_evidence['actual_container_argv'][1]
   require(output.split()[-1:]==[tag[1:]],'actual target runtime tag version')
   after=os.fstat(checked.fileno());current=binary.stat()
   identity=lambda info:(info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns)
   require(identity(before)==identity(after)==identity(current),'executed binary file identity changed')
   require(hashlib.file_digest(checked,'sha256').hexdigest()==executed_hash,'executed binary bytes changed')
   checked.seek(0)
   frozen_dir=target_dir/'verified-input';frozen_dir.mkdir(exist_ok=False);frozen=frozen_dir/'reader-buddy'
   with frozen.open('xb') as destination:
    remaining=before.st_size;copied=hashlib.sha256()
    while remaining:
     block=checked.read(min(65536,remaining));require(block,'short checked executable')
     destination.write(block);copied.update(block);remaining-=len(block)
   require(copied.hexdigest()==executed_hash and identity(os.fstat(checked.fileno()))==identity(before),'checked descriptor changed during freeze')
  require(container_git_conversion(repo,image,container_source,env['XDG_CONFIG_HOME'],conversion)==before_conversion,'container Git conversion changed')
 require(sha(frozen)==executed_hash,'frozen executable identity')
 cls,machine=elf_header(frozen);arch,abi,expected_cls,expected_machine=TARGETS[target]
 require((cls,machine)==(expected_cls,expected_machine),'target ELF correspondence')
 actual_elf=qualify(frozen,target,lambda command:run(command,repo,stdout_limit=65536,combined_limit=None))
 after_compiler=compiler_identity(repo,target,image,cross,run,sha)
 require(after_compiler==before_compiler,'actual Cross compiler/image identity changed')
 require(source_identity(repo,tag)==before_source and cargo_identity(repo,target,sdk_tag,private_cargo)==before_cargo,'build inputs changed')
 require(cargo_configuration(repo)==configurations and source_license(repo)==license_id,'source configuration/license changed')
 require(git_conversion(repo)==conversion,'source Git conversion semantics changed')
 require(sha(configuration)==configuration_hash and sha(Path(__file__))==helper_hash,'build helper/configuration changed')
 fields=before_compiler['rustc_fields'];sdk=before_cargo['sdk']
 observed_image=next(value for value in before_compiler['image']['digests'] if value==image)
 metadata=dict(schema=1,evidence='developer-source-build',eligibility='not-qualified',source=before_source,
  target=dict(rust_target=target,apk_arch=arch,abi=abi,elf_class=cls,elf_machine=machine,elf_endian='little',interpreter=actual_elf['elf']['interpreter']),
  sdk=sdk,cargo={key:before_cargo[key] for key in ['lock_sha256','metadata_identity_sha256','features']},
  toolchain=dict(rustc_release=fields['release'],rustc_commit=None if fields.get('commit-hash')=='unknown' else fields.get('commit-hash'),rustc_host=fields['host'],cross_version=before_compiler['cross_version'],cross_binary_sha256=before_compiler['cross_binary_sha256'],image_digest='sha256:'+observed_image.split('@sha256:',1)[1],build_configuration_sha256=configuration_hash),
  build=dict(argv=['TOOL_ROOT/'+cross.name,'+stable','build','--locked','--release','--target',target,'--target-dir','TARGET_ROOT','--bin','reader-buddy'],environment={key:'' for key in OVERRIDES},profile='legacy-no-sdk' if sdk is None else 'unqualified-development',source_build_helper_sha256=helper_hash),
  binary=dict(path='usr/bin/reader-buddy',sha256=executed_hash,bytes=before.st_size,mode=493))
 from apk_contract import canonical
 canonical(metadata)
 private_evidence=dict(source_root=str(repo),target_root=str(target_dir),configuration_path=str(configuration),cargo_configuration_digests=configurations,actual_build_argv=argv,actual_runtime_argv=runtime_argv,recorded_environment={key:env[key] for key in [*OVERRIDES,'READER_BUDDY_RELEASE_TAG','READER_BUDDY_RELEASE_SHA','CROSS_CONFIG','LD_BIND_NOW','SOURCE_DATE_EPOCH','XDG_CONFIG_HOME']},cargo_observations=private_cargo,compiler_before=before_compiler,compiler_after=after_compiler,executed_binary_identity=list(identity(before)),executed_binary_sha256=executed_hash,elf_observation=actual_elf,runtime_version_output=output,source_license=license_id,license_basis=dict(project_declaration=LICENSE_DECLARATION,committed_blob=LICENSE_BLOB,actual_file_sha256=sha(repo/'LICENSE'),existing_detected_identifier='GPL-3.0',classifier_is_owner_grant=False))
 private_evidence['git_conversion']=conversion
 private_evidence['container_git_conversion']=before_conversion
 private_evidence['runtime_observations']=runtime_evidence
 return frozen,metadata,private_evidence
