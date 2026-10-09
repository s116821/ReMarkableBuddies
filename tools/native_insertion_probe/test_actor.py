"""Owned exact-source recovery fixtures; no tablet/service manager/native payload."""
from pathlib import Path
import importlib.util
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("legacy_fixture", HERE.parent / "native_shutdown_probe/test_actor.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
fixture.SOURCE = (HERE / "actor.sh.in").read_text().split("# Main-only execution boundary", 1)[0]

class Recovery(unittest.TestCase):
    def run_case(self, damaged=False, unsafe=False, missing_identity=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            import hashlib
            original = root / "original.rm"; original.write_bytes(b"original ink")
            content = root / "fixture.content"; content.write_bytes(b"before")
            (root / "document-immutable.files").write_text(hashlib.sha256(original.read_bytes()).hexdigest() + "  " + str(original) + "\n")
            (root / "document-mutable.files").write_text(hashlib.sha256(content.read_bytes()).hexdigest() + "  " + str(content) + "\n")
            content.write_bytes(b"legitimate intended mutation")
            if damaged: original.write_bytes(b"unexpected damaged ink")
            overrides = """
reserve() { return 0; }
policy() { :; }
cgroup_gone() { :; }
process_gone() { :; }
value() { VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'; }
query() { printf '%s\n' "$*" >> "$root/events"; printf 'Result=success\n' > "$root/query.stdout"; }
stop_unit() { printf 'guarded-stop\n' >> "$root/events"; }
recovery_hashes() { @SAFETY@; }
verify_stock() { printf healthy > "$root/stock.restored"; }
""".replace("@SAFETY@", "return 90" if unsafe else ":")
            fixture.shell(root, 'stock_stopped=yes; ' + ('candidate_started=yes; ' if missing_identity else '') + 'restore', overrides, expected=90 if unsafe else 0)
            self.assertEqual((root / "stock.restored").exists(), not unsafe)
            if unsafe:
                self.assertFalse((root / "stock-start.claim").exists())
            else:
                self.assertTrue((root / "stock-start.claim").exists())
                self.assertEqual((root / "insertion.failed").exists(), damaged or missing_identity)
                self.assertIn("fail" if damaged else "pass", (root / "document.preservation").read_text())
                self.assertIn("differ-or-unreadable", (root / "document.mutation").read_text())
    def test_expected_mutation_restores_without_success_claim(self): self.run_case()
    def test_damaged_ink_restores_but_fails_preservation(self): self.run_case(damaged=True)
    def test_recovery_safety_failure_refuses_start(self): self.run_case(unsafe=True)
    def test_missing_launch_identity_stops_owned_unit_and_restores(self): self.run_case(missing_identity=True)
    def test_failed_primary_evidence_write_still_runs_recovery(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            (root / "insertion.outcome").mkdir()
            (root / "actor.sh").write_text('#!/bin/sh\nprintf healthy > "' + str(root / "stock.restored") + '"\n')
            fixture.shell(root, 'set +e; false; finish', 'preserve_failure() { return 90; }', expected=90)
            self.assertEqual((root / "stock.restored").read_text(), "healthy")
    def test_abnormal_exit_and_failed_failure_marker_still_restore(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            overrides = """
reserve() { return 0; }
policy() { :; }
cgroup_gone() { :; }
process_gone() { :; }
query() { printf 'Result=signal\n' > "$root/query.stdout"; }
stop_unit() { printf stopped > "$root/stopped"; }
value() { VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'; }
preserve_failure() { return 90; }
recovery_hashes() { :; }
verify_stock() { printf restored > "$root/stock.restored"; }
record_document_preservation() { :; }
"""
            fixture.shell(root,'stock_stopped=yes; candidate_started=yes; restore',overrides,expected=90)
            self.assertTrue((root/'stopped').exists())
            self.assertTrue((root/'stock-start.claim').exists())
            self.assertTrue((root/'stock.restored').exists())

class StopRequest(unittest.TestCase):
    def test_exact_request_admits_once_and_replay_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for name,data in (("stop.request","1"*32+"\n"),("attempt.identity","42 561\n")):
                (root/name).write_text(data); (root/name).chmod(0o600)
            fixture.shell(root,'observe_end=105; wait_insertion_stop','same_process() { return 0; }')
            self.assertTrue((root/"stop.admitted").exists())
            fixture.shell(root,'observe_end=105; wait_insertion_stop','same_process() { return 0; }',expected=2)
    def test_invalid_or_stale_request_refuses_without_admission(self):
        for content, stale in (("1"*32,False),("2"*32+"\n",False),("1"*32+"\n",True)):
            with tempfile.TemporaryDirectory() as directory:
                root=Path(directory)
                for name,data in (("stop.request",content),("attempt.identity","42 561\n")):
                    (root/name).write_text(data); (root/name).chmod(0o600)
                fixture.shell(root,'observe_end=105; wait_insertion_stop','same_process() { return '+('1' if stale else '0')+'; }',expected=93 if stale else 1)
                self.assertFalse((root/"stop.admitted").exists())
    def test_missing_request_reaches_fixed_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            fixture.shell(root,'observe_end=102; wait_insertion_stop','tick=100; now() { printf "%s\n" "$tick"; }; sleep() { tick=$((tick+1)); }',expected=1)
            self.assertFalse((root/"stop.admitted").exists())

class Partition(unittest.TestCase):
    def test_full_baseline_partition_and_refusals(self):
        import json
        spec = importlib.util.spec_from_file_location("insertion_prepare", HERE / "prepare_packet.py")
        prepare = importlib.util.module_from_spec(spec); spec.loader.exec_module(prepare)
        pages = ["a7181850-3f03-4bb0-8b9e-01acfc20032e", "d1261cb9-a8c0-4e15-ab24-7a9172b2027b", "f39ae285-3e0c-43dd-b27c-866dff7a24cd", "028b118f-474a-4886-a469-7273b290320c", "fd5afbe2-a79c-461b-8b15-96907d181ab7", "1692961d-ec5c-4f0f-9635-8e6081111dab"]
        selection = {"document_id": "e7f661f1-db6f-4dfc-854a-b38aff7f75de", "target_page_id": "11111111-1111-4111-8111-111111111111", "before_page_ids": pages}
        prefix = "/home/root/.local/share/remarkable/xochitl/" + selection["document_id"]
        paths = [prefix + "/" + p + ".rm" for p in pages[:3]] + [prefix + "." + ext for ext in ("content","local","metadata","pagedata","pdf")] + [prefix + ".thumbnails/" + p + ".png" for p in pages]
        paths += ["/usr/lib/libQt6" + n + ".so.6.10.3" for n in ("Core","Qml","Gui","Quick","Network")] + ["/usr/lib/libstdc++.so.6","/lib/libc.so.6","/lib/libm.so.6","/lib/libgcc_s.so.1","/lib/ld-linux-armhf.so.3"]
        files = [{"path": path,"sha256":"a"*64} for path in paths]
        _, _, _, runtime, immutable, mutable = prepare.partition(selection, files)
        self.assertEqual((len(runtime),len(immutable),len(mutable)), (10,12,2))
        self.assertEqual(sorted(e["path"] for e in runtime+immutable+mutable), sorted(e["path"] for e in files))
        for bad in ({**selection,"target_page_id":pages[0]}, {**selection,"before_page_ids":pages[:5]}, {**selection,"target_page_id":"00000000-0000-0000-0000-000000000000"}):
            with self.assertRaises(ValueError): prepare.partition(bad,files)
        with self.assertRaises(ValueError): prepare.partition(selection, files[1:])
        document_substitute = [dict(e) for e in files]; document_substitute[4]["path"] = prefix + ".unreviewed"
        with self.assertRaises(ValueError): prepare.partition(selection, document_substitute)
        substituted = [dict(e) for e in files]; substituted[-1]["path"] = "/usr/lib/unreviewed.so"
        with self.assertRaises(ValueError): prepare.partition(selection, substituted)

if __name__ == "__main__": unittest.main()
