"""Host-side acceptance must require both real TCP exchanges to finish."""
import importlib.util
from pathlib import Path
import threading
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location(
    'tcp_harness', Path(__file__).resolve().parents[1] / 'scripts/test-stage10-7-tcp.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)


class HostExchangeTests(unittest.TestCase):
    def run_server(self, payloads=(), stopped=False, bind_error=None):
        stop, ready, finished = (threading.Event() for _ in range(3))
        if stopped:
            stop.set()
        errors = []
        server = MagicMock()
        server.__enter__.return_value = server
        if bind_error:
            server.bind.side_effect = bind_error
        connections = []
        for payload in payloads:
            conn = MagicMock()
            conn.__enter__.return_value = conn
            conn.recv.side_effect = [payload, b'']
            connections.append(conn)
        server.accept.side_effect = [(conn, None) for conn in connections]
        with patch.object(harness.socket, 'socket', return_value=server):
            harness.serve(stop, errors, ready, finished)
        self.assertTrue(ready.is_set())
        return errors, finished, connections

    def test_stopped_before_exchange_is_failure(self):
        errors, finished, _ = self.run_server(stopped=True)
        self.assertTrue(errors)
        self.assertFalse(finished.is_set())

    def test_both_exchanges_and_eof_are_required(self):
        errors, finished, connections = self.run_server([harness.TX, harness.TX])
        self.assertFalse(errors)
        self.assertTrue(finished.is_set())
        connections[1].sendall.assert_called_once_with(harness.RX)
        connections[1].shutdown.assert_called_once_with(harness.socket.SHUT_WR)

    def test_corrupt_guest_payload_fails(self):
        errors, finished, _ = self.run_server([b'bad'])
        self.assertTrue(errors)
        self.assertFalse(finished.is_set())

    def test_listener_error_wakes_startup_waiter(self):
        errors, finished, _ = self.run_server(bind_error=OSError('port in use'))
        self.assertTrue(errors)
        self.assertFalse(finished.is_set())


if __name__ == '__main__':
    unittest.main()
