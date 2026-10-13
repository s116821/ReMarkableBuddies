"""Bounded ownership and confirmed removal of task-only labeled containers."""
import json,time,uuid
from apk_contract import require

def owner():return 'buddy-apk-'+uuid.uuid4().hex

def cleanup(nonce,cwd,command,timeout=5):
 deadline=time.monotonic()+timeout
 def remaining():
  value=deadline-time.monotonic();require(value>0,'owned Docker cleanup deadline');return value
 ids=command(['docker','ps','--all','--no-trunc','--filter','label=buddy-apk-owner='+nonce,'--format','{{.ID}}'],cwd,timeout=remaining()).splitlines()
 for identity in ids:
  require(len(identity)==64 and all(c in '0123456789abcdef' for c in identity),'owned container identity')
  info=json.loads(command(['docker','inspect','--format','{"id":{{json .Id}},"labels":{{json .Config.Labels}}}',identity],cwd,timeout=remaining()))
  require(info['id']==identity and info['labels'].get('buddy-apk-owner')==nonce,'container ownership changed')
  command(['docker','rm','--force',identity],cwd,timeout=remaining())
 remaining_ids=command(['docker','ps','--all','--no-trunc','--filter','label=buddy-apk-owner='+nonce,'--format','{{.ID}}'],cwd,timeout=remaining())
 require(not remaining_ids,'owned containers still present after removal')
 return dict(owner=nonce,removed=ids,absence_confirmed=True)

def phase(arguments,cwd,command,timeout=120,env=None,**options):
 nonce=owner()
 try:
  return command(['docker','run','--label','buddy-apk-owner='+nonce,'--name',nonce,*arguments],cwd,env=env,timeout=timeout,**options)
 finally:cleanup(nonce,cwd,command)

def cross_phase(argv,cwd,env,command,timeout=120):
 nonce=owner();selected=dict(env,CROSS_CONTAINER_OPTS='--label buddy-apk-owner='+nonce)
 try:return command(argv,cwd,env=selected,timeout=timeout)
 finally:cleanup(nonce,cwd,command)
