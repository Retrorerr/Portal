import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SOURCE = (Path(__file__).resolve().parents[1] / 'src/android/proot/setup.rs').read_text()
HELPER = SOURCE.split('const CHROMIUM_ENTRIES_HELPER: &str = r#"', 1)[1].split('"#;', 1)[0]


class DesktopLauncherTests(unittest.TestCase):
    def test_new_entries_use_native_backend_and_preserve_existing_custom_entries(self):
        with tempfile.TemporaryDirectory(prefix='portal-launcher-') as tmp:
            root = Path(tmp)
            apps = root / 'vendor/applications'; apps.mkdir(parents=True)
            bundle = root / 'electron/resources'; bundle.mkdir(parents=True)
            (bundle / 'app.asar').write_text('Owned runtime fixture')
            binary = bundle.parent / 'example'; binary.write_text('#!/bin/sh\nexit 0\n'); binary.chmod(0o700)
            source = f'[Desktop Entry]\nName=Example\nExec={binary} %U\n'
            (apps / 'example.desktop').write_text(source)
            script = HELPER.replace('/usr/share/applications', str(apps)).replace('/usr/local/share/applications', str(root / 'absent'))
            env = dict(os.environ, HOME=str(root / 'home'), XDG_DATA_HOME=str(root / 'data'))
            subprocess.run(['sh', '-c', script], env=env, check=True)
            entry = root / 'data/applications/example.desktop'
            generated = entry.read_text()
            self.assertIn('--enable-wayland-ime', generated)
            self.assertNotIn('--no-stdio-init', generated)
            self.assertNotIn('--ozone-platform=', generated)
            for custom in (source, generated + '# User customization\n'):
                entry.write_text(custom)
                subprocess.run(['sh', '-c', script], env=env, check=True)
                self.assertEqual(entry.read_text(), custom)
            entry.unlink()
            entry.symlink_to(root / 'missing-user-launcher')
            subprocess.run(['sh', '-c', script], env=env, check=True)
            self.assertEqual(entry.readlink(), root / 'missing-user-launcher')


if __name__ == '__main__':
    unittest.main()
