"""Exercise the real recovery CLI in a disposable Linux container, never a tablet.

Requires root, /.dockerenv, no running systemd, and no existing installed latch.
The binary path is the sole argument. All calls are bounded and use fixture data.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    assert sys.platform == "linux" and os.geteuid() == 0
    assert Path("/.dockerenv").is_file(), "disposable container required"
    assert not Path("/run/systemd/system").exists(), "must not run against PID1 services"
    binary = str(Path(sys.argv[1]).resolve(strict=True))
    directory = Path("/run/reader-buddy-resident")
    assert not os.path.lexists(directory), "preexisting installation must not be touched"
    record = directory / "activation.json"
    checks = 0
    with tempfile.TemporaryDirectory(prefix="buddy-recovery-entry-") as temporary:
        root = Path(temporary)
        config = root / "poison.toml"
        config.write_text("CONFIG_MUST_NOT_BE_READ = [\n", encoding="utf-8")
        (root / ".env").write_text("OPENAI_API_KEY=DOTENV_MUST_NOT_BE_READ\n", encoding="utf-8")
        environment = {
            "PATH": "/usr/bin:/bin",
            "HOME": temporary,
            "REMARKABLE_BUDDIES_CONFIG": str(config),
            "OPENAI_API_KEY": "FIXTURE_ONLY_MUST_NOT_BE_USED",
        }

        def run(*arguments):
            nonlocal checks
            result = subprocess.run(
                [binary, *arguments], cwd=root, env=environment,
                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=5,
            )
            checks += 1
            assert "FIXTURE_ONLY_MUST_NOT_BE_USED" not in result.stdout + result.stderr
            assert "DOTENV_MUST_NOT_BE_READ" not in result.stdout + result.stderr
            assert not (root / ".local").exists(), "ordinary Store initialized"
            assert config.read_text() == "CONFIG_MUST_NOT_BE_READ = [\n"
            return result

        missing = run("--resident-stock-recovery")
        assert missing.returncode != 0 and "installed lease directory absent" in missing.stderr
        assert not os.path.lexists(directory)
        directory.mkdir(mode=0o700)
        try:
            empty = run("--resident-stock-recovery")
            assert empty.returncode == 0, empty.stderr
            assert empty.stdout.strip() == "no-recorded-activation; stock was not inspected or changed"
            assert list(directory.iterdir()) == []
            for arguments in [
                ("--writer-note",), ("--configuration-operation", "inspect"),
                ("--model", "fixture-model"), ("--simulate", "missing.json"),
                ("--lease-directory", "/tmp/alternate"), ("--unit", "other.service"),
            ]:
                mixed = run("--resident-stock-recovery", *arguments)
                assert mixed.returncode != 0 and mixed.stdout == ""
                assert list(directory.iterdir()) == []
            for data in [b"", b"{", b"{}", b'{"version":1}']:
                record.write_bytes(data)
                record.chmod(0o600)
                restored = run("--resident-stock-recovery")
                assert restored.returncode != 0 and restored.stdout == ""
                ordinary = run()
                assert ordinary.returncode != 0 and ordinary.stdout == ""
                assert "installed activation latch refuses ordinary startup" in ordinary.stderr
                assert record.read_bytes() == data
                record.unlink()
            record.symlink_to(root / "missing-record")
            assert run("--resident-stock-recovery").returncode != 0
            assert run().returncode != 0
            assert record.is_symlink() and not (root / "missing-record").exists()
            record.unlink()
            record.mkdir(mode=0o700)
            assert run("--resident-stock-recovery").returncode != 0
            record.rmdir()
            os.mkfifo(record, 0o600)
            assert run("--resident-stock-recovery").returncode != 0
            record.unlink()
            directory.chmod(0o755)
            assert run("--resident-stock-recovery").returncode != 0
            assert run().returncode != 0
            directory.chmod(0o700)
        finally:
            # Remove only this fixture's known leaf; never recursively erase state.
            if os.path.lexists(record):
                if record.is_dir() and not record.is_symlink():
                    record.rmdir()
                else:
                    record.unlink()
            directory.rmdir()
    print(f"{checks} bounded recovery/startup CLI cases passed; no PID1 or tablet involved")


if __name__ == "__main__":
    main()
