"""Separate public-key-only observation after the generation container ends."""
import argparse,json,shutil,tempfile
from pathlib import Path
from apk_contract import HEX64, RM2_BUNDLE_SHA, RM2_FIRMWARE, RM2_PROFILE, dependencies, parse_build, require
from apk_generate import inventory,bounded_file,TOOL_SHA
from apk_index import FILENAME_PATTERN, compare, parse_dump
from apk_source_build import run,sha
from apk_payload import prepare,selected_bundle

def observe(expected_spki):
 require(isinstance(expected_spki,str) and HEX64.fullmatch(expected_spki),'externally supplied fixture SPKI pin required')
 artifacts=Path('/artifacts');inputs=Path('/inputs');tool=Path('/tool/apk')
 require(sha(tool)==TOOL_SHA,'pinned actual observation tool')
 control=parse_dump(bounded_file(Path('/observations/generated-control.json'),65536))
 require(control['status']=='generation-self-checks-only' and control['private_key_exported'] is False,'completed generation control receipt')
 require(control['fixture_public_key_spki_sha256']==expected_spki,'control pin differs from independently supplied observation pin')
 require({path.name for path in artifacts.iterdir()}==set(control['artifact_files']),'fixed immutable observation inventory')
 for name,expected in control['artifact_files'].items():
  path=artifacts/name
  require(path.is_file() and not path.is_symlink() and path.stat().st_size==expected['bytes'] and sha(path)==expected['sha256'],'artifact differs from immutable generation')
 manifest=parse_dump(bounded_file(inputs/'input-set.json',4096));builds={};licenses={};sizes={}
 packages=[];resident=None
 with tempfile.TemporaryDirectory(prefix='apk-public-observer-',dir='/tmp') as directory:
  root=Path(directory);meta=root/'meta';(meta/'keys').mkdir(parents=True)
  public=meta/'keys/fixture.pub';shutil.copyfile(artifacts/'fixture.pub',public)
  spki=root/'spki.der'
  env={'PATH':'/usr/bin:/bin','HOME':'/tmp','TMPDIR':'/tmp','LC_ALL':'C'}
  def invoke(argv,metadata=False):return run(argv,root,env,timeout=120,stdout_limit=65536 if metadata else 16384,combined_limit=None if metadata else 16384)
  invoke(['openssl','pkey','-pubin','-in',str(public),'-outform','DER','-out',str(spki)])
  require(spki.stat().st_size<=8192 and sha(spki)==expected_spki,'actual public key does not match external SPKI pin')
  common=[str(tool),'--root',str(meta),'--keys-dir','keys','--no-network','--no-cache','--no-logfile']
  for arch in ['armv7','aarch64']:
   build=parse_build(bounded_file(inputs/arch/'build.json',65536));builds[arch]=build
   version=build['source']['tag'][1:]+'-r'+str(manifest['pkgrel'])
   package=artifacts/f'remarkable-buddies-{version}.{arch}.apk'
   invoke(common+['verify',str(package)])
   dump=parse_dump(invoke(common+['adbdump','--format','json',str(package)],metadata=True).encode())
   expected,layout=prepare(inputs/arch,build,selected_bundle(inputs,build),root/('expected-'+arch))
   require(expected==control['packages'][arch]['payload'],'generation differs from frozen payload inputs')
   inventory(dump,expected,build['source']['epoch'],layout,dependencies(build))
   licenses[arch]=manifest['source_license'];sizes[arch]=package.stat().st_size
   packages.append(package)
   if build['build']['profile']==RM2_PROFILE:
    recovery_path='home/root/.local/lib/remarkable-buddies/resident-recovery.json'
    resident={
     'name': 'remarkable-buddies', 'version': version, 'apk_sha256': sha(package), 'apk_bytes': sizes[arch],
     'recovery_profile_sha256': expected[recovery_path]['sha256'],
     'source_sha': build['source']['sha'], 'sdk_sha': build['sdk']['sha'],
     'native_bundle_sha256': RM2_BUNDLE_SHA, 'arch': arch, 'model': 'rm2', 'firmware': RM2_FIRMWARE,
    }
  index=artifacts/'buddy-index.adb';invoke(common+['verify',str(index)])
  raw=invoke(common+['adbdump','--format','json',str(index)],metadata=True).encode()
  # Compute the comparison from these verified APKs in the public-only process.
  comparison=root/'comparison-index.adb'
  invoke(common+['mkndx','--hash','sha256','--pkgname-spec',FILENAME_PATTERN,'--compression','none',
                 '--output',str(comparison),*[str(package) for package in packages]])
  readback=invoke(common+['adbdump','--format','json',str(comparison)],metadata=True).encode()
  binding=compare(raw,readback,builds,manifest['pkgrel'],licenses,sizes)
  if resident is not None:
   resident.update(index_sha256=sha(index),index_metadata_sha256=binding['armv7']['index_metadata_hash'])
  for name,expected in control['artifact_files'].items():
   require(sha(artifacts/name)==expected['sha256'],'immutable artifact changed during observation')
  return dict(status='public-key-only-observed',eligibility='not-qualified',fixture_public_key_spki_sha256=expected_spki,private_key_available=False,pin_basis='separately-supplied-generation-control-channel',signature_and_payload_verified=True,resident_package=resident)

if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--expected-spki',required=True)
 print(json.dumps(observe(parser.parse_args().expected_spki),sort_keys=True))
