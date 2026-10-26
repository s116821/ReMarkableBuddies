"""Check real CLI notification startup isolation in a disposable Linux container.

Uses local Unix sockets and poisoned config, never PID1, input devices or providers.
The application binary path is the only argument.
"""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile


def main():
    assert sys.platform == "linux" and os.geteuid() == 0
    assert Path("/.dockerenv").is_file()
    assert not Path("/run/systemd/system").exists()
    directory = Path("/run/reader-buddy-resident")
    assert not os.path.lexists(directory), "existing installation must not be touched"
    binary = str(Path(sys.argv[1]).resolve(strict=True))
    checks = 0
    with tempfile.TemporaryDirectory(prefix="buddy-health-entry-") as temporary:
        root = Path(temporary)
        config = root / "poison.toml"
        config.write_text("CONFIG_MUST_FAIL = [\n", encoding="utf-8")
        environment = {
            "PATH": "/usr/bin:/bin", "HOME": temporary,
            "REMARKABLE_BUDDIES_CONFIG": str(config),
        }
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver:
            address = str(root / "notify.sock")
            receiver.bind(address)
            receiver.setblocking(False)

            def run(arguments=(), settings=None):
                nonlocal checks
                result = subprocess.run(
                    [binary, *arguments], cwd=root,
                    env={**environment, **(settings or {})},
                    stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=5,
                )
                checks += 1
                assert not (root / ".local").exists(), "ordinary Store was initialized"
                try:
                    receiver.recv(128)
                except BlockingIOError:
                    pass
                else:
                    raise AssertionError("notification before successful ordinary initialization")
                return result

            valid = {"NOTIFY_SOCKET": address, "WATCHDOG_USEC": "120000000"}
            failed_init = run(settings=valid)
            assert failed_init.returncode != 0
            assert "service notification" not in failed_init.stderr.lower()
            assert "watchdog" not in failed_init.stderr.lower()
            invalid = {**valid, "WATCHDOG_USEC": "60000000"}
            refused = run(settings=invalid)
            assert refused.returncode != 0
            assert "Unsupported service watchdog interval" in refused.stderr
            wrong_owner = run(settings={**valid, "WATCHDOG_PID": "0"})
            assert wrong_owner.returncode != 0 and "Service watchdog owner mismatch" in wrong_owner.stderr
            for arguments in [
                ("--writer-note",), ("--configuration-operation", "inspect"),
                ("--simulate", str(root / "absent-scenario.json")),
            ]:
                separate = run(arguments, invalid)
                assert "Unsupported service watchdog interval" not in separate.stderr
            directory.mkdir(mode=0o700)
            try:
                recovery = run(("--resident-stock-recovery",), invalid)
                assert recovery.returncode == 0, recovery.stderr
                assert recovery.stdout.strip() == "no-recorded-activation; stock was not inspected or changed"
                assert list(directory.iterdir()) == []
            finally:
                directory.rmdir()
            (root / ".env").write_text(
                f"NOTIFY_SOCKET={address}\nWATCHDOG_USEC=60000000\n", encoding="utf-8"
            )
            dotenv = run()
            assert dotenv.returncode != 0
            assert "Unsupported service watchdog interval" not in dotenv.stderr
    print(f"{checks} bounded health-entry cases passed; no PID1 or tablet involved")


if __name__ == "__main__":
    main()
