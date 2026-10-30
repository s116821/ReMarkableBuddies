"""Disposable root Linux filesystem fixtures; no tablet, Qt, input or service."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import time
import unittest
import uuid

HERE = Path(__file__).resolve().parent


class PublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if os.name != "posix" or os.getuid() != 0:
            raise unittest.SkipTest("Run in the existing isolated root Linux fixture container")
        cls.build = tempfile.TemporaryDirectory()
        cls.binary = Path(cls.build.name) / "publisher"
        cls.fixture = Path(cls.build.name) / "io-fixture"
        for source, output in (("publish_creation_arm.c", cls.binary), ("publisher_io_fixture.c", cls.fixture)):
            subprocess.run(["gcc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(HERE / source), "-o", str(output)], check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.build.cleanup()

    def setUp(self):
        self.nonce = uuid.uuid4().hex
        self.doc = str(uuid.uuid4())
        self.root = Path("/run/rmb-qt-probe-" + self.nonce)
        self.root.mkdir(mode=0o700)
        self.live = Path("/home/root/.local/share/remarkable/xochitl")
        self.live.mkdir(parents=True, exist_ok=True)
        pid = os.getpid()
        stat = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        start = int(stat[19])
        now = time.clock_gettime(time.CLOCK_BOOTTIME)
        candidate = int(now) - 1
        armed = candidate - 27
        self.args = [self.nonce, str(pid), str(start), str(int(now * 1000) + 10000), str(armed), str(armed + 360), str(candidate), self.doc]
        self.token = f"{self.nonce} {pid} {start} create {self.args[3]}\n".encode()
        self.put("owner", self.nonce.encode())
        self.put("candidate.window", f"{self.nonce} {armed} {armed+360} {candidate} {candidate+70}\n".encode())
        self.put("creation-waiting", self.token.replace(b" create ", b" waiting "))
        for ext in ("content", "metadata"):
            self.put("host-source." + ext, b"bounded source bytes\n")
            (self.live / f"{self.doc}.{ext}").write_bytes(b"bounded source bytes\n")

    def tearDown(self):
        # Only directories created by this fixture, inside its disposable container.
        for path in (self.root, Path(str(self.root) + "-old")):
            assert path.parent == Path("/run") and self.nonce in path.name
            if path.exists(): shutil.rmtree(path)
        for ext in ("content", "metadata"):
            (self.live / f"{self.doc}.{ext}").unlink(missing_ok=True)

    def put(self, name, data):
        path = self.root / name
        path.write_bytes(data)
        path.chmod(0o600)

    def run_publisher(self, env=None):
        executable = self.fixture if env else self.binary
        result = subprocess.run([str(executable), *self.args], env=os.environ | (env or {}), capture_output=True, timeout=3)
        return result

    def test_complete_single_link_arm_is_published_once(self):
        result = self.run_publisher()
        self.assertEqual(result.returncode, 0, result.stderr)
        arm = self.root / "creation-arm"
        self.assertEqual(arm.read_bytes(), self.token)
        self.assertEqual(arm.stat().st_nlink, 1)
        self.assertFalse((self.root / "creation-arm.tmp").exists())
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertEqual(arm.read_bytes(), self.token)

    def select_v2(self, startup=27):
        candidate = int(self.args[6]); armed = candidate - startup
        self.args[4:6] = [str(armed), str(armed + 360)]
        self.args[3] = str((candidate + 100) * 1000)
        self.args.append('open-document-insertion-v2')
        self.token = f'{self.nonce} {self.args[1]} {self.args[2]} create {self.args[3]}\n'.encode()
        self.put('creation-waiting', self.token.replace(b' create ', b' waiting '))
        self.put('candidate.window', f'{self.nonce} {armed} {armed+360} {candidate} {candidate+130}\n'.encode())

    def test_v2_fresh_commit_and_trigger_boundary(self):
        self.select_v2()
        candidate = int(self.args[6])
        # Conservative upper seconds at trigger90 refuse even just before90.
        self.assertEqual(self.run_publisher({'TEST_NOW_MS': str((candidate+89)*1000+1)}).returncode, 90)
        self.assertFalse((self.root/'creation-arm').exists())
        self.assertEqual(self.run_publisher({'TEST_NOW_MS': str((candidate+89)*1000)}).returncode, 0)
        self.assertEqual((self.root/'creation-arm').read_bytes(), self.token)

    def test_v2_latest_start_and_original_sdk_expiry(self):
        self.select_v2(startup=46)
        candidate = int(self.args[6])
        self.assertEqual(self.run_publisher({'TEST_NOW_MS': str(candidate*1000)}).returncode, 90)
        self.args[4:6] = [str(candidate-45), str(candidate+315)]
        self.put('candidate.window', f'{self.nonce} {candidate-45} {candidate+315} {candidate} {candidate+130}\n'.encode())
        self.assertEqual(self.run_publisher({'TEST_NOW_MS': self.args[3]}).returncode, 90)
        self.assertEqual(self.run_publisher({'TEST_NOW_MS': str((candidate+89)*1000)}).returncode, 0)

    def early_sdk_expiry(self):
        self.select_v2()
        self.args[3]=str((int(self.args[6])+80)*1000)
        self.token=f'{self.nonce} {self.args[1]} {self.args[2]} create {self.args[3]}\n'.encode()
        self.put('creation-waiting',self.token.replace(b' create ',b' waiting '))

    def test_original_sdk_expiry_independently_before_trigger(self):
        self.early_sdk_expiry()
        self.assertEqual(self.run_publisher({'TEST_NOW_MS':self.args[3]}).returncode,90)
        self.assertEqual(self.run_publisher({'TEST_NOW_MS':str(int(self.args[3])-1)}).returncode,0)

    def test_expiry_after_staging_refuses_at_fresh_commit(self):
        self.early_sdk_expiry()
        self.assertEqual(self.run_publisher({'TEST_NOW_MS':str(int(self.args[3])-1),
                                             'TEST_AFTER_WRITE_MS':self.args[3]}).returncode,90)
        self.assertTrue((self.root/'creation-arm.tmp').exists())
        self.assertFalse((self.root/'creation-arm').exists())

    def test_each_closure_shape_refuses_v2_without_publication(self):
        self.select_v2()
        for name in ('restore.claim','entry.closed','stop.request','stop.admitted','diagnostics.json','callback.json'):
            for kind in ('file','directory','dangling'):
                with self.subTest(name=name, kind=kind):
                    path=self.root/name
                    if kind=='file': self.put(name,b'closed')
                    elif kind=='directory': path.mkdir()
                    else: path.symlink_to(self.root/'absent-target')
                    self.assertEqual(self.run_publisher().returncode,90)
                    self.assertFalse((self.root/'creation-arm').exists())
                    if kind=='directory': path.rmdir()
                    else: path.unlink()
    def test_existing_destination_and_symlink_are_preserved(self):
        self.put("creation-arm", b"existing")
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertEqual((self.root / "creation-arm").read_bytes(), b"existing")
        (self.root / "creation-arm").unlink()
        (self.root / "creation-arm").symlink_to(self.root / "owner")
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertTrue((self.root / "creation-arm").is_symlink())

    def test_stale_temporary_is_not_adopted(self):
        self.put("creation-arm.tmp", b"stale")
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertEqual((self.root / "creation-arm.tmp").read_bytes(), b"stale")

    def test_partial_write_failure_never_exposes_arm(self):
        self.assertEqual(self.run_publisher({"TEST_WRITE_FAIL": "1"}).returncode, 90)
        self.assertFalse((self.root / "creation-arm").exists())
        self.assertLess((self.root / "creation-arm.tmp").stat().st_size, len(self.token))

    def test_short_writes_finish_before_atomic_publication(self):
        self.assertEqual(self.run_publisher({"TEST_SHORT_WRITE": "1"}).returncode, 0)
        self.assertEqual((self.root / "creation-arm").read_bytes(), self.token)

    def test_stale_process_start_refuses(self):
        self.args[2] = str(int(self.args[2]) + 1)
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertFalse((self.root / "creation-arm").exists())

    def test_expiry_and_closing_marker_refuse(self):
        original = self.args[3]
        self.args[3] = str(int(time.clock_gettime(time.CLOCK_BOOTTIME) * 1000) - 1)
        self.put("creation-waiting", f"{self.nonce} {self.args[1]} {self.args[2]} waiting {self.args[3]}\n".encode())
        self.assertEqual(self.run_publisher().returncode, 90)
        self.args[3] = original
        self.put("creation-waiting", self.token.replace(b" create ", b" waiting "))
        self.put("restore.claim", b"closing")
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertFalse((self.root / "creation-arm").exists())

    def test_sidecar_change_and_root_replacement_refuse(self):
        (self.live / f"{self.doc}.content").write_bytes(b"different")
        self.assertEqual(self.run_publisher().returncode, 90)
        self.assertFalse((self.root / "creation-arm").exists())
        (self.root / "creation-arm.tmp").unlink()
        (self.live / f"{self.doc}.content").write_bytes(b"bounded source bytes\n")
        self.assertEqual(self.run_publisher({"TEST_SWAP_ROOT": "1"}).returncode, 90)
        self.assertFalse((self.root / "creation-arm").exists())
        self.assertFalse((Path(str(self.root) + "-old") / "creation-arm").exists())

    def test_unsupported_rename_has_no_fallback(self):
        self.assertEqual(self.run_publisher({"TEST_UNSUPPORTED": "1"}).returncode, 90)
        self.assertFalse((self.root / "creation-arm").exists())
        self.assertTrue((self.root / "creation-arm.tmp").exists())

    def test_receipt_failure_does_not_mean_unpublished(self):
        self.assertEqual(self.run_publisher({"TEST_RECEIPT_FAIL": "1"}).returncode, 91)
        self.assertEqual((self.root / "creation-arm").read_bytes(), self.token)
        self.assertEqual(self.run_publisher().returncode, 90)


if __name__ == "__main__":
    unittest.main()
