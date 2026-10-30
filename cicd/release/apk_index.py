"""Compare authenticated pinned-tool index readbacks; no custom metadata hash."""
import json
from apk_contract import HEX64,require,tags,integer

FIELDS={'name','version','arch','origin','repo-commit','build-time','license','description','tags','hashes','file-size','installed-size'}
FILENAME_PATTERN='${name}-${version}.${arch}.apk'

def parse_dump(raw):
 require(isinstance(raw,bytes) and len(raw)<=65536,'index readback bound')
 def pairs(items):
  value={}
  for key,item in items:
   require(key not in value,'duplicate readback member');value[key]=item
  return value
 return json.loads(raw.decode('utf-8'),object_pairs_hook=pairs,parse_constant=lambda _:(_ for _ in ()).throw(ValueError('nonfinite readback')))

def entries(value):
 require(isinstance(value,dict) and set(value)=={'packages','pkgname-spec'} and value['pkgname-spec']==FILENAME_PATTERN and isinstance(value['packages'],list) and len(value['packages'])==2,'exact two-target index and filename pattern')
 result={}
 for item in value['packages']:
  require(isinstance(item,dict) and set(item)==FIELDS,'exact initial index fields')
  arch=item['arch'];require(arch in ('armv7','aarch64') and arch not in result,'duplicate or unknown index architecture')
  require(isinstance(item['hashes'],str) and HEX64.fullmatch(item['hashes']),'pinned mkndx SHA256 metadata identity')
  require(integer(item['file-size'],1,268435456) and integer(item['installed-size'],0,268435456),'index sizes')
  result[arch]=item
 require(set(result)=={'armv7','aarch64'},'both index architectures')
 return result

def compare(original_raw,regenerated_raw,builds,pkgrel,licenses,apk_sizes):
 original=entries(parse_dump(original_raw));regenerated=entries(parse_dump(regenerated_raw))
 require(original==regenerated,'authenticated index differs from newly regenerated APK metadata index')
 require(set(builds)==set(original) and set(licenses)==set(original) and set(apk_sizes)==set(original),'two-target observations')
 for arch,item in original.items():
  build=builds[arch];source=build['source']
  expected=dict(name='remarkable-buddies',origin='remarkable-buddies',version=source['tag'][1:]+'-r'+str(pkgrel),arch=arch,
   **{'repo-commit':source['sha'],'build-time':source['epoch'],'license':licenses[arch],
      'description':'ReMarkableBuddies test-key source-build package','tags':tags(build,pkgrel),'file-size':apk_sizes[arch]})
  require(all(item[key]==value and type(item[key]) is type(value) for key,value in expected.items()),'index/source-build correspondence')
 return {arch:dict(index_metadata_hash_algorithm='sha256',index_metadata_hash_encoding='lowercase-hex',index_metadata_hash=item['hashes'],apk_file_bytes=item['file-size']) for arch,item in original.items()}
