"""Synthetic trust-boundary controls only; never real source-build admission."""
import copy,hashlib,json,os,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
from apk_contract import canonical,Refusal
from test_apk_contract import specimen
if sys.platform=='linux':
 import apk_generate,apk_observe

@unittest.skipUnless(sys.platform=='linux' and Path('/tool/apk').is_file(),'Requires explicit offline pinned-tool fixture container')
class PublicObservation(unittest.TestCase):
 def test_disposed_generation_key_external_pin_and_immutable_files(self):
  # All executable bytes and build observations here are visibly synthetic.
  # The source-build entry point is never invoked or bypassed in a deliverable.
  with tempfile.TemporaryDirectory(dir='/tmp') as directory:
   root=Path(directory);inputs=root/'inputs';inputs.mkdir();artifacts=root/'artifacts';artifacts.mkdir();observations=root/'observations';observations.mkdir()
   builds={}
   for arch in ['armv7','aarch64']:
    source=inputs/arch;source.mkdir();value=specimen()
    if arch=='aarch64':value['target'].update(rust_target='aarch64-unknown-linux-gnu',apk_arch='aarch64',abi='gnu-aarch64',elf_class=64,elf_machine='AArch64',interpreter='/lib/ld-linux-aarch64.so.1')
    data=('synthetic trust control '+arch+'\n').encode()
    value['binary'].update(bytes=len(data),sha256=hashlib.sha256(data).hexdigest())
    (source/'reader-buddy').write_bytes(data);(source/'LICENSE').write_bytes(b'synthetic signature-control license\n');(source/'build.json').write_bytes(canonical(value));builds[arch]=value
   (inputs/'input-set.json').write_text(json.dumps(dict(pkgrel=2,source_license='GPL-3.0-only',targets=['armv7','aarch64'])))
   with patch.object(apk_generate,'INPUT',inputs),patch.object(apk_generate,'OUTPUT',artifacts):control=apk_generate.generate()
   pin=control['fixture_public_key_spki_sha256']
   self.assertFalse(list(root.rglob('*.pem')),'private generation key escaped volatile temporary root')
   (observations/'generated-control.json').write_text(json.dumps(control))
   # The observer is a distinct call with only public artifact/control inputs.
   actual=Path
   def mapped(value):
    return {'/artifacts':artifacts,'/inputs':inputs,'/observations/generated-control.json':observations/'generated-control.json'}.get(value,actual(value))
   with patch.object(apk_observe,'Path',side_effect=mapped):
    result=apk_observe.observe(pin);self.assertEqual(result['status'],'public-key-only-observed')
    with self.assertRaisesRegex(Refusal,'pin'):apk_observe.observe('0'*64)
    package=artifacts/'remarkable-buddies-1.2.3-r2.armv7.apk';raw=package.read_bytes();package.write_bytes(raw[:-1])
    with self.assertRaisesRegex(Refusal,'immutable generation'):apk_observe.observe(pin)

if __name__=='__main__':unittest.main()
