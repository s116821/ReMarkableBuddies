"""Guard real ELF failure classes, including version needs from weak imports."""
import copy
import unittest
from abi import parse_elf, verify_runtime

HEADERS = """  Class: ELF32
  Machine: ARM
  Flags: 0x5000400, Version5 EABI, hard-float ABI
 [Requesting program interpreter: /lib/ld-linux-armhf.so.3]
 0x00000001 (NEEDED) Shared library: [libc.so.6]
"""
SYMBOLS = """ 1: 00000000 0 FUNC GLOBAL DEFAULT UND read@GLIBC_2.4 (2)
 2: 00000000 0 FUNC WEAK DEFAULT UND pidfd_spawnp@GLIBC_2.39 (3)
"""
VERSIONS = """Version needs section '.gnu.version_r' contains 1 entry:
 0x000000: Version: 1 File: libc.so.6 Cnt: 2
 0x000010: Name: GLIBC_2.4 Flags: none Version: 2
 0x000020: Name: GLIBC_2.39 Flags: none Version: 3
"""


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.artifact = parse_elf(HEADERS, SYMBOLS, VERSIONS)
        self.baseline = dict(format=1, target="armv7-unknown-linux-gnueabihf",
                             firmware_baseline="fixture", sdk_metadata_revision="fixture",
                             source_basis="synthetic", providers={"libc.so.6": {
                                 "symbols": ["read@GLIBC_2.4", "pidfd_spawnp@GLIBC_2.39"],
                                 "sha256": "fixture", "native_provider_hash_matched": False}})

    def test_weak_symbol_version_still_requires_loader_provider_definition(self):
        verify_runtime(self.artifact, self.baseline["target"], self.baseline)
        self.baseline["providers"]["libc.so.6"]["symbols"].remove("pidfd_spawnp@GLIBC_2.39")
        with self.assertRaisesRegex(ValueError, "required version definitions"):
            verify_runtime(self.artifact, self.baseline["target"], self.baseline)

    def test_wrong_architecture_interpreter_float_abi_or_missing_provider_refuses(self):
        for key, value in [("machine", "AArch64"), ("interpreter", "/wrong/loader"), ("flags", "soft-float ABI")]:
            artifact = copy.deepcopy(self.artifact)
            artifact[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                verify_runtime(artifact, self.baseline["target"], self.baseline)
        self.baseline["providers"] = {}
        with self.assertRaisesRegex(ValueError, "NEEDED provider"):
            verify_runtime(self.artifact, self.baseline["target"], self.baseline)

    def test_missing_strong_symbol_and_unmapped_provider_are_not_accepted(self):
        self.baseline["providers"]["libc.so.6"]["symbols"].remove("read@GLIBC_2.4")
        with self.assertRaisesRegex(ValueError, "required symbols"):
            verify_runtime(self.artifact, self.baseline["target"], self.baseline)
        with self.assertRaisesRegex(ValueError, "qualified provider"):
            parse_elf(HEADERS, SYMBOLS.replace("(2)", "(99)"), VERSIONS)
