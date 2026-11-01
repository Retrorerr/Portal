import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('portal_run', Path(__file__).resolve().parents[1] / 'assets/portal-run.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class PortableAppTests(unittest.TestCase):
    def image(self, path, machine=183):
        header = bytearray(20)
        header[:6] = b'\x7fELF\x02\x01'
        header[8:11] = b'AI\x02'
        header[18:20] = machine.to_bytes(2, 'little')
        path.write_bytes(header)
        path.chmod(0o700)

    def test_bundled_library_and_plugin_paths_survive(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'app=with spaces'
            self.image(path)
            data = bytearray(path.read_bytes())
            data[8:11] = b'\0\0\0'
            path.write_bytes(data)
            with patch.dict(os.environ, {'QT_PLUGIN_PATH': '/my/plugins', 'LD_LIBRARY_PATH': '/my/libraries'}), patch.object(runner.platform, 'machine', return_value='aarch64'):
                command, env = runner.launch(path, ['a b', '$(id)'])
            self.assertEqual(command, [str(path), 'a b', '$(id)'])
            self.assertEqual(env['QT_PLUGIN_PATH'], '/my/plugins')
            self.assertEqual(env['LD_LIBRARY_PATH'], '/my/libraries')

    def test_extraction_cache_is_content_addressed_and_retries_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            image, cache = base / 'app.AppImage', base / 'cache'
            self.image(image)
            calls = []
            def extract(command, cwd, **kwargs):
                calls.append(command)
                appdir = Path(cwd) / 'squashfs-root'
                appdir.mkdir()
                (appdir / 'AppRun').write_text('#!/bin/sh\nexit 0\n')
                return subprocess.CompletedProcess(command, 0)
            with patch.object(runner.platform, 'machine', return_value='aarch64'), patch.object(runner.subprocess, 'run', side_effect=extract):
                first = runner.extract_appimage(image, cache)
                self.assertEqual(first, runner.extract_appimage(image, cache))
                self.assertEqual(len(calls), 1)
                image.write_bytes(image.read_bytes() + b'updated')
                second = runner.extract_appimage(image, cache)
                self.assertNotEqual(first, second)
                self.assertTrue((first / 'AppRun').is_file())
            image.write_bytes(image.read_bytes() + b'incomplete')
            with patch.object(runner.platform, 'machine', return_value='aarch64'), patch.object(runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)):
                with self.assertRaises(runner.Unsupported):
                    runner.extract_appimage(image, cache)
            self.assertFalse(list(cache.glob('.extract-*')))
            self.assertTrue((first / 'AppRun').is_file())

    def test_missing_translator_and_java_are_precise_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'app'
            self.image(path, 3)
            with patch.object(runner.platform, 'machine', return_value='aarch64'), patch.object(runner.shutil, 'which', return_value=None):
                with self.assertRaisesRegex(runner.Unsupported, '32-bit x86.*FEX'):
                    runner.translator(path)
                jar = Path(tmp) / 'app.jar'
                jar.touch()
                with self.assertRaisesRegex(runner.Unsupported, 'default-jre'):
                    runner.launch(jar, [])

    def test_windows_apps_reuse_installed_proton_with_a_separate_prefix(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            steam = base / 'Steam with spaces'
            proton = steam / 'steamapps/common/Proton 11.0 (ARM64)/proton'
            proton.parent.mkdir(parents=True)
            proton.touch()
            tool = base / 'portal-proton.sh'
            tool.touch()
            env = dict(STEAM_COMPAT_CLIENT_INSTALL_PATH=str(steam), XDG_DATA_HOME=str(base / 'data'),
                       QT_PLUGIN_PATH='/bundle/plugins', LIBVA_DRIVER_NAME='termuxva')
            with patch.object(runner, 'PROTON_TOOL', tool), patch.object(runner.shutil, 'which', return_value=None):
                command, actual = runner.windows_command(base / 'test.exe', ['a b'], env)
                self.assertEqual(command, [str(tool), 'run', str(base / 'test.exe'), 'a b'])
                self.assertEqual(actual['STEAM_COMPAT_DATA_PATH'], str(base / 'data/portal/windows'))
                self.assertEqual(actual['QT_PLUGIN_PATH'], '/bundle/plugins')
                self.assertEqual(actual['LIBVA_DRIVER_NAME'], 'termuxva')
                selected = dict(env, STEAM_COMPAT_DATA_PATH='/my/chosen/prefix')
                self.assertEqual(runner.windows_command(base / 'test.msi', [], selected)[1]['STEAM_COMPAT_DATA_PATH'], '/my/chosen/prefix')
                self.assertNotIn('STEAM_COMPAT_DATA_PATH', env)

    def test_explicit_wine_prefix_and_invalid_proton_are_not_replaced(self):
        with patch.object(runner.shutil, 'which', return_value='/usr/bin/wine'), patch.object(runner, 'translator', return_value=[]):
            env = {'WINEPREFIX': '/my/existing/wine'}
            command, actual = runner.windows_command(Path('/app.exe'), [], env)
            self.assertEqual(command, ['/usr/bin/wine', '/app.exe'])
            self.assertEqual(actual, env)
        with patch.object(runner.shutil, 'which', return_value=None):
            with self.assertRaisesRegex(runner.Unsupported, 'WINEPREFIX'):
                runner.windows_command(Path('/app.exe'), [], env)
            with self.assertRaisesRegex(runner.Unsupported, 'selected Proton'):
                runner.windows_command(Path('/app.exe'), [], {'PORTAL_PROTON': '/missing/proton'})

    def test_x86_wine_uses_its_installed_translator(self):
        with tempfile.TemporaryDirectory() as tmp:
            wine = Path(tmp) / 'wine64'
            self.image(wine, 62)
            def which(name):
                return str(wine) if name == 'wine' else '/usr/bin/box64' if name == 'box64' else None
            env = {'WINEPREFIX': '/my/prefix', 'BOX64_LD_LIBRARY_PATH': '/my/x86/libraries'}
            with patch.object(runner.shutil, 'which', side_effect=which), patch.object(runner.platform, 'machine', return_value='aarch64'):
                command, actual = runner.windows_command(Path('/app.exe'), ['a b'], env)
            self.assertEqual(command, ['/usr/bin/box64', str(wine), '/app.exe', 'a b'])
            self.assertEqual(actual, env)


if __name__ == '__main__':
    unittest.main()
