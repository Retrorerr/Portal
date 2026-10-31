import shlex
import unittest

from scripts.guest_exec import command_arguments, find_process_pid, parse_environment, Session


class GuestExecProcessLookupTests(unittest.TestCase):
    def test_matches_exact_process_name_not_another_process_command_line(self):
        table = ('USER PID PPID VSZ RSS WCHAN ADDR S NAME\n'
                 'u0_a509 20191 20084 1768716 186204 0 0 S kwin_wayland\n'
                 'u0_a509 20381 20076 145000 72000 0 0 S plasmashell\n')
        self.assertEqual(find_process_pid(table, 'plasmashell'), '20381')

    def test_explicit_columns(self):
        self.assertEqual(find_process_pid('PID PPID NAME\n42 1 plasmashell\n', 'plasmashell'), '42')
        self.assertIsNone(find_process_pid('PID PPID NAME\n42 1 kwin_wayland\n', 'plasmashell'))

    def test_environment_preserves_equals_and_unicode(self):
        self.assertEqual(parse_environment('X=a=b\0TEXT=日本語\0'.encode()), {'X': 'a=b', 'TEXT': '日本語'})

    def record(self, engine):
        return dict(version=1, engine=engine, program=f'/native/lib{engine}.so', user='desktop',
                    args=['-r', '/actual/runtime-A', '-b', '/graphics:/usr/lib/graphics'],
                    env={'PROOT_LOADER': '/native/loader'} if engine == 'proot' else {})

    def test_preserves_engine_mounts_and_session_values(self):
        for engine in ('tawcroot', 'proot'):
            with self.subTest(engine=engine):
                env = dict(USER='desktop', HOME='/home/desktop', WAYLAND_DISPLAY='wayland-7',
                           DBUS_SESSION_BUS_ADDRESS='unix:path=/tmp/a=b', LD_LIBRARY_PATH='/bundled/lib',
                           QT_PLUGIN_PATH='/custom/plugins', CUSTOM='$(echo bad) `id` 日本語', SHLVL='9')
                args = command_arguments(self.record(engine), env, "printf '%s' test")
                self.assertEqual(args[:2], ['/system/bin/sh', '-c'])
                args = shlex.split(args[2].split('exec ', 1)[1])
                self.assertIn('/actual/runtime-A', args)
                self.assertIn('/graphics:/usr/lib/graphics', args)
                self.assertIn('WAYLAND_DISPLAY=wayland-7', args)
                self.assertIn('CUSTOM=' + env['CUSTOM'], args)
                self.assertIn('QT_PLUGIN_PATH=/custom/plugins', args)
                self.assertNotIn('SHLVL=9', args)
                self.assertIn('/usr/sbin/runuser', args)
                self.assertEqual(shlex.split(shlex.join(args)), args)

    def test_privilege_requires_explicit_flag(self):
        env = dict(USER='desktop', HOME='/home/desktop')
        args = command_arguments(self.record('tawcroot'), env, 'id', root=True)
        args = shlex.split(args[2].split('exec ', 1)[1])
        self.assertIn('HOME=/root', args)
        self.assertNotIn('/usr/sbin/runuser', args)
        with self.assertRaises(RuntimeError):
            command_arguments(self.record('tawcroot'), dict(USER='root', HOME='/root'), 'id')

    def test_refuses_unknown_record_or_stable_before_adb(self):
        with self.assertRaises(RuntimeError):
            command_arguments(dict(version=2), {}, 'id')
        with self.assertRaises(RuntimeError):
            Session.connect(package='app.polarbear.portal')


if __name__ == '__main__':
    unittest.main()
