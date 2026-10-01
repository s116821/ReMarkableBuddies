"""Refuse incomplete downloaded inventory before any release asset upload."""
import json
from pathlib import Path
import tempfile
import unittest
from build import Release, TARGETS, sha256, verify_packages


class PackageTests(unittest.TestCase):
    def test_missing_extra_corrupt_or_wrong_source_packages_refuse(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            release = Release("v0.2.1", "a" * 40)
            packages = {}
            for target in TARGETS:
                path = root / f"reader-buddy-{target}.tar.gz"
                path.write_bytes(b"fixture package")
                packages[path.name] = sha256(path)
            manifest = dict(tag=release.tag, sha=release.sha, version="0.2.1", packages=packages)
            (root / "provenance.json").write_text(json.dumps(manifest))
            verify_packages(root, release)
            path = root / next(iter(packages))
            path.unlink()
            with self.assertRaisesRegex(ValueError, "inventory"):
                verify_packages(root, release)
            path.write_bytes(b"fixture package")
            extra = root / "unexpected.zip"
            extra.write_bytes(b"extra")
            with self.assertRaisesRegex(ValueError, "inventory"):
                verify_packages(root, release)
            extra.unlink()
            path.write_bytes(b"corrupt package")
            with self.assertRaisesRegex(ValueError, "checksum"):
                verify_packages(root, release)
            with self.assertRaisesRegex(ValueError, "source"):
                verify_packages(root, Release(release.tag, "b" * 40))
