"""Pin the unmodified upstream binary used by the Rust container parser tests."""
import hashlib
from pathlib import Path
import unittest


class FirmwareFixtureTests(unittest.TestCase):
    def test_upstream_binary_is_unmodified(self):
        fixture = Path(__file__).parent / 'fixtures' / 'iwlwifi' / 'iwlwifi-cc-a0-77.ucode'
        self.assertEqual(len(fixture.read_bytes()), 1368100)
        self.assertEqual(hashlib.sha256(fixture.read_bytes()).hexdigest(),
                         '94f5fb915f2074f98059365e11cca313e25a6fa3b7cd6772e80b3ba5edfb966b')
