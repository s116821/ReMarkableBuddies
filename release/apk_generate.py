"""Internal offline container phase; inputs come only from the source builder."""
import hashlib,json,os,resource,shutil,tempfile
from pathlib import Path
from apk_contract import canonical,parse_build,require,tags
from apk_source_build import run,sha
from apk_index import compare,parse_dump,FILENAME_PATTERN

TOOL_SHA='7488a9650411143a205bf80af511203fadd79499e21613c970b4ef8c207fe11a'
INPUT=Path('/inputs');OUTPUT=Path('/output');TOOL=Path('/tool/apk')
FILES={'usr/bin/reader-buddy':493,'usr/share/remarkable-buddies/LICENSE':420,'usr/share/remarkable-buddies/build.json':420}
DIRECTORIES={'usr','usr/bin','usr/share','usr/share/remarkable-buddies'}

def bounded_file(path,cap):
 require(path.is_file() and not path.is_symlink(),'regular bounded internal input')
 with path.open('rb') as stream:
  size=os.fstat(stream.fileno()).st_size;require(0<size<=cap,'internal input bound')
  raw=stream.read(size)
  require(len(raw)==size and os.fstat(stream.fileno()).st_size==size,'internal input changed')
  return raw

def inventory(dump,expected,epoch):
 require(isinstance(dump,dict) and set(dump)=={'info','paths'},'package has no scripts/triggers/unknown members')
 directories=set();files={}
 for directory in dump['paths']:
  require(set(directory)<= {'name','acl','files'} and {'name','acl'}<=set(directory),'directory fields')
  name=directory['name'];require(name in DIRECTORIES and name not in directories,'fixed package directories')
  directories.add(name);acl=directory['acl']
  require(acl==dict(mode=493,user='root',group='root'),'directory ownership/mode/xattrs')
  for item in directory.get('files',[]):
   require(set(item)=={'name','acl','size','mtime','hash'},'regular file fields only')
   path=name+'/'+item['name'];require(path in FILES and path not in files,'fixed regular file inventory')
   require(item['acl']==dict(mode=FILES[path],user='root',group='root') and type(item['mtime']) is int and item['mtime']==epoch,'file ownership/mode/time')
   observed=expected[path]
   require(type(item['size']) is int and item['size']==observed['bytes'] and item['hash']==observed['sha256'],'signed payload hash/size')
   files[path]=item
 require(directories==DIRECTORIES and set(files)==set(FILES),'complete package inventory')

