"""Offline rejection tests for the fixture evidence verifier; no SSH or services."""
import copy
import io
import itertools
import json
import tarfile
import unittest
from unittest.mock import Mock, patch

from bootstrap_fixture import (PROPERTIES, Run, digest, read_control_archive,
                               recovery_results, states, verify_archive, verify_states)


def control_archive(nonce, entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, data in entries.items():
            member = tarfile.TarInfo("rsdk-control-" + nonce + "/" + name)
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))
    return output.getvalue()


class CompletionTests(unittest.TestCase):
    def setUp(self):
        self.run = Run.__new__(Run)
        self.run.nonce = "12" * 16
        self.run.receipt = {}
        self.run.save = Mock()
        self.samples = {
            self.run.unit(role): dict(zip(PROPERTIES, (self.run.unit(role), "inactive", "dead",
                                                       "0", "0", "", "", "0", "success")))
            for role in ("actor", "recovery", "stock", "buddy")}
        self.one = "recovery-result-" + "34" * 16
        self.two = "recovery-result-" + "56" * 16
        self.actor = "actor-result-" + "78" * 16
        self.entries = {self.one: b"Ok(())\n", self.actor: b"Ok(())\n"}
        self.run.show = Mock(side_effect=lambda roles: self.snapshot())
        self.run.archive = Mock(side_effect=lambda: control_archive(self.run.nonce, self.entries))

    def snapshot(self):
        return "\n\n".join("\n".join(k + "=" + v for k, v in sample.items())
                           for sample in self.samples.values()).encode()

    def wait(self, **kwargs):
        with patch("bootstrap_fixture.time.monotonic", side_effect=itertools.count(0, 40)), \
                patch("bootstrap_fixture.time.sleep"):
            return self.run.wait(**kwargs)

    def test_cleared_success_and_distinct_second_success(self):
        first = self.wait()
        prior = dict(self.run.receipt["first_results"])
        self.assertEqual(prior, {self.one: digest(b"Ok(())\n")})
        self.entries[self.two] = b"Ok(())\n"
        second = self.wait(previous="0", previous_results=prior)
        self.assertEqual(first, second)  # PID1 snapshots alone cannot distinguish.
        self.assertEqual(set(self.run.receipt["second_results"]), {self.one, self.two})
        self.assertEqual(self.run.show.call_count, 4)  # Fresh state after each archive.

    def test_missing_or_stale_result_cannot_complete(self):
        for entries, prior in (({self.actor: b"Ok(())\n"}, {}),
                               (self.entries, {self.one: digest(b"Ok(())\n")})):
            with self.subTest(prior=prior):
                self.entries = entries
                with self.assertRaises(TimeoutError):
                    self.wait(previous="0", previous_results=prior)
                self.assertNotIn("second_results", self.run.receipt)

    def test_changed_missing_extra_or_malformed_result_refuses(self):
        prior = {self.one: digest(b"Ok(())\n")}
        for entries in ({self.one: b"Err(changed)\n", self.two: b"Ok(())\n"},
                        {self.two: b"Ok(())\n"},
                        {self.one: b"Ok(())\n", self.two: b"Ok(())\n",
                         "recovery-result-" + "90" * 16: b"Ok(())\n"},
                        {self.one: b"Ok(())\n", "recovery-result-bad": b"Ok(())\n"}):
            with self.subTest(entries=entries):
                self.entries = {self.actor: b"Ok(())\n", **entries}
                with self.assertRaises((AssertionError, ValueError)):
                    self.wait(previous="0", previous_results=prior)
                self.assertNotIn("second_results", self.run.receipt)

    def test_success_requires_exact_bytes_and_actor_result(self):
        for entries in ({self.one: b"Ok(())"}, {self.one: b"Err(failed)\n"},
                        {self.one: b"Ok(())\n"},
                        {self.one: b"Ok(())\n", self.actor: b"Err(failed)\n"}):
            with self.subTest(entries=entries):
                self.entries = entries
                with self.assertRaises(TimeoutError):
                    self.wait()
                self.assertNotIn("first_results", self.run.receipt)

    def test_failed_collection_and_pending_closing_state_refuse(self):
        self.run.archive.side_effect = RuntimeError("collection failed")
        with self.assertRaisesRegex(RuntimeError, "collection failed"):
            self.wait()
        self.run.archive.side_effect = lambda: control_archive(self.run.nonce, self.entries)
        first = self.snapshot()
        self.samples[self.run.unit("recovery")]["Job"] = "99"
        self.run.show.side_effect = itertools.chain([first], itertools.repeat(self.snapshot()))
        with self.assertRaises(TimeoutError):
            self.wait()
        self.assertNotIn("first_results", self.run.receipt)

    def test_retained_signal_fault_without_result_then_success(self):
        self.entries = {self.actor: b"Ok(())\n"}
        self.samples[self.run.unit("recovery")].update(
            ActiveState="failed", SubState="failed", Result="signal",
            ExecMainStartTimestampMonotonic="123", InvocationID="original-helper")
        self.wait()
        self.assertEqual(self.run.receipt["first_results"], {})
        self.samples[self.run.unit("recovery")].update(
            ActiveState="inactive", SubState="dead", Result="success",
            ExecMainStartTimestampMonotonic="0", InvocationID="")
        self.entries[self.two] = b"Ok(())\n"
        self.wait(previous="123", previous_results={})
        self.assertEqual(self.run.receipt["second_results"], {self.two: digest(b"Ok(())\n")})

    def test_duplicate_archive_records_and_other_nonce_refuse(self):
        packed = control_archive("aa" * 16, self.entries)
        with self.assertRaises(ValueError):
            read_control_archive(packed, self.run.nonce)
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as archive:
            for _ in range(2):
                member = tarfile.TarInfo("rsdk-control-" + self.run.nonce + "/" + self.one)
                member.size = 7
                archive.addfile(member, io.BytesIO(b"Ok(())\n"))
        with self.assertRaises(ValueError):
            recovery_results(read_control_archive(output.getvalue(), self.run.nonce))

    def test_only_final_archive_matches_cleanup_receipt_selection(self):
        calls = []

        def remote(label, command, required):
            calls.append({"label": f"{len(calls) + 1:03d}-" + label, "returncode": 0})
            return b"archive"

        self.run.remote = remote
        Run.archive(self.run)
        Run.archive(self.run)
        Run.archive(self.run, required=False, label="control-archive")
        selected = [call for call in calls if call["label"].endswith("-control-archive")]
        self.assertEqual(selected, [calls[-1]])
        self.assertEqual(selected[0]["returncode"], 0)


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

    def test_equal_cleared_timestamps_require_distinct_durable_success(self):
        for sample in (self.first, self.second):
            sample["recovery"].update(ExecMainStartTimestampMonotonic="0", InvocationID="")
        one = {"recovery-result-" + "12" * 16: digest(b"Ok(())\n")}
        two = {**one, "recovery-result-" + "34" * 16: digest(b"Ok(())\n")}
        verify_states("none", lambda v: v, self.original, self.first, self.second, (one, two))
        for completion in (None, (one, one), (two, one)):
            with self.subTest(completion=completion):
                with self.assertRaises(AssertionError):
                    verify_states("none", lambda v: v, self.original, self.first, self.second, completion)

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
