"""Real loader regressions for mixed-version and RTLD_LOCAL Qt hosts."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from scripts.build_qt_animation import build

CORE = '_ZN16QCoreApplication4execEv'
GUI = '_ZN15QGuiApplication4execEv'
WIDGETS = '_ZN12QApplication4execEv'
VERSION = '_Z8qVersionv'
INSTANCE = '_ZN13QUnifiedTimer8instanceEb'
INTERVAL = '_ZN13QUnifiedTimer17setTimingIntervalEi'
PRIMARY = '_ZN15QGuiApplication13primaryScreenEv'
REFRESH = '_ZNK7QScreen11refreshRateEv'


class QtAnimationLoaderTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='portal-qt-loader-')
        cls.directory = Path(cls.temporary.name)
        build(cls.directory / 'preload.so', target='')
        def shared(name, code, exports, dependencies=()):
            source = cls.directory / (name + '.c')
            source.write_text(code)
            version_map = cls.directory / (name + '.map')
            version_map.write_text(exports)
            subprocess.run(['cc', '-shared', '-fPIC', str(source), '-o', str(cls.directory / name),
                            '-Wl,-soname,' + name, '-Wl,--version-script=' + str(version_map),
                            '-L' + str(cls.directory), '-Wl,-rpath,$ORIGIN',
                            '-Wl,--no-as-needed', *['-l:' + d for d in dependencies]], check=True)
        for major, minor in [(5, 15), (6, 8)]:
            tag = f'Qt_{major}'
            core, gui, widgets = [f'libQt{major}{name}.so.{major}' for name in ('Core', 'Gui', 'Widgets')]
            shared(core, f'''static int interval = 16;
const char *{VERSION}(void) {{ const char *v = __builtin_getenv("PORTAL_FAKE_QT_VERSION"); return v && {major} == 6 ? v : "{major}.{minor}.2"; }}
void *{INSTANCE}(int create) {{ return &interval; }}
void {INTERVAL}(void *p, int value) {{ *(int *)p = value; }}
int {CORE}(void) {{ return {major}00 + interval; }}
'''.replace('__builtin_getenv', 'getenv').replace('static int interval', '#include <stdlib.h>\nstatic int interval'),
                   f'{tag}_PRIVATE_API {{ global: {INSTANCE}; {INTERVAL}; }};\n{tag} {{ global: {VERSION}; {CORE}; local: *; }};')
            shared(gui, f'''extern int {CORE}(void);
int {GUI}(void) {{ return {CORE}(); }}
void *{PRIMARY}(void) {{ return (void *)1; }}
double {REFRESH}(const void *screen) {{ return 144.0; }}
''', f'{tag} {{ global: {GUI}; {PRIMARY}; {REFRESH}; local: *; }};', [core])
            shared(widgets, f'extern int {GUI}(void); int {WIDGETS}(void) {{ return {GUI}(); }}',
                   f'{tag} {{ global: {WIDGETS}; local: *; }};', [gui])
            source = cls.directory / f'plugin{major}.c'
            source.write_text(f'extern int {WIDGETS}(void); int run(void) {{ return {WIDGETS}(); }}')
            subprocess.run(['cc', '-shared', '-fPIC', str(source), '-o', str(cls.directory / f'plugin{major}.so'),
                            '-L' + str(cls.directory), '-l:' + widgets, '-Wl,-rpath,$ORIGIN'], check=True)
        source = cls.directory / 'host.c'
        source.write_text('''#include <dlfcn.h>
#include <stdio.h>
int main(void) {
 void *a = dlopen("plugin6.so", RTLD_NOW | RTLD_GLOBAL);
 void *b = dlopen("plugin5.so", RTLD_NOW | RTLD_LOCAL);
 if (!a || !b) { puts(dlerror()); return 1; }
 int (*six)(void) = dlsym(a, "run");
 int (*five)(void) = dlsym(b, "run");
 printf("%d %d\\n", five(), six());
 return 0;
}''')
        subprocess.run(['cc', str(source), '-o', str(cls.directory / 'host'), '-ldl'], check=True)
        shared('libQtCore.so.4', f'int {CORE}(void) {{ return 416; }}',
               f'{{ global: {CORE}; local: *; }};')
        shared('libQtGui.so.4', f'extern int {CORE}(void); int {WIDGETS}(void) {{ return {CORE}(); }}',
               f'{{ global: {WIDGETS}; local: *; }};', ['libQtCore.so.4'])
        source = cls.directory / 'legacy.c'
        source.write_text(f'''#include <stdio.h>
extern int {WIDGETS}(void);
int main(void) {{ printf("%d\\n", {WIDGETS}()); return 0; }}''')
        subprocess.run(['cc', str(source), '-o', str(cls.directory / 'legacy'),
                        '-L' + str(cls.directory), '-l:libQtGui.so.4', '-Wl,-rpath,$ORIGIN'], check=True)
        shared('libQt5Core-bundled.so.5', f'int {CORE}(void) {{ return 523; }}',
               f'Qt_5 {{ global: {CORE}; local: *; }};')
        source = cls.directory / 'bundled-plugin.c'
        source.write_text(f'extern int {CORE}(void); int run(void) {{ return {CORE}(); }}')
        subprocess.run(['cc', '-shared', '-fPIC', str(source), '-o', str(cls.directory / 'bundled-plugin.so'),
                        '-L' + str(cls.directory), '-l:libQt5Core-bundled.so.5', '-Wl,-rpath,$ORIGIN'], check=True)
        source = cls.directory / 'bundled-host.c'
        source.write_text('''#include <dlfcn.h>
#include <stdio.h>
int main(void) {
 void *p=dlopen("bundled-plugin.so", RTLD_NOW|RTLD_LOCAL);
 if (!p) { puts(dlerror()); return 1; }
 int (*run)(void)=dlsym(p,"run"); printf("%d\\n", run()); return 0;
}''')
        subprocess.run(['cc', str(source), '-o', str(cls.directory / 'bundled-host'), '-ldl'], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def run_host(self, **overrides):
        env = {k: v for k, v in os.environ.items() if k not in ('LD_PRELOAD', 'PORTAL_QT_ANIMATION_INTERVAL_MS')}
        env.update(LD_LIBRARY_PATH=str(self.directory), LD_PRELOAD=str(self.directory / 'preload.so'))
        env.update(overrides)
        return subprocess.check_output([str(self.directory / 'host')], env=env, text=True).strip()

    def test_global_qt6_and_local_qt5_keep_their_own_exec_and_timer(self):
        self.assertEqual(self.run_host(), '506 606')

    def test_explicit_disable_preserves_native_timers(self):
        self.assertEqual(self.run_host(PORTAL_QT_ANIMATION_INTERVAL_MS='0'), '516 616')

    def test_unverified_qt_minor_never_calls_private_timer_api(self):
        self.assertEqual(self.run_host(PORTAL_FAKE_QT_VERSION='6.10.0'), '506 616')

    def test_unversioned_legacy_qt_exec_remains_native(self):
        env = dict(os.environ, LD_LIBRARY_PATH=str(self.directory), LD_PRELOAD=str(self.directory / 'preload.so'))
        result = subprocess.check_output([str(self.directory / 'legacy')], env=env, text=True)
        self.assertEqual(result.strip(), '416')

    def test_build_does_not_embed_its_output_path(self):
        other = self.directory / 'different-name.so'
        build(other, target='')
        self.assertEqual(other.read_bytes(), (self.directory / 'preload.so').read_bytes())

    def test_local_bundled_qt_with_renamed_soname_remains_loadable(self):
        env = dict(os.environ, LD_LIBRARY_PATH=str(self.directory), LD_PRELOAD=str(self.directory / 'preload.so'))
        result = subprocess.check_output([str(self.directory / 'bundled-host')], env=env, text=True)
        self.assertEqual(result.strip(), '523')


if __name__ == '__main__':
    unittest.main()
