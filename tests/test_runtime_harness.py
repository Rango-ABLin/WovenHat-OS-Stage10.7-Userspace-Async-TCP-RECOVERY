<<<<<<< HEAD
"""Reject incomplete Wi-Fi evidence even when QEMU exits successfully."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'runtime_harness', Path(__file__).resolve().parents[1] / 'scripts/test-stage10-runtime.py')
=======
"""Reject false Wi-Fi acceptance, even when QEMU exits successfully."""
import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    'runtime_harness',
    Path(__file__).resolve().parents[1] / 'scripts/test-stage10-runtime.py')
>>>>>>> ad20d1a331df81e46ae48575036f1f520d5a6270
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)


<<<<<<< HEAD
class WifiEvidenceTests(unittest.TestCase):
    def run_gate(self, markers, exit_code=33):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'scripts').mkdir()
            qemu = root / 'qemu.exe'
            firmware = root / 'firmware.fd'
            qemu.touch()
            firmware.touch()

            def run(command, **kwargs):
                if command[0] == 'cargo':
                    return subprocess.CompletedProcess(command, 0, 'image.img', '')
                serial_arg = command[command.index('-serial') + 1]
                Path(serial_arg.removeprefix('file:')).write_text('\n'.join(markers))
                return subprocess.CompletedProcess(command, exit_code)

            argv = ['harness', '--stage', '13.9', '--qemu', str(qemu),
                    '--firmware', str(firmware)]
            with patch.object(harness, '__file__', str(root / 'scripts' / 'harness.py')), \
                    patch.object(harness.sys, 'argv', argv), \
                    patch.object(harness.subprocess, 'run', side_effect=run), \
                    patch('builtins.print'):
                return harness.main()

    def complete_markers(self):
        return ['[SMP] online=1 expected=1',
                '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
                *harness.WIFI_REQUIRED_MARKERS]

    def test_complete_evidence_passes(self):
        self.assertEqual(self.run_gate(self.complete_markers()), 0)

    def test_old_wifi_gate_is_insufficient(self):
        self.assertEqual(self.run_gate([
            '[SMP] online=1 expected=1',
            '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
            '[S13.9H] GTK + encrypted key data: PASSED']), 1)

    def test_every_wifi_marker_is_required(self):
        for marker in harness.WIFI_REQUIRED_MARKERS:
            with self.subTest(marker=marker):
                markers = self.complete_markers()
                markers.remove(marker)
                self.assertEqual(self.run_gate(markers), 1)

    def test_latest_stage_is_in_contract(self):
        self.assertIn('[S13.10AC] deferred AX200 IRQ -> CSR RX service: PASSED',
                      harness.WIFI_REQUIRED_MARKERS)

    def test_markers_do_not_override_bad_exit(self):
        self.assertEqual(self.run_gate(self.complete_markers(), exit_code=1), 1)
=======
class RuntimeAcceptanceTests(unittest.TestCase):
    def test_complete_wifi_evidence_passes(self):
        required = harness.required_markers('13.9', 4)
        self.assertFalse(harness.validation_errors(33, '\n'.join(required), required))

    def test_old_wifi_gate_cannot_certify_ac(self):
        old_log = ('[SMP] online=4 expected=4\n'
                   '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED\n'
                   '[S13.9H] GTK + encrypted key data: PASSED')
        errors = harness.validation_errors(
            33, old_log, harness.required_markers('13.9', 4))
        self.assertTrue(any('[S13.10AC]' in error for error in errors))

    def test_every_wifi_prerequisite_is_required(self):
        required = harness.required_markers('13.9', 2)
        for missing in required:
            with self.subTest(missing=missing):
                log = '\n'.join(marker for marker in required if marker != missing)
                self.assertTrue(harness.validation_errors(33, log, required))

    def test_failure_or_panic_overrides_success_markers(self):
        required = harness.required_markers('13.9', 1)
        for failure in ('[S13.10AC] deferred service: FAILED', 'KERNEL PANIC',
                        '[S13.10AC] deferred service: TIMEOUT'):
            with self.subTest(failure=failure):
                log = '\n'.join([*required, failure])
                self.assertTrue(harness.validation_errors(33, log, required))

    def test_wrong_exit_or_cpu_count_fails(self):
        required = harness.required_markers('13.9', 4)
        self.assertTrue(harness.validation_errors(0, '\n'.join(required), required))
        self.assertTrue(harness.validation_errors(
            33, '\n'.join(harness.required_markers('13.9', 1)), required))
>>>>>>> ad20d1a331df81e46ae48575036f1f520d5a6270


if __name__ == '__main__':
    unittest.main()
