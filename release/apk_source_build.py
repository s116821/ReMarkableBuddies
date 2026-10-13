"""Actual developer source-build admission. No external artifacts or release writes."""
import hashlib,json,os,re,shutil,subprocess,tomllib,threading,time,queue
from pathlib import Path
from apk_contract import REPOSITORY,SDK_REPOSITORY,TARGETS,Refusal,require,stable

OVERRIDES=['RUSTFLAGS','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS']
UNRECORDED_OVERRIDES={'CARGO_ENCODED_RUSTFLAGS','RUSTC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_BUILD_RUSTC','CARGO_BUILD_RUSTC_WRAPPER','CARGO_BUILD_RUSTFLAGS','CROSS_RUNNER'}
LICENSE_BLOB='f288702d2fa16d3cdf0035b15a9fcbc552cd88e7'
LICENSE_BYTES={'3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986','230184f60bae2feaf244f10a8bac053c8ff33a183bcc365b4d8b876d2b7f4809'}
LICENSE_DECLARATION='https://github.com/s116821/RemarkableBuddiesDocs/blob/d34c14d5d25eee671f7441a69f4feef7bd2c5d69/docs/licensing.md'
IMAGES={
 'armv7-unknown-linux-gnueabihf':'ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf@sha256:1ab143450cd888657baa3563ad663203bc5887629e338ef77718ef515d56fb7a',
 'aarch64-unknown-linux-gnu':'ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:99294ae75048aa07d00bb4f1a18a017e0812ff25753768d21d5938bf7562b40a',
}