def generate():
 require(sha(TOOL)==TOOL_SHA,'pinned actual APK tool')
 require(not list(OUTPUT.iterdir()),'new empty staged output')
 resource.setrlimit(resource.RLIMIT_FSIZE,(268435456,268435456))
 raw=bounded_file(INPUT/'input-set.json',4096)
 manifest=parse_dump(raw);require(set(manifest)=={'pkgrel','source_license','targets'},'internal input-set members')
 require(manifest['targets']==['armv7','aarch64'],'both input architectures')
 pkgrel=manifest['pkgrel'];license_id=manifest['source_license']
 require(license_id in {'GPL-3.0','GPL-3.0-only'},'reviewed existing source SPDX license')
 builds={};sizes={};licenses={};packages=[];observed={}
 with tempfile.TemporaryDirectory(prefix='apk-generation-',dir='/tmp') as temporary:
  temporary=Path(temporary);meta=temporary/'meta';(meta/'keys').mkdir(parents=True)
  (meta/'config').write_text('');(meta/'repositories').write_text('')
  private=temporary/'fixture.pem';public=meta/'keys/fixture.pub'
  env={'PATH':'/usr/bin:/bin','LC_ALL':'C','HOME':'/tmp','TMPDIR':'/tmp','APK_CONFIG':str(meta/'config')}
  def invoke(argv,metadata=False):return run(argv,temporary,env,timeout=120,stdout_limit=65536 if metadata else 16384,combined_limit=None if metadata else 16384)
  invoke(['openssl','genrsa','-out',str(private),'2048'])
  invoke(['openssl','rsa','-in',str(private),'-pubout','-out',str(public)])
  spki=temporary/'spki.der'
  invoke(['openssl','pkey','-pubin','-in',str(public),'-outform','DER','-out',str(spki)])
  require(spki.stat().st_size<=8192,'fixture SPKI bound');fingerprint=sha(spki)
  common=[str(TOOL),'--root',str(meta),'--keys-dir','keys','--no-network','--no-cache','--no-logfile','--repositories-file',str(meta/'repositories')]
  for arch in manifest['targets']:
   source=INPUT/arch;build=parse_build(bounded_file(source/'build.json',65536))
   require(build['target']['apk_arch']==arch,'input architecture');builds[arch]=build;licenses[arch]=license_id
   epoch=build['source']['epoch'];env['SOURCE_DATE_EPOCH']=str(epoch)
   if arch=='aarch64':require(build['source']==builds['armv7']['source'],'same two-target source identity')
   tree=temporary/('payload-'+arch);tree.mkdir();os.chmod(tree,493)
   expected={}
   for path,mode in FILES.items():
    name={'usr/bin/reader-buddy':'reader-buddy','usr/share/remarkable-buddies/LICENSE':'LICENSE','usr/share/remarkable-buddies/build.json':'build.json'}[path]
    origin=source/name;require(origin.is_file() and not origin.is_symlink(),'regular internal build input')
    require(0<origin.stat().st_size<=(134217728 if name=='reader-buddy' else 65536),'input file bound')
    destination=tree/path;destination.parent.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(origin,destination);os.chmod(destination,mode);os.utime(destination,(epoch,epoch))
    expected[path]=dict(bytes=destination.stat().st_size,sha256=sha(destination))
   require(expected['usr/bin/reader-buddy']==dict(bytes=build['binary']['bytes'],sha256=build['binary']['sha256']),'actual executable identity')
   require((tree/'usr/share/remarkable-buddies/build.json').read_bytes()==canonical(build),'generated canonical metadata')
   for directory in tree.rglob('*'):
    if directory.is_dir():os.chmod(directory,493);os.utime(directory,(epoch,epoch))
   version=build['source']['tag'][1:]+'-r'+str(pkgrel);package=temporary/f'remarkable-buddies-{version}.{arch}.apk'
   info={'name':'remarkable-buddies','version':version,'arch':arch,'origin':'remarkable-buddies','repo-commit':build['source']['sha'],'build-time':str(epoch),'license':license_id,'description':'ReMarkableBuddies test-key source-build package','tags':' '.join(tags(build,pkgrel))}
   argv=common+['mkpkg','--compat','3.0.0_pre3','--compression','none','--sign-key',str(private),'--files',str(tree),'--output',str(package),'--no-xattrs']
   for key,value in info.items():argv+=['--info',key+':'+value]
   require(package.name==FILENAME_PATTERN.replace('${name}',info['name']).replace('${version}',version).replace('${arch}',arch),'exact actual signed-index filename mapping')
   invoke(argv);invoke(common+['verify',str(package)])
   dump=invoke(common+['adbdump','--format','json',str(package)],metadata=True).encode();inventory(parse_dump(dump),expected,epoch)
   sizes[arch]=package.stat().st_size;packages.append(package)
   observed[arch]=dict(apk_file_sha256=sha(package),apk_file_bytes=sizes[arch],payload=expected)
   if arch=='aarch64':require(expected['usr/share/remarkable-buddies/LICENSE']==observed['armv7']['payload']['usr/share/remarkable-buddies/LICENSE'],'same source license payload')
  indexes=[];readbacks=[]
  for filename in ['buddy-index.adb','comparison-index.adb']:
   index=temporary/filename
   invoke(common+['mkndx','--hash','sha256','--pkgname-spec',FILENAME_PATTERN,'--compression','none','--sign-key',str(private),'--output',str(index),*[str(p) for p in packages]])
   invoke(common+['verify',str(index)])
   readbacks.append(invoke(common+['adbdump','--format','json',str(index)],metadata=True).encode());indexes.append(index)
  binding=compare(*readbacks,builds,pkgrel,licenses,sizes)
  for arch in observed:observed[arch].update(binding[arch])
  for arch,package in zip(manifest['targets'],packages):
   require(sha(package)==observed[arch]['apk_file_sha256'] and package.stat().st_size==observed[arch]['apk_file_bytes'],'APK changed during index verification')
  exported={}
  for path in [*packages,indexes[0],public]:
   expected_hash=sha(path);expected_size=path.stat().st_size
   destination=OUTPUT/path.name
   with path.open('rb') as source,destination.open('xb') as output:shutil.copyfileobj(source,output,65536)
   require(destination.stat().st_size==expected_size and sha(destination)==expected_hash,'exported artifact identity')
   exported[path.name]=dict(bytes=expected_size,sha256=expected_hash)
  return dict(status='test-key-source-build-only',eligibility='not-qualified',fixture_public_key_spki_sha256=fingerprint,packages=observed,index_sha256=sha(indexes[0]),artifact_files=exported,private_key_exported=False)

if __name__=='__main__':print(json.dumps(generate(),sort_keys=True))
