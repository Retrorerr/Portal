import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

LAUNCHER = Path(__file__).resolve().parents[1] / 'assets/steam/portal-steam.sh'
BOX64_TOOL = LAUNCHER.parent / 'compat-box64/portal-box64.sh'


class SteamLauncherTests(unittest.TestCase):
    def launch(self, **overrides):
        with tempfile.TemporaryDirectory(prefix='portal-steam-launch-') as tmp:
            home = Path(tmp)
            runtime = home / 'Steam/steamrtarm64'
            runtime.mkdir(parents=True)
            client = runtime / 'steam'
            client.write_text('#!' + sys.executable + '\nimport os,json\nprint(json.dumps(dict(scale=os.environ.get("STEAM_FORCE_DESKTOPUI_SCALING"),args=__import__("sys").argv[1:])))\n')
            client.chmod(0o700)
            pidfile = home / '.steam/steam.pid'
            pidfile.parent.mkdir()
            pidfile.write_text('1234\n')
            env = {key: value for key, value in os.environ.items()
                   if key not in ('STEAM_FORCE_DESKTOPUI_SCALING', 'QT_SCALE_FACTOR', 'LD_PRELOAD')}
            env.update(HOME=tmp, PORTAL_STEAM_ROOT=str(home / 'Steam'), **overrides)
            result = subprocess.run(['sh', str(LAUNCHER), 'steam://test argument'], env=env,
                                    capture_output=True, text=True, timeout=15, check=True)
            settings = json.loads(result.stdout)
            settings['overlay_links'] = {
                name: (home / '.steam' / name).readlink().relative_to(home).as_posix()
                for name in ('bin', 'bin32', 'bin64')
            }
            settings['legacy_pid'] = (home / '.steampid').read_text()
            return settings

    def test_native_scaling_is_not_forced_from_a_qt_setting(self):
        for env in ({}, dict(QT_SCALE_FACTOR='2.625')):
            result = self.launch(**env)
            self.assertIsNone(result['scale'])
            self.assertEqual(result['args'], ['steam://test argument'])

    def test_explicit_steam_scaling_is_preserved(self):
        self.assertEqual(self.launch(STEAM_FORCE_DESKTOPUI_SCALING='1.5', QT_SCALE_FACTOR='3')['scale'], '1.5')

    def test_first_launch_sets_valve_overlay_paths(self):
        self.assertEqual(self.launch()['overlay_links'], {
            'bin': '.steam/bin32',
            'bin32': 'Steam/ubuntu12_32',
            'bin64': 'Steam/ubuntu12_64',
        })

    def test_forwarding_a_url_keeps_steams_pid(self):
        self.assertEqual(self.launch()['legacy_pid'], '1234\n')


class SteamBox64OverlayTests(unittest.TestCase):
    def test_x86_game_loads_only_the_emulated_steam_renderer(self):
        with tempfile.TemporaryDirectory(prefix='portal-steam-overlay-') as tmp:
            root = Path(tmp)
            renderers = []
            # elf_kind reads the ELF magic and e_machine from the header.
            for architecture, machine in (('arm64', 0xB7), ('i386', 0x03), ('x86_64', 0x3E)):
                renderer = root / architecture / 'gameoverlayrenderer.so'
                renderer.parent.mkdir()
                renderer.write_bytes(b'\x7fELF' + bytes(14) + machine.to_bytes(2, 'little'))
                renderers.append(renderer)
            game = root / 'game'
            game.write_bytes(renderers[-1].read_bytes())
            shim = root / 'portal-shim.so'
            shim.write_bytes(renderers[0].read_bytes())
            box64 = root / 'box64'
            box64.write_text('#!/bin/sh\nprintf "native=%s\\nemulated=%s\\n" "$LD_PRELOAD" "$BOX64_LD_PRELOAD"\n')
            box64.chmod(0o700)
            env = dict(os.environ, PORTAL_BOX64=str(box64), TMPDIR=tmp, SteamAppId='overlay-test',
                       LD_PRELOAD=f'{renderers[0]}:{renderers[1]}:{renderers[2]}{shim}',
                       BOX64_LD_PRELOAD='extra.so')
            subprocess.run(['sh', str(BOX64_TOOL), 'run', '--', str(game)], env=env,
                           capture_output=True, text=True, timeout=15, check=True)
            output = (root / 'portal-box64-overlay-test.log').read_text().splitlines()
            self.assertIn(f'native={shim}', output)
            self.assertIn(f'emulated=extra.so:{renderers[-1]}', output)


if __name__ == '__main__':
    unittest.main()
