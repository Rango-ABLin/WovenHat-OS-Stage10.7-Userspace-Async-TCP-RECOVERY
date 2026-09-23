"""Reject false Wi-Fi acceptance, even when QEMU exits successfully."""
import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    'runtime_harness',
    Path(__file__).resolve().parents[1] / 'scripts/test-stage10-runtime.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)


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


if __name__ == '__main__':
    unittest.main()
