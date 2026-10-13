"""Linux-only pinned-tool metadata schema probe; no source/signature admission."""
import copy,hashlib,json,os,subprocess,tempfile,unittest,sys
from pathlib import Path
from apk_contract import Refusal
if sys.platform=='linux':
 from apk_generate import inventory,TOOL_SHA,FILES

@unittest.skipUnless(sys.platform=='linux','Pinned APK tool probe runs in the offline Linux container')
class ActualSchema(unittest.TestCase):
 def test_signed_standard_filename_pattern(self):
  tool=Path('/tool/apk');self.assertEqual(hashlib.sha256(tool.read_bytes()).hexdigest(),TOOL_SHA)
  with tempfile.TemporaryDirectory(dir='/tmp') as directory:
   root=Path(directory);meta=root/'meta';(meta/'keys').mkdir(parents=True)
   key=root/'fixture.pem';public=meta/'keys/fixture.pub'
   def command(args):
    result=subprocess.run(args,capture_output=True,timeout=15,check=True)
    self.assertLess(len(result.stdout)+len(result.stderr),65536)
    return result.stdout
   command(['openssl','genrsa','-out',str(key),'2048'])
   command(['openssl','rsa','-in',str(key),'-pubout','-out',str(public)])
   common=[str(tool),'--root',str(meta),'--keys-dir','keys','--no-network']
   package=root/'filename-schema-probe-1.0.0-r0.armv7.apk'
   command(common+['mkpkg','--sign-key',str(key),'--compression','none','--info','name:filename-schema-probe','--info','version:1.0.0-r0','--info','arch:armv7','--output',str(package)])
   command(common+['verify',str(package)])
   index=root/'schema-index.adb';pattern='${name}-${version}.${arch}.apk'
   command(common+['mkndx','--hash','sha256','--pkgname-spec',pattern,'--sign-key',str(key),'--compression','none','--output',str(index),str(package)])
   command(common+['verify',str(index)])
   dump=json.loads(command(common+['adbdump','--format','json',str(index)]))
   self.assertEqual(set(dump),{'packages','pkgname-spec'})
   self.assertEqual(dump['pkgname-spec'],pattern)
   self.assertEqual(len(dump['packages']),1)
   self.assertEqual(dump['packages'][0]['file-size'],package.stat().st_size)
   self.assertEqual(len(dump['packages'][0]['hashes']),64)

 def test_fixed_inventory_and_tampering(self):
  tool=Path('/tool/apk');self.assertEqual(hashlib.sha256(tool.read_bytes()).hexdigest(),TOOL_SHA)
  with tempfile.TemporaryDirectory(dir='/tmp') as directory:
   root=Path(directory);tree=root/'files';tree.mkdir();expected={}
   for path,mode in FILES.items():
    file=tree/path;file.parent.mkdir(parents=True,exist_ok=True)
    data=('synthetic metadata-only '+path+'\n').encode();file.write_bytes(data);os.chmod(file,mode);os.utime(file,(1,1))
    expected[path]=dict(bytes=len(data),sha256=hashlib.sha256(data).hexdigest())
   for file in tree.rglob('*'):
    if file.is_dir():os.chmod(file,493);os.utime(file,(1,1))
   package=root/'unsigned-schema-probe.apk'
   environment={'PATH':'/usr/bin:/bin','LC_ALL':'C','SOURCE_DATE_EPOCH':'1'}
   subprocess.run([str(tool),'--no-network','mkpkg','--compat','3.0.0_pre3','--compression','none','--files',str(tree),'--no-xattrs','--info','name:unsigned-schema-probe','--info','version:1.0.0-r0','--info','arch:armv7','--output',str(package)],env=environment,capture_output=True,timeout=15,check=True)
   raw=subprocess.run([str(tool),'adbdump','--format','json',str(package)],env=environment,capture_output=True,timeout=15,check=True).stdout
   self.assertLess(len(raw),65536);dump=json.loads(raw)
   inventory(dump,expected,1)
   changed=copy.deepcopy(dump);changed['paths'].append(changed['paths'][0])
   with self.assertRaises(Refusal):inventory(changed,expected,1)
   changed=copy.deepcopy(dump);changed['scripts']={}
   with self.assertRaises(Refusal):inventory(changed,expected,1)
   changed=copy.deepcopy(expected);changed['usr/bin/reader-buddy']['sha256']='0'*64
   with self.assertRaises(Refusal):inventory(dump,changed,1)

if __name__=='__main__':unittest.main()
