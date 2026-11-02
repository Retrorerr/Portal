from pathlib import Path
import re
import subprocess
import tempfile
import unittest

TOOL = Path(__file__).resolve().parents[1] / 'assets/steam/compat/portal-proton.sh'
# Run only the overlay function, not the whole Proton wrapper.
FUNCTION = re.search(r'^ubisoft_overlay_off\(\) \{\n.*?^\}\n', TOOL.read_text(), re.S | re.M)[0]


class UbisoftOverlayTests(unittest.TestCase):
    def run_in(self, prefix):
        subprocess.run(['sh', '-c', FUNCTION + 'ubisoft_overlay_off "$1"', 'sh', str(prefix)], check=True)

    def prefix(self, tmp, launcher=True):
        prefix = Path(tmp)
        drive = prefix / 'pfx/drive_c'
        for user in ('steamuser', 'Public'):
            (drive / 'users' / user / 'AppData/Local').mkdir(parents=True)
        if launcher:
            (drive / 'Program Files (x86)/Ubisoft/Ubisoft Game Launcher').mkdir(parents=True)
        return prefix, drive / 'users/steamuser/AppData/Local/Ubisoft Game Launcher/settings.yaml'

    def test_first_launch_seeds_the_overlay_off(self):
        with tempfile.TemporaryDirectory() as tmp:
            prefix, yaml = self.prefix(tmp)
            self.run_in(prefix)
            self.assertEqual(yaml.read_text(), 'overlay:\n  enabled: false\n  warning_enabled: false\n')
            self.assertFalse((prefix / 'pfx/drive_c/users/Public/AppData/Local/Ubisoft Game Launcher').exists())

    def test_existing_settings_turn_the_overlay_off_and_keep_the_rest(self):
        with tempfile.TemporaryDirectory() as tmp:
            prefix, yaml = self.prefix(tmp)
            yaml.parent.mkdir()
            yaml.write_bytes(b'misc:\r\n  enabled: true\r\noverlay:\r\n  enabled: true\r\n  forceunhookgame: false\r\n  warning_enabled: true\r\n')
            self.run_in(prefix)
            self.assertEqual(yaml.read_bytes(),
                             b'misc:\r\n  enabled: true\r\noverlay:\r\n  enabled: false\r\n  forceunhookgame: false\r\n  warning_enabled: false\r\n')

    def test_prefixes_without_ubisoft_connect_are_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            prefix, yaml = self.prefix(tmp, launcher=False)
            self.run_in(prefix)
            self.assertFalse(yaml.parent.exists())


if __name__ == '__main__':
    unittest.main()
