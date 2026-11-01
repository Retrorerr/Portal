from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
# Load the real service functions without starting any system services.
FUNCTIONS = (ROOT / 'assets/portal-system-services.sh').read_text().rsplit('\ncase "$1" in', 1)[0]


class FlatpakEnvironmentTests(unittest.TestCase):
    def setup_case(self, directory, overrides=''):
        directory = Path(directory)
        (directory / 'overrides').write_text(overrides)
        executable = directory / 'flatpak'
        executable.write_text('#!' + sys.executable + '\n' + '''
import json,os,pathlib,sys
base=pathlib.Path(os.environ['FLATPAK_FIXTURE'])
if sys.argv[1:]==['override','--system','--show']:
 print((base/'overrides').read_text(),end='')
else:
 with (base/'calls').open('a') as f:f.write(json.dumps(sys.argv[1:])+'\\n')
 if os.environ.get('FAIL_OVERRIDE')=='1':sys.exit(1)
''')
        executable.chmod(0o755)
        return dict(os.environ, PATH=str(directory) + os.pathsep + os.environ['PATH'], FLATPAK_FIXTURE=str(directory))

    def migrate(self, directory, env, *defaults):
        return subprocess.run(['/bin/sh', '-c', FUNCTIONS + '\nflatpak_env_defaults "$@"',
                               'test', str(Path(directory) / 'state/environment-v1'), *defaults],
                              env=env, capture_output=True, text=True)

    def calls(self, directory):
        path = Path(directory) / 'calls'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_only_missing_environment_defaults_are_changed(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.setup_case(tmp, '[Context]\nfilesystems=home:ro;\n'
                                      'unset-environment=TURNIP_KMD;\n[Environment]\nFD_FORCE_KGSL=0\n')
            result = self.migrate(tmp, env, 'FD_FORCE_KGSL=1', 'FD_KGSL_ENABLE_DMABUF=1', 'TURNIP_KMD=kgsl')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.calls(tmp), [['override', '--system', '--env=FD_KGSL_ENABLE_DMABUF=1']])

    def test_later_user_removals_are_preserved(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.setup_case(tmp)
            self.assertEqual(self.migrate(tmp, env, 'FD_FORCE_KGSL=1').returncode, 0)
            self.assertEqual(self.migrate(tmp, env, 'FD_FORCE_KGSL=1').returncode, 0)
            self.assertEqual(len(self.calls(tmp)), 1)

    def test_failed_write_retries_and_interrupted_marker_does_not_replace_choices(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.setup_case(tmp)
            self.assertNotEqual(self.migrate(tmp, dict(env, FAIL_OVERRIDE='1'), 'FD_FORCE_KGSL=1').returncode, 0)
            marker = Path(tmp) / 'state/environment-v1'
            self.assertFalse(marker.exists())
            # The override succeeded before an interrupted marker write.
            (Path(tmp) / 'overrides').write_text('[Environment]\nFD_FORCE_KGSL=1\n')
            self.assertEqual(self.migrate(tmp, env, 'FD_FORCE_KGSL=1').returncode, 0)
            self.assertTrue(marker.exists())
            self.assertEqual(len(self.calls(tmp)), 1)

    def test_renderer_choices_survive_legacy_variable_cleanup(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = self.setup_case(tmp, '[Environment]\nMESA_LOADER_DRIVER_OVERRIDE=custom\n')
            result = self.migrate(tmp, env, 'MESA_LOADER_DRIVER_OVERRIDE', 'GALLIUM_DRIVER')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.calls(tmp), [['override', '--system', '--unset-env=GALLIUM_DRIVER']])


if __name__ == '__main__':
    unittest.main()
