"""Run exact tap branch/tail with owned mock I/O; never open tablet devices."""
from pathlib import Path
import re
import subprocess
import tempfile

source = (Path(__file__).resolve().parents[1] / "examples/hardware_probe.rs").read_text()
branch = re.search(r'Some\("tap"\).*?=> \{(.*?)\n        \}\n        Some\("strokes"\)', source, re.S).group(1)
tail = source[source.index("    sleep(Duration::from_secs(2));\n    let mut screenshot = Screenshot::new()?;") : source.index('\n}\n\n#[cfg(not(target_os = "linux"))]')]
harness = r'''
extern crate self as anyhow;
use std::time::Duration;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[macro_export] macro_rules! anyhow { ($text:literal) => { $text.to_string() }; }
#[macro_export] macro_rules! ensure { ($ok:expr, $text:literal) => { if !$ok { return Err($text.into()); } }; }
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static FAIL_STOP: AtomicBool = AtomicBool::new(false);
fn event(text: String) { EVENTS.lock().unwrap().push(text); }
fn sleep(duration: Duration) { event(format!("wait:{}", duration.as_millis())); }
enum TriggerCorner { LowerLeft }
struct Touch;
impl Touch {
 fn new(_: bool, _: TriggerCorner) -> Self { event("open".into()); Self }
 fn touch_start(&mut self, point: (i32, i32)) -> Result<()> { event(format!("start:{},{}", point.0, point.1)); Ok(()) }
 fn touch_stop(&mut self) -> Result<()> { event("stop".into()); if FAIL_STOP.load(Ordering::Relaxed) { Err("stop failed".into()) } else { Ok(()) } }
}
struct Screenshot;
impl Screenshot {
 fn new() -> Result<Self> { event("capture-new".into()); Ok(Self) }
 fn take_screenshot(&mut self) -> Result<()> { event("capture".into()); Ok(()) }
 fn save_image(&self, path: &str) -> Result<()> { event(format!("save:{path}")); Ok(()) }
}
fn run(args: Vec<String>) -> Result<()> {
__BRANCH__
__TAIL__
}
fn check(args: &[&str], fail_stop: bool, success: bool, expected: &[&str]) {
 EVENTS.lock().unwrap().clear(); FAIL_STOP.store(fail_stop, Ordering::Relaxed);
 let args = std::iter::once("hardware_probe").chain(args.iter().copied()).map(str::to_owned).collect();
 assert_eq!(run(args).is_ok(), success);
 assert_eq!(*EVENTS.lock().unwrap(), expected);
}
fn main() {
 check(&["tap-only", "90", "260"], false, true, &["open", "start:90,260", "wait:100", "stop"]);
 check(&["tap", "90", "260"], false, true, &["open", "start:90,260", "wait:100", "stop", "wait:2000", "capture-new", "capture", "save:/tmp/reader-buddy-probe.png"]);
 check(&["press", "90", "260", "350"], false, true, &["open", "start:90,260", "wait:350", "stop", "wait:2000", "capture-new", "capture", "save:/tmp/reader-buddy-probe.png"]);
 check(&["press", "90", "260"], false, true, &["open", "start:90,260", "wait:2000", "stop", "wait:2000", "capture-new", "capture", "save:/tmp/reader-buddy-probe.png"]);
 check(&["tap-only", "bad", "260"], false, false, &[]);
 check(&["tap-only", "90"], false, false, &[]);
 check(&["tap-only", "90", "260"], true, false, &["open", "start:90,260", "wait:100", "stop"]);
 check(&["tap", "90", "260"], true, false, &["open", "start:90,260", "wait:100", "stop"]);
 println!("PASS 8 exact tap branch/tail cases; owned mock I/O, no tablet/capture");
}
'''.replace("__BRANCH__", branch).replace("__TAIL__", tail)
with tempfile.TemporaryDirectory(prefix="hardware-tap-") as directory:
    root = Path(directory)
    rust = root / "fixture.rs"
    rust.write_text(harness)
    executable = root / "fixture.exe"
    subprocess.run(["rustc", "--edition=2021", str(rust), "-o", str(executable)], check=True)
    subprocess.run([str(executable)], check=True)
