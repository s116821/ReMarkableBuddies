"""Build both actual tagged targets, then generate offline test-key APK artifacts."""
import argparse,hashlib,json,os,shutil,tempfile
from pathlib import Path
from apk_contract import canonical,require,integer
from apk_source_build import build,run,sha,source_identity,source_license
from docker_owned import phase

IMAGE='sha256:8d79948df2c7c568a043683f786e7ce4ad2e25e469673b72b8da8d2dbb2fa9ac'
TOOL_SHA='7488a9650411143a205bf80af511203fadd79499e21613c970b4ef8c207fe11a'
TARGETS=[('armv7','armv7-unknown-linux-gnueabihf'),('aarch64','aarch64-unknown-linux-gnu')]
MODULES=['apk_generate.py','apk_observe.py','apk_contract.py','apk_source_build.py','apk_index.py','process_guard.py','docker_owned.py','apk_compiler.py']

def copy_verified(source,destination,expected_bytes,expected_sha):
 require(source.is_file() and not source.is_symlink(),'regular generated input')
 with source.open('rb') as stream,destination.open('xb') as output:
  before=os.fstat(stream.fileno());require(before.st_size==expected_bytes,'input byte count')
  digest=hashlib.sha256();remaining=expected_bytes
  while remaining:
   block=stream.read(min(65536,remaining));require(block,'short input')
   digest.update(block);output.write(block);remaining-=len(block)
  after=os.fstat(stream.fileno())
  require((before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns),'input changed while copying')
  require(digest.hexdigest()==expected_sha,'actual input hash')
 require(sha(destination)==expected_sha,'staged input hash')

