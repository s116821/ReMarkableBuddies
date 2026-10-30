"""Own command descendants before execution; kill only the owned job/group."""
import os,signal,subprocess,time

class OwnedProcess:
 def __init__(self,argv,release_deadline=None,**kwargs):
  self.job=None;self.closed=False
  self.release_deadline=release_deadline
  if os.name!='nt':
   self.process=subprocess.Popen(argv,start_new_session=True,**kwargs)
   self.group=self.process.pid
   return
  import ctypes
  from ctypes import wintypes as w
  self.ctypes=ctypes;kernel=ctypes.WinDLL('kernel32',use_last_error=True);self.kernel=kernel
  size=ctypes.c_size_t
  class Basic(ctypes.Structure):
   _fields_=[('process_time',ctypes.c_longlong),('job_time',ctypes.c_longlong),('flags',w.DWORD),('min_working',size),('max_working',size),('active_limit',w.DWORD),('affinity',size),('priority',w.DWORD),('scheduling',w.DWORD)]
  class IO(ctypes.Structure):
   _fields_=[(name,ctypes.c_ulonglong) for name in ('read_ops','write_ops','other_ops','read_bytes','write_bytes','other_bytes')]
  class Extended(ctypes.Structure):
   _fields_=[('basic',Basic),('io',IO),('process_memory',size),('job_memory',size),('peak_process',size),('peak_job',size)]
  class Accounting(ctypes.Structure):
   _fields_=[('user_time',ctypes.c_longlong),('kernel_time',ctypes.c_longlong),('period_user',ctypes.c_longlong),('period_kernel',ctypes.c_longlong),('faults',w.DWORD),('total',w.DWORD),('active',w.DWORD),('terminated',w.DWORD)]
  self.Accounting=Accounting
  kernel.CreateJobObjectW.argtypes=[ctypes.c_void_p,w.LPCWSTR];kernel.CreateJobObjectW.restype=w.HANDLE
  kernel.SetInformationJobObject.argtypes=[w.HANDLE,ctypes.c_int,ctypes.c_void_p,w.DWORD]
  kernel.AssignProcessToJobObject.argtypes=[w.HANDLE,w.HANDLE]
  kernel.TerminateJobObject.argtypes=[w.HANDLE,w.UINT]
  kernel.QueryInformationJobObject.argtypes=[w.HANDLE,ctypes.c_int,ctypes.c_void_p,w.DWORD,ctypes.c_void_p]
  kernel.CloseHandle.argtypes=[w.HANDLE]
  self.job=kernel.CreateJobObjectW(None,None)
  if not self.job:raise ctypes.WinError(ctypes.get_last_error())
  limits=Extended();limits.basic.flags=0x2000 # KILL_ON_JOB_CLOSE
  if not kernel.SetInformationJobObject(self.job,9,ctypes.byref(limits),ctypes.sizeof(limits)):
   self.close();raise ctypes.WinError(ctypes.get_last_error())
  try:
   # Suspension closes the launch/assignment race: descendants cannot start
   # before the process belongs to this non-breakaway kill-on-close job.
   self.process=subprocess.Popen(argv,creationflags=0x4|0x08000000,**kwargs)
   if not kernel.AssignProcessToJobObject(self.job,w.HANDLE(int(self.process._handle))):
    self.process.kill();self.process.wait();raise ctypes.WinError(ctypes.get_last_error())
   self._resume_primary_thread()
  except BaseException:
   if hasattr(self,'process') and self.process.poll() is None:self.process.kill();self.process.wait()
   self.close();raise

 def _resume_primary_thread(self):
  ctypes=self.ctypes
  from ctypes import wintypes as w
  kernel=self.kernel
  class Thread(ctypes.Structure):
   _fields_=[('size',w.DWORD),('usage',w.DWORD),('thread',w.DWORD),('owner',w.DWORD),('base_priority',w.LONG),('delta_priority',w.LONG),('flags',w.DWORD)]
  kernel.CreateToolhelp32Snapshot.argtypes=[w.DWORD,w.DWORD];kernel.CreateToolhelp32Snapshot.restype=w.HANDLE
  kernel.Thread32First.argtypes=[w.HANDLE,ctypes.POINTER(Thread)];kernel.Thread32Next.argtypes=[w.HANDLE,ctypes.POINTER(Thread)]
  kernel.OpenThread.argtypes=[w.DWORD,w.BOOL,w.DWORD];kernel.OpenThread.restype=w.HANDLE
  kernel.GetProcessIdOfThread.argtypes=[w.HANDLE];kernel.GetProcessIdOfThread.restype=w.DWORD
  kernel.ResumeThread.argtypes=[w.HANDLE];kernel.ResumeThread.restype=w.DWORD
  snapshot=kernel.CreateToolhelp32Snapshot(4,0)
  if snapshot==w.HANDLE(-1).value:raise ctypes.WinError(ctypes.get_last_error())
  thread=Thread();thread.size=ctypes.sizeof(thread);matches=[]
  try:
   present=kernel.Thread32First(snapshot,ctypes.byref(thread))
   while present:
    if thread.owner==self.process.pid:matches.append(thread.thread)
    present=kernel.Thread32Next(snapshot,ctypes.byref(thread))
  finally:kernel.CloseHandle(snapshot)
  if len(matches)!=1:raise RuntimeError('owned suspended primary thread identity')
  handle=kernel.OpenThread(0x0002|0x0800,False,matches[0])
  if not handle:raise ctypes.WinError(ctypes.get_last_error())
  try:
   if self.release_deadline is not None and time.monotonic()>=self.release_deadline:raise RuntimeError('owned launch deadline before release')
   if kernel.GetProcessIdOfThread(handle)!=self.process.pid or kernel.ResumeThread(handle)!=1:
    raise RuntimeError('owned primary thread resume refused')
  finally:kernel.CloseHandle(handle)

 def exit_status(self,deadline):
  if os.name=='nt':return self.process.wait(timeout=max(0.001,deadline-time.monotonic()))
  # Keep the exited parent unreaped until group termination so its PID cannot
  # be reused as an unrelated process-group identifier.
  while True:
   info=os.waitid(os.P_PID,self.process.pid,os.WEXITED|os.WNOHANG|os.WNOWAIT)
   if info is not None:return info.si_status if info.si_code==os.CLD_EXITED else -info.si_status
   if time.monotonic()>=deadline:raise TimeoutError('owned command deadline')
   time.sleep(0.01)

 def terminate_and_verify(self,timeout=3):
  if os.name=='nt':
   if not self.kernel.TerminateJobObject(self.job,1):raise self.ctypes.WinError(self.ctypes.get_last_error())
   deadline=time.monotonic()+timeout
   while True:
    info=self.Accounting()
    if not self.kernel.QueryInformationJobObject(self.job,1,self.ctypes.byref(info),self.ctypes.sizeof(info),None):raise self.ctypes.WinError(self.ctypes.get_last_error())
    if info.active==0:break
    if time.monotonic()>=deadline:raise RuntimeError('owned job termination not confirmed')
    time.sleep(0.01)
  else:
   try:os.killpg(self.group,signal.SIGKILL)
   except ProcessLookupError:pass
  self.process.wait(timeout=timeout)
  if os.name!='nt':
   deadline=time.monotonic()+timeout
   while True:
    active=False
    for path in __import__('pathlib').Path('/proc').glob('[0-9]*/stat'):
     try:
      value=path.read_text();tail=value[value.rfind(')')+2:].split()
      if int(tail[2])==self.group and tail[0] not in ('Z','X'):active=True;break
     except (OSError,ValueError,IndexError):continue
    if not active:break
    if time.monotonic()>=deadline:raise RuntimeError('owned process-group termination not confirmed')
    time.sleep(0.01)

 def close(self):
  if self.job is not None and not self.closed:
   self.kernel.CloseHandle(self.job);self.closed=True
