"""Exercise the exact raw helper orchestration with owned I/O substitutes."""
import ast
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
tree = ast.parse((root / "tools/test_touch_echo.py").read_text())
harness = next(n.value for n in ast.walk(tree) if isinstance(n, ast.Constant)
               and isinstance(n.value, str) and 'extern crate self as anyhow;' in n.value)
touch = (root / "src/device/touch.rs").read_text()
method = re.search(r'    pub fn diagnostic_tap_echo_raw\(.*?(?=\n    /// One development contact)', touch, re.S).group()
method = method.replace('std::fs::read_to_string("/etc/os-release")', 'crate::release_file()')
harness = harness.replace('pub struct InputObserver;',
    'pub struct InputObserver {pub raw_evidence: Option<super::tap_echo_raw::Evidence>}')
harness = harness.replace('io("observer")?; Ok(Self)',
    'io("observer")?; Ok(Self {raw_evidence:None})')
harness = harness.replace(' pub mod touch {', r'''
 pub mod tap_echo_raw {
  use super::*;
  pub struct Evidence;
  impl Evidence {
   pub fn new(_: &std::path::Path)->Result<Self> {io("reserve")?; Ok(Self)}
   pub fn persist(&mut self,_:(i32,i32),_:(i32,i32),_:&Result<()>)->Result<()> {io("persist")}
  }
 }
 pub mod touch {''')
harness = harness.replace('__METHOD__',method).replace('__BRANCH__','Ok(())')
harness = harness[:harness.index('fn check(')] + r'''
fn main() {
 let prefix=["firmware-read","reserve","observer","identity","begin","down"];
 let complete=["firmware-read","reserve","observer","identity","begin","down","wait","release","finish","persist"];
 for failure in ["", "down", "release", "finish", "persist", "down,release",
                 "down,finish", "release,finish", "down,release,finish,persist"] {
  reset(failure);
  let mut t=Touch{device_model:DeviceModel::Remarkable2};
  let result=t.diagnostic_tap_echo_raw((110,280),std::path::Path::new("owned-fixture"));
  assert_eq!(result.is_ok(),failure.is_empty(),"{failure}");
  let expected=if failure.split(',').any(|v|v=="down") {
   [prefix.as_slice(),&["release","finish","persist"]].concat()
  } else {complete.to_vec()};
  assert_eq!(*EVENTS.lock().unwrap(),expected,"{failure}");
 }
 reset("begin");
 let mut t=Touch{device_model:DeviceModel::Remarkable2};
 assert!(t.diagnostic_tap_echo_raw((110,280),std::path::Path::new("owned")).is_err());
 assert_eq!(*EVENTS.lock().unwrap(),["firmware-read","reserve","observer","identity","begin","persist"]);
 reset("reserve");
 assert!(t.diagnostic_tap_echo_raw((110,280),std::path::Path::new("owned")).is_err());
 assert_eq!(*EVENTS.lock().unwrap(),["firmware-read","reserve"]);
 eprintln!("PASS 11 exact raw helper cleanup/persistence cases; owned I/O, no tablet");
}
'''
with tempfile.TemporaryDirectory(prefix="touch-echo-raw-") as directory:
    source = Path(directory) / "fixture.rs"
    source.write_text(harness)
    executable = Path(directory) / "fixture.exe"
    subprocess.run(["rustc", "--edition=2021", str(source), "-o", str(executable)], check=True)
    result = subprocess.run([str(executable)], check=True, capture_output=True, text=True)
    print(result.stderr.strip())
