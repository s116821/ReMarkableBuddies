"""Synthetic parser controls, never executable/source-build qualification."""
import copy,json,unittest
from apk_contract import (RM2_BINARY,RM2_BUNDLE_SHA,RM2_FEATURE,RM2_PROFILE,RM2_SDK_SHA,
                          SDK_REPOSITORY,canonical,dependencies,parse_build,tags,verify_tags,
                          Refusal,REPOSITORY)

def specimen():
 return dict(schema=1,evidence='developer-source-build',eligibility='not-qualified',
  source=dict(repository=REPOSITORY,tag='v1.2.3',sha='1'*40,epoch=1),
  target=dict(rust_target='armv7-unknown-linux-gnueabihf',apk_arch='armv7',abi='gnu-arm-eabihf',elf_class=32,elf_machine='ARM',elf_endian='little',interpreter='/lib/ld-linux-armhf.so.3'),
  sdk=None,cargo=dict(lock_sha256='2'*64,metadata_identity_sha256='3'*64,features=[]),
  toolchain=dict(rustc_release='1.98.1',rustc_commit=None,rustc_host='x86_64-pc-windows-msvc',cross_version='cross test',cross_binary_sha256='4'*64,image_digest='sha256:'+'5'*64,build_configuration_sha256='6'*64),
  build=dict(argv=['cross','run'],environment={key:'' for key in ['RUSTFLAGS','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS']},profile='legacy-no-sdk',source_build_helper_sha256='7'*64),
  binary=dict(path='usr/bin/reader-buddy',sha256='8'*64,bytes=1,mode=493))

class Serialization(unittest.TestCase):
 def test_exact_roundtrip(self):
  value=specimen();self.assertEqual(parse_build(canonical(value)),value)
 def test_noncanonical_and_duplicate(self):
  raw=canonical(specimen())
  for changed in [raw.replace(b'"schema":1',b'"schema":1,"schema":1'),raw.replace(b'"schema":1',b'"schema":1.0'),raw.replace(b'\n',b'\r\n'),b' '+raw]:
   with self.subTest(changed=changed[-20:]),self.assertRaises(Refusal):parse_build(changed)
 def test_contradictions(self):
  for path,value in [('epoch',True),('tag','v01.2.3')]:
   data=specimen();data['source'][path]=value
   with self.assertRaises(Refusal):canonical(data)
  data=specimen();data['target']['abi']='gnu-aarch64'
  with self.assertRaises(Refusal):canonical(data)
 def test_tag_duplicate_unknown_and_order(self):
  data=specimen();actual=tags(data,2);self.assertEqual(len(actual),12)
  for changed in [actual+[actual[0]],actual+['remarkable-buddies:unknown=1'],list(reversed(actual))]:
   with self.assertRaises(Refusal):verify_tags(changed,data,2)
 def test_sdk_variants(self):
  data=specimen();data['sdk']=dict(ref_kind='commit-pinned',repository='https://github.com/s116821/ReMarkableOpenSDK',sha='9'*40,tag=None,manifest_sha256='a'*64)
  data['build']['profile']='unqualified-development';self.assertEqual(len(tags(data,0)),14)
  data['sdk'].update(ref_kind='tagged-git',tag='v1.0.0');self.assertEqual(len(tags(data,0)),15)
  data['sdk']['tag']=None
  with self.assertRaises(Refusal):canonical(data)

 def test_resident_requires_the_approved_target_feature_and_bundle_identity(self):
  data=specimen()
  data['sdk']=dict(ref_kind='commit-pinned',repository=SDK_REPOSITORY,
                   sha=RM2_SDK_SHA,tag=None,manifest_sha256='a'*64)
  data['cargo']['features']=[RM2_FEATURE]
  data['build']['profile']=RM2_PROFILE
  data['build']['argv']+=['--features',RM2_FEATURE]
  data['binary']['path']=RM2_BINARY
  self.assertEqual(parse_build(canonical(data)),data)
  self.assertEqual(dependencies(data),['remarkable-os=3.28.0.172-r0','rm2'])
  self.assertIn('remarkable-buddies:firmware=3.28.0.172',tags(data,0))
  self.assertIn('remarkable-buddies:sdk-bundle-sha256='+RM2_BUNDLE_SHA,tags(data,0))
  for section,key,wrong in [
    ('sdk','sha','1'*40),('cargo','features',[]),('build','argv',['cross','build']),
    ('binary','path','usr/bin/reader-buddy'),
  ]:
   changed=copy.deepcopy(data);changed[section][key]=wrong
   with self.subTest(section=section,key=key),self.assertRaises(Refusal):canonical(changed)

if __name__=='__main__':unittest.main()
