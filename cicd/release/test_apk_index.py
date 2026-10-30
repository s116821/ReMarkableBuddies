"""Synthetic comparison controls, not signed-byte package qualification."""
import copy,json,unittest
from apk_contract import Refusal,tags
from apk_index import compare,FILENAME_PATTERN
from test_apk_contract import specimen

class IndexBinding(unittest.TestCase):
 def setUp(self):
  arm=specimen();wide=copy.deepcopy(arm)
  wide['target'].update(rust_target='aarch64-unknown-linux-gnu',apk_arch='aarch64',abi='gnu-aarch64',elf_class=64,elf_machine='AArch64',interpreter='/lib/ld-linux-aarch64.so.1')
  self.builds={'armv7':arm,'aarch64':wide};self.licenses={arch:'GPL-3.0-only' for arch in self.builds};self.sizes={arch:1234 for arch in self.builds}
  self.doc={'pkgname-spec':FILENAME_PATTERN,'packages':[dict(name='remarkable-buddies',origin='remarkable-buddies',version='1.2.3-r2',arch=arch,
   **{'repo-commit':'1'*40,'build-time':1,'license':self.licenses[arch],'description':'ReMarkableBuddies test-key source-build package',
      'tags':tags(build,2),'hashes':('a' if arch=='armv7' else 'b')*64,'file-size':1234,'installed-size':4096}) for arch,build in self.builds.items()]}
 def check(self,original,regenerated):
  return compare(json.dumps(original).encode(),json.dumps(regenerated).encode(),self.builds,2,self.licenses,self.sizes)
 def test_two_architectures(self):
  result=self.check(self.doc,self.doc);self.assertEqual(set(result),set(self.builds))
 def test_replacement_metadata_identity(self):
  changed=copy.deepcopy(self.doc);changed['packages'][0]['hashes']='c'*64
  with self.assertRaisesRegex(Refusal,'differs'):self.check(self.doc,changed)
 def test_duplicate_and_missing_arch(self):
  changed=copy.deepcopy(self.doc);changed['packages'][1]=changed['packages'][0]
  with self.assertRaises(Refusal):self.check(changed,changed)
 def test_matching_caller_fields_do_not_override_build(self):
  changed=copy.deepcopy(self.doc);changed['packages'][0]['tags'][0]='remarkable-buddies:abi=wrong'
  with self.assertRaises(Refusal):self.check(changed,changed)
 def test_file_hash_shape_and_epoch_type(self):
  for field,value in [('hashes','a'*40),('build-time',1.0)]:
   changed=copy.deepcopy(self.doc);changed['packages'][0][field]=value
   with self.subTest(field=field),self.assertRaises(Refusal):self.check(changed,changed)
 def test_missing_or_wrong_filename_pattern(self):
  for pattern in [None,'${name}-${version}.apk']:
   changed=copy.deepcopy(self.doc)
   if pattern is None:del changed['pkgname-spec']
   else:changed['pkgname-spec']=pattern
   with self.subTest(pattern=pattern),self.assertRaises(Refusal):self.check(changed,changed)

if __name__=='__main__':unittest.main()
