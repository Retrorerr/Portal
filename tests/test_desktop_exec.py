import os
from pathlib import Path
import shlex
import signal
import sys
import tempfile
import time
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from desktop_exec_broker import run_job


class DesktopExecTests(unittest.TestCase):
    def test_preserves_output_arguments_environment_and_failure_status(self):
        with tempfile.TemporaryDirectory(prefix='portal exec 日本語 ') as tmp:
            directory = Path(tmp)
            env = dict(os.environ, HOME=tmp, PROBE_VALUE='value with spaces 日本語')
            code = 'import os,sys; print(os.environ["PROBE_VALUE"]); print(sys.argv[1],file=sys.stderr); sys.exit(7)'
            request = dict(command=shlex.join([sys.executable, '-c', code, 'a b']), environment=env, timeout=5)
            result = run_job(directory, request)
            self.assertEqual(result['returncode'], 7)
            self.assertEqual((directory / 'stdout').read_text(), env['PROBE_VALUE'] + '\n')
            self.assertEqual((directory / 'stderr').read_text(), 'a b\n')

    def test_timeout_terminates_the_owned_process_group(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            marker = directory / 'should-not-exist'
            child = 'import time,pathlib; time.sleep(0.5); pathlib.Path(' + repr(str(marker)) + ').touch()'
            parent = 'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",' + repr(child) + ']); time.sleep(20)'
            request = dict(command=shlex.join([sys.executable, '-c', parent]),
                           environment=dict(os.environ, HOME=tmp), timeout=0.15)
            result = run_job(directory, request)
            self.assertTrue(result['timed_out'])
            self.assertEqual(result['returncode'], 124)
            time.sleep(0.6)
            self.assertFalse(marker.exists(), 'A timed-out grandchild outlived the probe')

    def test_timeout_kills_a_descendant_that_ignores_term(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            pid_file = directory / 'child-pid'
            child = ('import os,pathlib,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); '
                     'pathlib.Path(' + repr(str(pid_file)) + ').write_text(str(os.getpid())); time.sleep(20)')
            parent = 'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",' + repr(child) + ']); time.sleep(20)'
            request = dict(command=shlex.join([sys.executable, '-c', parent]),
                           environment=dict(os.environ, HOME=tmp), timeout=0.5)
            result = run_job(directory, request)
            self.assertEqual(result['returncode'], 124)
            pid = int(pid_file.read_text())
            try:
                stat = Path(f'/proc/{pid}/stat')
                deadline = time.monotonic() + 1
                while stat.exists() and not stat.read_text().split(') ', 1)[1].startswith('Z ') and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(not stat.exists() or stat.read_text().split(') ', 1)[1].startswith('Z '),
                                'The shell exited but a TERM-resistant child survived')
            finally:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == '__main__':
    unittest.main()
