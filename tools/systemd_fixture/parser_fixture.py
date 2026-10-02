"""Original offline 255.21 parser fixtures. No unit or job is executed."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def main():
    version = subprocess.check_output(["systemctl", "--version"], text=True).splitlines()[0]
    if version != "systemd 255 (255.21)":
        raise RuntimeError("exact upstream parser version required")
    source = subprocess.check_output(["git", "-C", "/opt/systemd-source", "rev-parse", "HEAD"], text=True).strip()
    if source != "70500d37992a01d3275b1c414c3ed161d6f91f9e":
        raise RuntimeError("source identity mismatch")
    results = []
    with tempfile.TemporaryDirectory(prefix="buddy-systemd-parser-") as directory:
        root = Path(directory)
        root.chmod(0o755)  # Fake public units must be readable by test-mode nobody.
        for scenario in ("empty-dependency", "full-fragment", "separate-unit", "generic-dropin"):
            base = root / scenario
            vendor, runtime = base / "vendor", base / "runtime"
            vendor.mkdir(parents=True)
            runtime.mkdir()
            generators = base / "no-generators"
            generators.mkdir()
            original = "[Unit]\nDefaultDependencies=no\nOnFailure=fixture-fail.service\n[Service]\nExecStart=/usr/bin/sleep infinity\nRestart=on-failure\nRestartMode=direct\n"
            marker = "[Unit]\nDefaultDependencies=no\n[Service]\nType=oneshot\nExecStart=/usr/bin/true\n"
            write(vendor / "fixture-stock.service", original)
            before = sha(vendor / "fixture-stock.service")
            for name in ("fixture-fail.service", "fixture-ownedfail.service", "fixture-genericfail.service"):
                write(vendor / name, marker)
            unit = "fixture-stock.service"
            expected = {"fixture-fail.service"}
            if scenario == "empty-dependency":
                write(runtime / (unit + ".d") / "owned.conf", "[Unit]\nOnFailure=\nOnFailure=fixture-ownedfail.service\n[Service]\nRestart=no\n")
                expected.add("fixture-ownedfail.service")
            elif scenario == "full-fragment":
                write(runtime / unit, "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/usr/bin/sleep infinity\n")
                write(vendor / (unit + ".d") / "vendor.conf", "[Unit]\nOnFailure=fixture-fail.service\n")
            else:
                unit = "fixture-owned.service"
                write(runtime / unit, "[Unit]\nDefaultDependencies=no\n[Service]\nExecStart=/usr/bin/sleep infinity\n")
                expected = set()
                if scenario == "generic-dropin":
                    write(vendor / "service.d" / "generic.conf", "[Unit]\nOnFailure=fixture-genericfail.service\n")
                    expected.add("fixture-genericfail.service")
            environment = os.environ.copy()
            environment["SYSTEMD_UNIT_PATH"] = str(runtime) + ":" + str(vendor)
            environment["SYSTEMD_GENERATOR_PATH"] = str(generators)
            completed = subprocess.run(["runuser", "-u", "nobody", "--", "env",
                                        "SYSTEMD_UNIT_PATH=" + environment["SYSTEMD_UNIT_PATH"],
                                        "SYSTEMD_GENERATOR_PATH=" + str(generators),
                                        "/usr/lib/systemd/systemd", "--system", "--test",
                                        "--unit=" + unit, "--log-target=console"],
                                       text=True, capture_output=True, timeout=20)
            if completed.returncode:
                raise RuntimeError(completed.stderr)
            pattern = r"\t-> Unit " + re.escape(unit) + r":\n(.*?)(?=\t-> Unit |-> By jobs:)"
            match = re.search(pattern, completed.stdout, re.S)
            if not match:
                raise RuntimeError("selected unit absent from parsed dump")
            block = match.group(1)
            actual = set(re.findall(r"^\s+OnFailure: ([^ ]+)", block, re.M))
            if actual != expected or sha(vendor / "fixture-stock.service") != before:
                raise RuntimeError("dependency or baseline assertion failed: " + scenario)
            if scenario == "empty-dependency" and "Restart: no" not in block:
                raise RuntimeError("runtime service override not effective")
            results.append({"scenario": scenario, "on_failure": sorted(actual),
                            "vendor_sha": before, "vendor_unchanged": True,
                            "selected_unit": unit, "dump_sha": hashlib.sha256(completed.stdout.encode()).hexdigest(),
                            "unit_execution": False, "job_execution": False})
    print(json.dumps({"scope": "offline parser and initial transaction construction only",
                      "version": version, "source": source,
                      "systemd_sha": sha(Path("/usr/lib/systemd/systemd")),
                      "compiler": subprocess.check_output(["cc", "--version"], text=True).splitlines()[0],
                      "results": results}, indent=2))


if __name__ == "__main__":
    main()
