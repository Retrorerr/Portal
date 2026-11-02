from pathlib import Path
import os
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
# Load the real service functions without starting any system services.
FUNCTIONS = (ROOT / 'assets/portal-system-services.sh').read_text().rsplit('\ncase "$1" in', 1)[0]


class StaleBusPidTests(unittest.TestCase):
    def stop(self, directory, pid):
        directory = Path(directory)
        (directory / 'pid').write_text(f'{pid}\n')
        # pkill must not reach the host's own daemons.
        (directory / 'pkill').write_text('#!/bin/sh\nexit 1\n')
        (directory / 'pkill').chmod(0o755)
        env = dict(os.environ, PATH=str(directory) + os.pathsep + os.environ['PATH'])
        script = FUNCTIONS + f'\npidfile={directory}/pid\nsocket={directory}/socket\nstop'
        return subprocess.run(['/bin/sh', '-c', script], env=env, capture_output=True, text=True)

    def spawn(self, directory, name):
        # comm is the executable's basename.
        executable = Path(directory) / name
        executable.symlink_to('/bin/sleep')
        process = subprocess.Popen([str(executable), '30'])
        self.addCleanup(process.wait)
        self.addCleanup(process.kill)
        deadline = time.monotonic() + 5
        while Path(f'/proc/{process.pid}/comm').read_text().strip() != name[:15]:
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.01)
        return process

    def test_reused_pid_of_another_process_survives(self):
        with tempfile.TemporaryDirectory() as tmp:
            process = self.spawn(tmp, 'app.polarbear')
            self.stop(tmp, process.pid)
            time.sleep(0.2)
            self.assertIsNone(process.poll())
            self.assertFalse((Path(tmp) / 'pid').exists())

    def test_previous_bus_is_stopped(self):
        with tempfile.TemporaryDirectory() as tmp:
            process = self.spawn(tmp, 'dbus-daemon')
            self.stop(tmp, process.pid)
            self.assertIsNotNone(process.wait(timeout=5))

    def test_malformed_pidfile_is_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = self.stop(tmp, '-1')
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