def produce(args):
 source=args.source.resolve();target_root=args.target_dir.resolve();output=args.output.resolve()
 require(integer(args.pkgrel,0,65535),'pkgrel')
 require(not output.exists() and not target_root.exists() and not output.is_relative_to(source) and not target_root.is_relative_to(source),'new external output roots')
 identity=source_identity(source,args.tag);license_id=source_license(source)
 tool=args.apk_tool.resolve();require(tool.name=='apk' and tool.is_file() and sha(tool)==TOOL_SHA,'pinned APK tool')
 require(run(['docker','image','inspect',IMAGE,'--format','{{.Id}}'],source)==IMAGE,'existing pinned generation image')
 target_root.mkdir(exist_ok=False)
 # Preserve failed or incomplete builds as private working evidence. Only the
 # final output has a completed two-target artifact receipt.
 work=Path(tempfile.mkdtemp(prefix='buddy-apk-work-',dir=output.parent))
 inputs=work/'inputs';inputs.mkdir();artifacts=work/'artifacts';artifacts.mkdir();modules=work/'modules';modules.mkdir()
 for name in MODULES:shutil.copyfile(Path(__file__).with_name(name),modules/name)
 private={};input_hashes={}
 for arch,target in TARGETS:
  binary,metadata,evidence=build(source,args.tag,target,target_root/arch,Path(__file__).with_name('Cross.apk.toml'),args.sdk_tag)
  require(metadata['source']==identity,'source unchanged across targets')
  directory=inputs/arch;directory.mkdir()
  copy_verified(binary,directory/'reader-buddy',metadata['binary']['bytes'],metadata['binary']['sha256'])
  license_file=source/'LICENSE';copy_verified(license_file,directory/'LICENSE',license_file.stat().st_size,sha(license_file))
  with (directory/'build.json').open('xb') as stream:stream.write(canonical(metadata))
  private[arch]=evidence
  for path in directory.iterdir():input_hashes[str(path.relative_to(inputs))]=sha(path)
 with (inputs/'input-set.json').open('x',encoding='ascii',newline='\n') as stream:
  json.dump(dict(pkgrel=args.pkgrel,source_license=license_id,targets=[arch for arch,_ in TARGETS]),stream,sort_keys=True,separators=(',',':'));stream.write('\n')
 input_hashes['input-set.json']=sha(inputs/'input-set.json')
 with (work/'private-source-build-evidence.json').open('x',encoding='utf-8') as stream:json.dump(private,stream,sort_keys=True)
 command=['--rm','--network','none','--cap-drop','ALL','--read-only','--memory','1536m','--pids-limit','64','--tmpfs','/tmp:rw,nosuid,noexec,size=768m',
  '--mount',f'type=bind,source={inputs},target=/inputs,readonly','--mount',f'type=bind,source={artifacts},target=/output',
  '--mount',f'type=bind,source={modules},target=/modules,readonly','--mount',f'type=bind,source={tool.parent},target=/tool,readonly',
  '--entrypoint','/usr/bin/python3',IMAGE,'/modules/apk_generate.py']
 result=json.loads(phase(command,source,run,timeout=120,stdout_limit=65536,combined_limit=None))
 require(source_identity(source,args.tag)==identity and source_license(source)==license_id,'source changed before output')
 require(all(sha(inputs/path)==digest for path,digest in input_hashes.items()),'internal inputs changed during generation')
 require(sha(tool)==TOOL_SHA,'APK tool changed during generation')
 version=args.tag[1:]+'-r'+str(args.pkgrel)
 names={f'remarkable-buddies-{version}.{arch}.apk' for arch,_ in TARGETS}|{'buddy-index.adb','fixture.pub'}
 require({path.name for path in artifacts.iterdir()}==names,'exact completed artifact set')
 require(result['status']=='generation-self-checks-only' and result['eligibility']=='not-qualified' and result['private_key_exported'] is False,'generation result')
 require(set(result['artifact_files'])==names,'generation artifact observations')
 observations=work/'observations';observations.mkdir()
 with (observations/'generated-control.json').open('x',encoding='ascii') as stream:json.dump(result,stream,sort_keys=True)
 # Generation has returned, its private temporary key has been disposed, and
 # its owned container absence is confirmed before this separate phase.
 # Pass the pin via the generation control channel, never from adjacent key
 # bytes or signed package metadata. This remains local fixture trust only.
 observation_command=['--rm','--network','none','--cap-drop','ALL','--read-only','--memory','768m','--pids-limit','32','--tmpfs','/tmp:rw,nosuid,noexec,size=32m',
  '--mount',f'type=bind,source={inputs},target=/inputs,readonly','--mount',f'type=bind,source={artifacts},target=/artifacts,readonly',
  '--mount',f'type=bind,source={observations},target=/observations,readonly','--mount',f'type=bind,source={modules},target=/modules,readonly',
  '--mount',f'type=bind,source={tool.parent},target=/tool,readonly','--entrypoint','/usr/bin/python3',IMAGE,
  '/modules/apk_observe.py','--expected-spki',result['fixture_public_key_spki_sha256']]
 observation=json.loads(phase(observation_command,source,run,timeout=120))
 require(observation['status']=='public-key-only-observed' and observation['private_key_available'] is False,'independent public-key-only observation required')
 output.mkdir(exist_ok=False)
 published={}
 for name in sorted(names):
  path=artifacts/name;size=result['artifact_files'][name]['bytes'];digest=result['artifact_files'][name]['sha256']
  require(0<size<=268435456,'artifact bound')
  copy_verified(path,output/name,size,digest);published[name]=dict(bytes=size,sha256=digest)
 # This exclusive receipt is the completion marker; partial outputs without
 # it remain unfinished. Future consumers must revalidate actual bytes.
 receipt=dict(status='test-key-source-build-only',eligibility='not-qualified',source=identity,artifacts=published,
  fixture_public_key_spki_sha256=result['fixture_public_key_spki_sha256'],generation_observations=result,public_key_observation=observation)
 with (output/'test-key-build-receipt.json').open('x',encoding='ascii',newline='\n') as stream:json.dump(receipt,stream,sort_keys=True);stream.write('\n')
 return receipt

if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('--source',type=Path,required=True);parser.add_argument('--tag',required=True)
 parser.add_argument('--pkgrel',type=int,default=0);parser.add_argument('--sdk-tag')
 parser.add_argument('--target-dir',type=Path,required=True);parser.add_argument('--output',type=Path,required=True)
 parser.add_argument('--apk-tool',type=Path,required=True)
 print(json.dumps(produce(parser.parse_args()),sort_keys=True))
