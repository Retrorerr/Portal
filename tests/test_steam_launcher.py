import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

LAUNCHER = Path(__file__).resolve().parents[1] / 'assets/steam/portal-steam.sh'


class SteamLauncherTests(unittest.TestCase):
    def launch(self, **overrides):
        with tempfile.TemporaryDirectory(prefix='portal-steam-launch-') as tmp:
            home = Path(tmp)
            runtime = home / 'Steam/steamrtarm64'
            runtime.mkdir(parents=True)
            client = runtime / 'steam'
            client.write_text('#!' + sys.executable + '\nimport os,json\nprint(json.dumps(dict(scale=os.environ.get("STEAM_FORCE_DESKTOPUI_SCALING"),args=__import__("sys").argv[1:])))\n')
            client.chmod(0o700)
            env = {key: value for key, value in os.environ.items()
                   if key not in ('STEAM_FORCE_DESKTOPUI_SCALING', 'QT_SCALE_FACTOR', 'LD_PRELOAD')}
            env.update(HOME=tmp, PORTAL_STEAM_ROOT=str(home / 'Steam'), **overrides)
            result = subprocess.run(['sh', str(LAUNCHER), 'steam://test argument'], env=env,
                                    capture_output=True, text=True, timeout=15, check=True)
            return json.loads(result.stdout)

    def test_native_scaling_is_not_forced_from_a_qt_setting(self):
        for env in ({}, dict(QT_SCALE_FACTOR='2.625')):
            result = self.launch(**env)
            self.assertIsNone(result['scale'])
            self.assertEqual(result['args'], ['steam://test argument'])

    def test_explicit_steam_scaling_is_preserved(self):
        self.assertEqual(self.launch(STEAM_FORCE_DESKTOPUI_SCALING='1.5', QT_SCALE_FACTOR='3')['scale'], '1.5')


if __name__ == '__main__':
    unittest.main()
