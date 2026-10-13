"""Separate public-key-only observation after the generation container ends."""
import argparse,json,shutil,tempfile
from pathlib import Path
from apk_contract import HEX64,parse_build,require
from apk_generate import inventory,bounded_file,TOOL_SHA
from apk_index import compare,parse_dump
from apk_source_build import run,sha

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
   expected=control['packages'][arch]['payload']
   inventory(dump,expected,build['source']['epoch'])
   require(expected['usr/bin/reader-buddy']==dict(bytes=build['binary']['bytes'],sha256=build['binary']['sha256']),'verified payload/source-build identity')
   licenses[arch]=manifest['source_license'];sizes[arch]=package.stat().st_size
  index=artifacts/'buddy-index.adb';invoke(common+['verify',str(index)])
  raw=invoke(common+['adbdump','--format','json',str(index)],metadata=True).encode()
  compare(raw,control['regenerated_index_readback'].encode(),builds,manifest['pkgrel'],licenses,sizes)
  for name,expected in control['artifact_files'].items():
   require(sha(artifacts/name)==expected['sha256'],'immutable artifact changed during observation')
  return dict(status='public-key-only-observed',eligibility='not-qualified',fixture_public_key_spki_sha256=expected_spki,private_key_available=False,pin_basis='separately-supplied-generation-control-channel',signature_and_payload_verified=True)

if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--expected-spki',required=True)
 print(json.dumps(observe(parser.parse_args().expected_spki),sort_keys=True))
