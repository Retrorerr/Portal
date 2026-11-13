import importlib.util
from pathlib import Path
import socket
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "assets/portal-screen-inhibit.py"
SPEC = importlib.util.spec_from_file_location("screen_inhibition", SOURCE)
SERVICE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SERVICE)


class ScreenInhibitionTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        address = str(Path(directory.name) / "display")
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(self.listener.close)
        self.listener.bind(address)
        self.listener.listen()
        self.listener.settimeout(1)
        self.state = SERVICE.Inhibitions(address)

    def accept(self):
        connection, _ = self.listener.accept()
        self.addCleanup(connection.close)
        connection.settimeout(1)
        return connection

    def test_keeps_one_lease_until_the_last_display_request_ends(self):
        first = self.state.add(":1.1", display=True)
        connection = self.accept()
        second = self.state.add(":1.2", display=True)
        self.state.release(":1.2", first)
        self.state.release(":1.1", first)
        self.assertIsNotNone(self.state.connection)
        self.state.release(":1.2", second)
        self.assertEqual(connection.recv(1), b"")

    def test_owner_disconnection_releases_display_even_with_a_suspend_request(self):
        self.state.add(":1.1", display=False)
        self.assertIsNone(self.state.connection)
        self.state.add(":1.2", display=True)
        connection = self.accept()
        self.state.disconnected(":1.2")
        self.assertEqual(connection.recv(1), b"")
        self.assertIsNone(self.state.connection)
        self.assertEqual(list(self.state.requests.values()), [(":1.1", False)])


if __name__ == "__main__":
    unittest.main()
