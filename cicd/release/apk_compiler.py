"""Observe the explicit stable Linux toolchain mounted by Cross, not host rustc."""
import json,re
from pathlib import Path
from apk_contract import require
from docker_owned import phase

def identity(repo,target,image,cross,command,digest):
 active=command(['rustup','show','active-toolchain'],repo).split()[0]
 require(active.startswith('stable-'),'initial builder requires explicit stable toolchain')
 home=Path(command(['rustup','show','home'],repo))
 root=home/'toolchains/stable-x86_64-unknown-linux-gnu'
 rustc=root/'bin/rustc';require(rustc.is_file(),'existing Linux Cross compiler required')
 driver=sorted((root/'lib').glob('librustc_driver-*.so'))
 libraries=sorted((root/'lib/rustlib'/target/'lib').glob('*.rlib'))
 require(len(driver)==1 and libraries,'existing Cross compiler driver and target libraries')
 image_info=json.loads(command(['docker','image','inspect',image,'--format','{"id":{{json .Id}},"os":{{json .Os}},"arch":{{json .Architecture}},"digests":{{json .RepoDigests}}}'],repo))
 require(image_info['os']=='linux' and image_info['arch']=='amd64' and image in image_info['digests'],'observed immutable Cross image')
 verbose=phase(['--rm','--network','none','--cap-drop','ALL','--read-only','--mount',f'type=bind,source={root},target=/toolchain,readonly','--env','LD_LIBRARY_PATH=/toolchain/lib','--entrypoint','/toolchain/bin/rustc',image,'--version','--verbose'],repo,command,timeout=30)
 fields=dict(line.split(': ',1) for line in verbose.splitlines() if ': ' in line)
 require(fields.get('host')=='x86_64-unknown-linux-gnu','actual Cross compiler host')
 stdout,stderr=command([str(cross),'--version'],repo,return_streams=True)
 versions=re.findall(rb'(?m)^cross [^\r\n]+',stdout+b'\n'+stderr)
 require(len(versions)==1,'actual Cross banner identity')
 files=[rustc,*driver,*libraries]
 hashes={str(path.relative_to(root)):digest(path) for path in files}
 return dict(toolchain='stable',linux_sysroot=str(root),rustc_verbose=verbose,rustc_fields=fields,toolchain_file_hashes=hashes,cross_version=versions[0].decode('ascii'),cross_binary_sha256=digest(cross),image=image_info)
