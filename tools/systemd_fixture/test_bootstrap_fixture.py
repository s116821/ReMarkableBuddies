"""Offline rejection tests for the fixture evidence verifier; no SSH or services."""
import copy
import io
import json
import tarfile
import unittest

from bootstrap_fixture import PROPERTIES, states, verify_archive, verify_states


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.original = {
            role: dict(zip(PROPERTIES, (role, "active", "running", "10", "0", "",
                                       "original-" + role, "100", "success")))
            for role in ("actor", "recovery", "stock", "buddy")
        }
        self.first = copy.deepcopy(self.original)
        for role in ("actor", "recovery"):
            self.first[role].update(ActiveState="inactive", SubState="dead", MainPID="0")
        for role in ("stock", "buddy"):
            self.first[role]["InvocationID"] = "restored-" + role
        self.second = copy.deepcopy(self.first)
        self.second["recovery"]["ExecMainStartTimestampMonotonic"] = "200"

    def check(self, case="none"):
        verify_states(case, lambda value: value, self.original, self.first, self.second)

    def test_normal_and_second_helper_without_restart(self):
        self.check()

    def test_refuse_a_failed_actor_despite_both_services_running(self):
        self.first["actor"]["Result"] = "exit-code"
        with self.assertRaisesRegex(AssertionError, "selected completion/fault"):
            self.check()

    def test_require_an_actual_second_helper_invocation(self):
        self.second["recovery"]["ExecMainStartTimestampMonotonic"] = "100"
        with self.assertRaisesRegex(AssertionError, "second helper"):
            self.check()

    def test_refuse_a_helper_that_restarted_an_active_service(self):
        self.second["stock"]["InvocationID"] = "unexpected-third-start"
        with self.assertRaisesRegex(AssertionError, "restarted an active"):
            self.check()

    def test_refuse_unchanged_services_when_the_test_never_stopped_them(self):
        self.first["stock"]["InvocationID"] = self.original["stock"]["InvocationID"]
        self.second["stock"]["InvocationID"] = self.original["stock"]["InvocationID"]
        with self.assertRaisesRegex(AssertionError, "not exercised"):
            self.check()

    def test_partial_stop_preserves_original_stock(self):
        self.first["actor"]["Result"] = "exit-code"
        self.first["stock"] = self.original["stock"].copy()
        self.second["stock"] = self.original["stock"].copy()
        self.check("after_buddy_stop")

    def test_uncertain_start_cannot_be_replayed(self):
        self.first["recovery"]["Result"] = "signal"
        self.second["recovery"]["Result"] = "exit-code"
        for snapshots in (self.first, self.second):
            for role in ("stock", "buddy"):
                snapshots[role].update(ActiveState="inactive", SubState="dead", MainPID="0")
        self.check("recovery_after_intent")
        self.second["stock"].update(ActiveState="active", SubState="running", MainPID="12")
        with self.assertRaisesRegex(AssertionError, "replayed"):
            self.check("recovery_after_intent")

    def test_a_missing_first_helper_fault_cannot_pass(self):
        with self.assertRaisesRegex(AssertionError, "first helper"):
            self.check("recovery_after_start")
        self.first["recovery"]["Result"] = "signal"
        self.check("recovery_after_start")

    def test_source_mismatch_withholds_buddy_after_both_helpers(self):
        self.first["actor"]["Result"] = "exit-code"
        for sample in (self.first, self.second):
            sample["recovery"]["Result"] = "exit-code"
            sample["buddy"].update(ActiveState="inactive", SubState="dead", MainPID="0")
        self.check("source_mismatch")
        first, second = copy.deepcopy(self.first), copy.deepcopy(self.second)
        for which, role, change in (
            ("first", "buddy", {"ActiveState": "active", "MainPID": "12"}),
            ("second", "buddy", {"ActiveState": "active", "MainPID": "12"}),
            ("first", "stock", {"InvocationID": "original-stock"}),
            ("first", "stock", {"ActiveState": "inactive", "MainPID": "0"}),
            ("second", "stock", {"InvocationID": "third-stock"}),
            ("first", "recovery", {"Result": "success"}),
            ("second", "recovery", {"Result": "success"}),
        ):
            with self.subTest(which=which, role=role, change=change):
                self.first, self.second = copy.deepcopy(first), copy.deepcopy(second)
                getattr(self, which)[role].update(change)
                with self.assertRaises(AssertionError):
                    self.check("source_mismatch")

    def test_source_mismatch_requires_injection_and_exact_failed_preservation(self):
        nonce = "12" * 16
        stock_unit = f"rsdk-bootstrap-{nonce}-stock.service"
        stock = {"version": 1, "group": {"unit": stock_unit},
                 "snapshot": f"Id={stock_unit}\nActiveState=active\nSubState=running\nMainPID=12\n"
                             "ControlPID=0\nJob=\nInvocationID=new-stock\n"}
        entries = {"ownership.json": b'{"version":1}', "buddy.stop-intent": nonce.encode(),
                   "stock.stop-intent": nonce.encode(), "observer.launch-intent": nonce.encode(),
                   "stock.start-intent": nonce.encode(), "fixture-source-mismatch": nonce.encode(),
                   "observer.exit": b"exit status: 0", "actor-result-one": b"Err(source manifest changed)\n"}
        for attempt in ("one", "two"):
            entries[f"recovery-result-{attempt}"] = b"Err(source manifest changed)\n"
            entries[f"recovery-{attempt}.source-after"] = b"Err(source manifest changed)"
            entries[f"recovery-{attempt}.stock.restored"] = json.dumps(stock).encode()

        def check(values):
            output = io.BytesIO()
            with tarfile.open(fileobj=output, mode="w:gz") as archive:
                for name, data in values.items():
                    entry = tarfile.TarInfo("rsdk-control-" + nonce + "/" + name)
                    entry.size = len(data)
                    archive.addfile(entry, io.BytesIO(data))
            return verify_archive(output.getvalue(), nonce, "source_mismatch")

        check(entries)
        for leaf in ("fixture-source-mismatch", "observer.exit", "actor-result-one",
                     "recovery-result-two", "recovery-two.source-after", "recovery-two.stock.restored"):
            with self.subTest(missing=leaf):
                altered = dict(entries)
                del altered[leaf]
                with self.assertRaises(AssertionError):
                    check(altered)
        for leaf, value in (
            ("buddy.start-intent", nonce.encode()),
            ("recovery-one.buddy.start-intent", nonce.encode()),
            ("recovery-one.buddy.restored", b"{}"),
            ("recovery-two.source-after", b"Ok(())"),
            ("recovery-result-two", b"Err(unrelated failure)\n"),
            ("recovery-two.stock.restored", b"{}"),
            ("observer.exit", b"exit status: 1"),
        ):
            with self.subTest(changed=leaf):
                with self.assertRaises(AssertionError):
                    check({**entries, leaf: value})

    def test_duplicate_or_missing_snapshot_properties_are_rejected(self):
        data = "\n".join(k + "=" + v for k, v in self.original["stock"].items())
        self.assertEqual(states(data.encode())["stock"], self.original["stock"])
        for bad in (data + "\nId=stock", data.replace("MainPID=10\n", "")):
            with self.assertRaises(ValueError):
                states(bad.encode())

    def test_archive_requires_effect_evidence_and_refuses_links(self):
        nonce = "12" * 16
        prefix = "rsdk-control-" + nonce + "/"

        def archive(entries, link=False):
            output = io.BytesIO()
            with tarfile.open(fileobj=output, mode="w:gz") as target:
                for name, data in entries.items():
                    entry = tarfile.TarInfo(prefix + name)
                    entry.size = len(data)
                    if link:
                        entry.type = tarfile.SYMTYPE
                        entry.linkname = "/etc/passwd"
                    target.addfile(entry, io.BytesIO(data))
            return output.getvalue()

        entries = {"ownership.json": b'{"version":1}', "buddy.stop-intent": nonce.encode()}
        verify_archive(archive(entries), nonce, "after_buddy_stop")
        with self.assertRaisesRegex(AssertionError, "missing selected"):
            verify_archive(archive(entries), nonce, "none")
        with self.assertRaises(ValueError):
            verify_archive(archive(entries, link=True), nonce, "after_buddy_stop")
        entries["stock.stop-intent"] = nonce.encode()
        with self.assertRaisesRegex(AssertionError, "partial-stop"):
            verify_archive(archive(entries), nonce, "after_buddy_stop")

        owner = {"unit": f"rsdk-bootstrap-{nonce}-actor.service", "mount_path": "/cgroup",
                 "group": {"dev": 1, "ino": 2}, "context": {"boot": "same"},
                 "parent": {"pid": 10, "start_ticks": 20}}
        ready = {"pid": 11, "start_ticks": 21}
        child = {**owner, "parent": ready}
        entries.update({"ownership.json": json.dumps({"version": 1, "actor": json.dumps({"group": owner})}).encode(),
                        "observer.launch-intent": nonce.encode(), "stock.start-intent": nonce.encode(),
                        "fixture-child-ready": json.dumps(ready).encode(),
                        "fixture-child-ready-complete": b"",
                        "fixture-child-observed": json.dumps(child).encode()})
        verify_archive(archive(entries), nonce, "actor_hang")
        entries["fixture-child-observed"] = b"{"  # expiry during witness write
        with self.assertRaises(ValueError):
            verify_archive(archive(entries), nonce, "actor_hang")
        child["group"] = {"dev": 1, "ino": 3}
        entries["fixture-child-observed"] = json.dumps(child).encode()
        with self.assertRaisesRegex(AssertionError, "identity/group"):
            verify_archive(archive(entries), nonce, "actor_hang")


if __name__ == "__main__":
    unittest.main()