def run(argv,cwd,env=None,timeout=120,stdout_limit=16*1024*1024,combined_limit=None):
 process=subprocess.Popen(argv,cwd=cwd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 events=queue.Queue(maxsize=8)
 stopped=threading.Event()
 def deliver(item):
  while not stopped.is_set():
   try: events.put(item,timeout=0.1);return
   except queue.Full: continue
 def drain(stream,channel):
  try:
   while True:
    block=stream.read(4096)
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
  require(process.wait(timeout=max(0.001,deadline-time.monotonic()))==0,'actual build/tool command failed')
  return buffers[0].decode('utf-8').strip()
 finally:
  stopped.set()
  if process.poll() is None: process.kill()
  process.wait(timeout=5)
  for reader in readers: reader.join(timeout=1)

def git(repo,*args): return run(['git',*args],repo)
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
 data=json.loads(run(['cargo','metadata','--locked','--format-version','1','--filter-platform',target],repo))
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
 repo=repo.resolve();target_dir=target_dir.resolve();configuration=cross_configuration.resolve()
 require(target in TARGETS and configuration.is_file(),'build target/configuration')
 require(not target_dir.is_relative_to(repo),'external build output root required')
 config=tomllib.loads(configuration.read_text());image=config['target'][target]['image']
 require(image==IMAGES[target],'reviewed immutable build image')
 configuration_hash=sha(configuration)
 cross=Path(shutil.which('cross') or '')
 require(cross.is_file(),'actual cross executable')
 cross_hash=sha(cross)
 private_cargo=[]
 before_source=source_identity(repo,tag)
 license_id=source_license(repo)
 configurations=cargo_configuration(repo)
 before_cargo=cargo_identity(repo,target,sdk_tag,private_cargo)
 # Disallow inherited compiler/linker overrides rather than silently attesting them.
 require(all(not os.environ.get(k) for k in OVERRIDES),'unreviewed build override')
 require(not any(value and (key in UNRECORDED_OVERRIDES or key.startswith('CARGO_PROFILE_') or (key.startswith('CARGO_TARGET_') and key.endswith(('_RUSTFLAGS','_LINKER','_RUNNER')))) for key,value in os.environ.items()),'unrecorded compiler/linker/runner override')
 require(not any(k.startswith('VERGEN_') for k in os.environ),'version override')
 env=dict(os.environ,READER_BUDDY_RELEASE_TAG=tag,READER_BUDDY_RELEASE_SHA=before_source['sha'],CROSS_CONFIG=str(configuration),LD_BIND_NOW='1',SOURCE_DATE_EPOCH=str(before_source['epoch']))
 env.update({key:'' for key in OVERRIDES})
 argv=['cross','run','--locked','--release','--target',target,'--target-dir',str(target_dir),'--bin','reader-buddy','--','--version']
 output=run(argv,repo,env)
 require(output.split()[-1:]==[tag[1:]],'actual target runtime tag version')
 require(source_identity(repo,tag)==before_source and cargo_identity(repo,target,sdk_tag,private_cargo)==before_cargo,'build inputs changed')
 require(cargo_configuration(repo)==configurations,'Cargo configuration changed')
 require(source_license(repo)==license_id,'source license changed')
 require(sha(configuration)==configuration_hash and sha(cross)==cross_hash,'build tools/configuration changed')
 binary=target_dir/target/'release/reader-buddy'
 require(binary.is_file() and not binary.is_symlink() and 1<=binary.stat().st_size<=134217728,'actual build executable')
 cls,machine=elf_header(binary);arch,abi,expected_cls,expected_machine=TARGETS[target]
 require((cls,machine)==(expected_cls,expected_machine),'target ELF correspondence')
 # Full interpreter/ABI admission uses the reviewed readelf helper, not this header alone.
 from abi import qualify
 actual_elf=qualify(binary,target,lambda command:run(command,repo))
 rustc=run(['rustc','--version','--verbose'],repo)
 fields=dict(line.split(': ',1) for line in rustc.splitlines() if ': ' in line)
 sdk=before_cargo['sdk']
 metadata=dict(schema=1,evidence='developer-source-build',eligibility='not-qualified',source=before_source,
  target=dict(rust_target=target,apk_arch=arch,abi=abi,elf_class=cls,elf_machine=machine,elf_endian='little',interpreter=actual_elf['elf']['interpreter']),
  sdk=sdk,cargo={k:before_cargo[k] for k in ['lock_sha256','metadata_identity_sha256','features']},
  toolchain=dict(rustc_release=fields['release'],rustc_commit=None if fields.get('commit-hash')=='unknown' else fields.get('commit-hash'),rustc_host=fields['host'],cross_version=run(['cross','--version'],repo),cross_binary_sha256=sha(cross),image_digest='sha256:'+image.split('@sha256:',1)[1],build_configuration_sha256=sha(configuration)),
  build=dict(argv=['cross','run','--locked','--release','--target',target,'--target-dir','TARGET_ROOT','--bin','reader-buddy','--','--version'],environment={k:'' for k in OVERRIDES},profile='legacy-no-sdk' if sdk is None else 'unqualified-development',source_build_helper_sha256=sha(Path(__file__))),
  binary=dict(path='usr/bin/reader-buddy',sha256=sha(binary),bytes=binary.stat().st_size,mode=493))
 from apk_contract import canonical
 canonical(metadata)
 private_evidence=dict(source_root=str(repo),target_root=str(target_dir),configuration_path=str(configuration),cargo_configuration_digests=configurations,actual_argv=argv,recorded_environment={key:env[key] for key in [*OVERRIDES,'READER_BUDDY_RELEASE_TAG','READER_BUDDY_RELEASE_SHA','CROSS_CONFIG','LD_BIND_NOW','SOURCE_DATE_EPOCH']},cargo_observations=private_cargo,rustc_verbose=rustc,elf_observation=actual_elf,runtime_version_output=output,source_license=license_id,license_basis=dict(project_declaration=LICENSE_DECLARATION,committed_blob=LICENSE_BLOB,actual_file_sha256=sha(repo/'LICENSE'),existing_detected_identifier='GPL-3.0',classifier_is_owner_grant=False))
 return binary,metadata,private_evidence
