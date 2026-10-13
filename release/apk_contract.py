"""Offline test-key APK contract; no release or installation authority."""
import hashlib,json,re

TARGETS={
 'armv7-unknown-linux-gnueabihf':('armv7','gnu-arm-eabihf',32,'ARM'),
 'aarch64-unknown-linux-gnu':('aarch64','gnu-aarch64',64,'AArch64'),
}
REPOSITORY='https://github.com/s116821/ReMarkableBuddies'
SDK_REPOSITORY='https://github.com/s116821/ReMarkableOpenSDK'
TAG=re.compile(r'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z')
HEX40=re.compile(r'[0-9a-f]{40}\Z');HEX64=re.compile(r'[0-9a-f]{64}\Z')
PREFIX='remarkable-buddies:'

class Refusal(ValueError): pass

def require(value,message):
 if not value: raise Refusal(message)

def ascii_text(value,cap):
 return isinstance(value,str) and 0<len(value)<=cap and all(32<=ord(c)<127 for c in value)

def stable(value): return ascii_text(value,64) and TAG.fullmatch(value)

def exact(value,keys):
 require(isinstance(value,dict) and set(value)==set(keys),'exact object members')

def integer(value,minimum,maximum): return type(value) is int and minimum<=value<=maximum

def digest(value,pattern=HEX64): return isinstance(value,str) and pattern.fullmatch(value)

def validate_build(value):
 exact(value,['schema','evidence','eligibility','source','target','sdk','cargo','toolchain','build','binary'])
 require(type(value['schema']) is int and value['schema']==1 and value['evidence']=='developer-source-build' and value['eligibility']=='not-qualified','contract/evidence')
 source=value['source'];exact(source,['repository','tag','sha','epoch'])
 require(source['repository']==REPOSITORY and stable(source['tag']) and digest(source['sha'],HEX40) and integer(source['epoch'],0,2**63-1),'source identity')
 target=value['target'];exact(target,['rust_target','apk_arch','abi','elf_class','elf_machine','elf_endian','interpreter'])
 require(target['rust_target'] in TARGETS,'target')
 arch,abi,cls,machine=TARGETS[target['rust_target']]
 require((target['apk_arch'],target['abi'],target['elf_class'],target['elf_machine'],target['elf_endian'])==(arch,abi,cls,machine,'little') and ascii_text(target['interpreter'],256) and target['interpreter'].startswith('/'),'ELF/ABI')
 sdk=value['sdk']
 if sdk is not None:
  exact(sdk,['ref_kind','repository','sha','tag','manifest_sha256'])
  require(sdk['ref_kind'] in ['tagged-git','commit-pinned'] and sdk['repository']==SDK_REPOSITORY and digest(sdk['sha'],HEX40) and digest(sdk['manifest_sha256']),'SDK source')
  require(stable(sdk['tag']) if sdk['ref_kind']=='tagged-git' else sdk['tag'] is None,'SDK tag variant')
 cargo=value['cargo'];exact(cargo,['lock_sha256','metadata_identity_sha256','features'])
 require(digest(cargo['lock_sha256']) and digest(cargo['metadata_identity_sha256']) and isinstance(cargo['features'],list) and len(cargo['features'])<=64 and all(ascii_text(f,128) for f in cargo['features']) and cargo['features']==sorted(set(cargo['features'])),'Cargo identity')
 tools=value['toolchain'];exact(tools,['rustc_release','rustc_commit','rustc_host','cross_version','cross_binary_sha256','image_digest','build_configuration_sha256'])
 require(all(ascii_text(tools[k],128) for k in ['rustc_release','rustc_host','cross_version']) and (tools['rustc_commit'] is None or digest(tools['rustc_commit'],HEX40)) and all(digest(tools[k]) for k in ['cross_binary_sha256','build_configuration_sha256']) and isinstance(tools['image_digest'],str) and tools['image_digest'].startswith('sha256:') and digest(tools['image_digest'][7:]),'toolchain')
 build=value['build'];exact(build,['argv','environment','profile','source_build_helper_sha256'])
 exact(build['environment'],['RUSTFLAGS','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS'])
 require(all(isinstance(v,str) and len(v)<=1024 and all(32<=ord(c)<127 for c in v) for v in build['environment'].values()),'environment')
 require(isinstance(build['argv'],list) and len(build['argv'])<=64 and all(ascii_text(a,256) for a in build['argv']) and digest(build['source_build_helper_sha256']) and build['profile']==('legacy-no-sdk' if sdk is None else 'unqualified-development'),'build/profile')
 binary=value['binary'];exact(binary,['path','sha256','bytes','mode'])
 require(binary['path']=='usr/bin/reader-buddy' and digest(binary['sha256']) and integer(binary['bytes'],1,134217728) and type(binary['mode']) is int and binary['mode']==493,'binary')
 return value

def canonical(value):
 validate_build(value)
 raw=(json.dumps(value,sort_keys=True,separators=(',',':'),ensure_ascii=True)+'\n').encode('utf-8')
 require(len(raw)<=65536,'build JSON bound')
 return raw

def parse_build(raw):
 require(len(raw)<=65536,'build JSON bound')
 def pairs(items):
  result={}
  for key,value in items:
   require(key not in result,'duplicate JSON member');result[key]=value
  return result
 value=json.loads(raw.decode('utf-8'),object_pairs_hook=pairs,parse_constant=lambda _:(_ for _ in ()).throw(Refusal('nonfinite JSON')))
 require(canonical(value)==raw,'canonical build JSON bytes')
 return value

def tags(value,pkgrel):
 validate_build(value);require(integer(pkgrel,0,65535),'pkgrel')
 source,target,sdk=value['source'],value['target'],value['sdk']
 fields=dict(contract='1',evidence=value['evidence'],eligibility=value['eligibility'],**{'source-repo':source['repository'],'source-tag':source['tag'],'source-sha':source['sha'],'target':target['rust_target'],'abi':target['abi'],'pkgrel':str(pkgrel),'binary-sha256':value['binary']['sha256'],'sdk-ref':'absent' if sdk is None else sdk['ref_kind'],'profile':value['build']['profile']})
 if sdk is not None:
  fields.update({'sdk-repo':sdk['repository'],'sdk-sha':sdk['sha']})
  if sdk['ref_kind']=='tagged-git': fields['sdk-tag']=sdk['tag']
 values=[PREFIX+k+'='+v for k,v in sorted(fields.items())]
 require(len(values)<=18 and sum(len(v) for v in values)<=4096 and all(len(v)<=256 and re.fullmatch(r'[a-zA-Z0-9._+,:/\[\]=\-]+',v) for v in values),'signed tag bounds/encoding')
 return values

def verify_tags(actual,value,pkgrel):
 require(isinstance(actual,list) and len(actual)<=18,'tag list')
 seen=set()
 for item in actual:
  require(isinstance(item,str) and '=' in item,'tag shape')
  name=item.split('=',1)[0];require(name not in seen,'duplicate tag');seen.add(name)
 require(actual==tags(value,pkgrel),'exact signed tag set/order/value')
