"""Reject incomplete Wi-Fi evidence even when QEMU exits successfully."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'runtime_harness', Path(__file__).resolve().parents[1] / 'scripts/test-stage10-runtime.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)


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


if __name__ == '__main__':
    unittest.main()
