"""Execute exact diagnostic method/CLI bodies with owned OS/I/O substitutes.

This checks orchestration only; production classifier tests run separately.
"""
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
touch = (root / "src/device/touch.rs").read_text()
example = (root / "examples/hardware_probe.rs").read_text()
method = re.search(r'    pub fn diagnostic_tap_echo\(.*?(?=\n    pub\(super\) fn input_identity)', touch, re.S).group()
# Substitute only the firmware file read; all guard/control-flow code is exact.
method = method.replace('std::fs::read_to_string("/etc/os-release")', 'crate::release_file()')
branch = re.search(r'Some\("tap-echo"\) => \{(.*?)\n        \}\n        Some\("multi-hold"\)', example, re.S).group(1)
assert '#[cfg(feature = "development-input-diagnostics")]\n    pub fn diagnostic_tap_echo' in touch
assert '#[cfg(feature = "development-input-diagnostics")]\n        Some("tap-echo")' in example
assert touch.index('pub fn diagnostic_tap_echo') < touch.index('#[cfg(not(target_os = "linux"))]\nimpl Touch')
harness = r'''
extern crate self as anyhow;
use std::{sync::Mutex, time::Duration};
type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)] struct Error(String);
impl std::fmt::Display for Error { fn fmt(&self,f:&mut std::fmt::Formatter)->std::fmt::Result { write!(f,"{}",self.0) } }
impl std::error::Error for Error {}
impl From<std::num::ParseIntError> for Error { fn from(e:std::num::ParseIntError)->Self { Self(e.to_string()) } }
impl Error { fn context(self,s:String)->Self { Self(format!("{}: {s}",self.0)) } }
#[macro_export] macro_rules! anyhow { ($s:literal) => { Error($s.into()) }; }
#[macro_export] macro_rules! ensure { ($ok:expr,$s:literal) => { if !$ok { return Err(Error($s.into())); } }; }
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static FAIL: Mutex<String> = Mutex::new(String::new());
fn io(name:&str)->Result<()> { EVENTS.lock().unwrap().push(name.into()); if FAIL.lock().unwrap().split(',').any(|s|s==name) { Err(Error(name.into())) } else { Ok(()) } }
fn sleep(d:Duration) { assert_eq!(d.as_millis(),100); io("wait").unwrap(); }
fn release_file()->Result<String> { io("firmware-read")?; Ok("fixture".into()) }
#[derive(Clone,Copy,PartialEq)] enum DeviceModel { Remarkable2, Other }
impl DeviceModel { fn detect()->Self { if FAIL.lock().unwrap().contains("model") { Self::Other } else { Self::Remarkable2 } } }
enum TriggerCorner { LowerLeft }
mod device {
 use super::*;
 pub mod native_page { use super::*; pub fn verified_contract(_:DeviceModel,_:&str)->bool { !FAIL.lock().unwrap().contains("firmware-invalid") } }
 pub mod input_observer {
  use super::*;
  pub struct InputObserver;
  impl InputObserver {
   pub fn new(_:TriggerCorner,_:Option<()>)->Result<Self> { io("observer")?; Ok(Self) }
   pub fn begin_owned_touch(&mut self,_:(),_:(i32,i32))->Result<()> { io("begin") }
   pub fn finish_owned_touch(&mut self)->Result<()> { io("finish") }
  }
 }
 pub mod touch {
  use super::*;
  pub struct Touch { pub device_model:DeviceModel }
  impl Touch {
   pub fn new(_:bool,_:TriggerCorner)->Self { io("open").unwrap(); Self {device_model:DeviceModel::Remarkable2} }
   fn input_identity(&self)->Result<()> { io("identity") }
   fn native_point(&self,p:(i32,i32))->(i32,i32) { p }
   fn touch_start(&mut self,_:(i32,i32))->Result<()> { io("down") }
   fn touch_stop(&mut self)->Result<()> { io("release") }
__METHOD__
  }
 }
}
use device::touch::Touch;
fn cli(args:Vec<String>)->Result<()> {
__BRANCH__
}
fn reset(f:&str) { EVENTS.lock().unwrap().clear(); *FAIL.lock().unwrap()=f.into(); }
fn check(f:&str,expected:&[&str],success:bool) {
 reset(f);
 let mut t=Touch{device_model:DeviceModel::Remarkable2};
 let result=t.diagnostic_tap_echo((90,260));
 assert_eq!(result.is_ok(),success,"failure={f}");
 assert_eq!(*EVENTS.lock().unwrap(),expected,"failure={f}");
}
fn main() {
 let prefix=["firmware-read","identity","observer","begin","down"];
 let complete=["firmware-read","identity","observer","begin","down","wait","release","finish"];
 check("",&complete,true);
 check("down",&[prefix.as_slice(),&["release","finish"]].concat(),false);
 check("down,release",&[prefix.as_slice(),&["release","finish"]].concat(),false);
 check("down,finish",&[prefix.as_slice(),&["release","finish"]].concat(),false);
 check("down,release,finish",&[prefix.as_slice(),&["release","finish"]].concat(),false);
 check("release",&complete,false);
 check("finish",&complete,false);
 check("release,finish",&complete,false);
 check("begin",&["firmware-read","identity","observer","begin"],false);
 check("identity",&["firmware-read","identity"],false);
 check("observer",&["firmware-read","identity","observer"],false);
 check("model",&[],false);
 check("firmware-invalid",&["firmware-read"],false);
 check("firmware-read",&["firmware-read"],false);
 reset("");
 let mut t=Touch{device_model:DeviceModel::Other};
 assert!(t.diagnostic_tap_echo((90,260)).is_err()); assert!(EVENTS.lock().unwrap().is_empty());
 for p in [(0,260),(768,260),(90,0),(90,1024),(-1,260),(i32::MAX,260)] {
  reset(""); assert!(t.diagnostic_tap_echo(p).is_err()); assert!(EVENTS.lock().unwrap().is_empty());
 }
 for a in [vec!["probe","tap-echo","bad","260"],vec!["probe","tap-echo","90"],vec!["probe","tap-echo","0","260"],vec!["probe","tap-echo","90","1024"],vec!["probe","tap-echo","90","260","extra"]] {
  reset(""); assert!(cli(a.into_iter().map(str::to_owned).collect()).is_err()); assert!(EVENTS.lock().unwrap().is_empty());
 }
 for f in ["down","release","finish","down,release","down,finish","release,finish","down,release,finish"] {
  reset(f); assert!(cli(["probe","tap-echo","90","260"].map(str::to_owned).to_vec()).is_err());
 }
 reset(""); assert!(cli(["probe","tap-echo","90","260"].map(str::to_owned).to_vec()).is_ok());
 assert_eq!(*EVENTS.lock().unwrap(),[&["open"][..],&complete].concat());
 eprintln!("PASS 34 exact diagnostic method/CLI cases; owned I/O, no tablet");
}
'''.replace("__METHOD__", method).replace("__BRANCH__", branch)
with tempfile.TemporaryDirectory(prefix="touch-echo-") as directory:
    fixture = Path(directory) / "fixture.rs"
    fixture.write_text(harness)
    executable = Path(directory) / "fixture.exe"
    subprocess.run(["rustc", "--edition=2021", str(fixture), "-o", str(executable)], check=True)
    result = subprocess.run([str(executable)], check=True, capture_output=True, text=True)
    assert result.stdout.count('"echo_observed":true') == 1
    assert '"ui_acknowledged":false,"native_navigation_qualified":false' in result.stdout
    print(result.stderr.strip())
