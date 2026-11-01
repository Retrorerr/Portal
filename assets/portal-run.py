#!/usr/bin/python3 -I
"""Launch portable desktop applications without replacing their libraries."""
import argparse
import fcntl
import hashlib
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile


class Unsupported(RuntimeError):
    pass


PROTON_TOOL = Path('/usr/share/steam/compatibilitytools.d/portal-proton-arm64/portal-proton')


def steam_libraries(environment):
    root = Path(environment.get('STEAM_COMPAT_CLIENT_INSTALL_PATH', str(Path.home() / '.local/share/Steam')))
    libraries = [root]
    try:
        lines = (root / 'steamapps/libraryfolders.vdf').read_text().splitlines()
    except OSError:
        return libraries
    for line in lines:
        match = re.fullmatch(r'\s*"path"\s*"((?:\\.|[^"\\])*)"\s*', line)
        if match:
            value = re.sub(r'\\([\\"])', r'\1', match[1])
            library = Path(value)
            if library.is_absolute() and library not in libraries:
                libraries.append(library)
    return libraries


def windows_command(path, arguments, environment):
    wine = shutil.which('wine') or shutil.which('wine64')
    selection = environment.get('PORTAL_PROTON', '')
    windows_args = ['msiexec', '/i', str(path)] if path.suffix.lower() == '.msi' else [str(path)]
    if wine and not selection:
        return [*translator(wine), wine, *windows_args, *arguments], environment
    if environment.get('WINEPREFIX') and not selection:
        raise Unsupported('WINEPREFIX is set but Wine is missing. Install Wine for that prefix, or select Proton and set STEAM_COMPAT_DATA_PATH.')
    libraries = steam_libraries(environment)
    if selection not in ('', 'experimental', 'stable'):
        installed = Path(selection).is_file()
        if not installed:
            raise Unsupported(f'The selected Proton does not exist: {selection}')
    else:
        installed = any(p.is_file() for library in libraries for p in (library / 'steamapps/common').glob('Proton*/proton')
                        if 'arm64' in p.parent.name.lower()
                        and (selection != 'stable' or 'experimental' not in p.parent.name.lower()))
    if not PROTON_TOOL.is_file() or not installed:
        raise Unsupported('Windows runtime missing. Install Wine, or install Proton for ARM64 through Steam. Existing Windows prefixes and game data are preserved.')
    env = dict(environment)
    # Use Portal's existing ARM64EC/FEX integration, with a separate prefix
    # for non-Steam apps. Never borrow or reset a game's Proton prefix.
    env.setdefault('STEAM_COMPAT_CLIENT_INSTALL_PATH', str(libraries[0]))
    data = Path(env.get('XDG_DATA_HOME', str(Path.home() / '.local/share'))) / 'portal/windows'
    env.setdefault('STEAM_COMPAT_DATA_PATH', str(data))
    return [str(PROTON_TOOL), 'run', *windows_args, *arguments], env


def elf_machine(path):
    with open(path, 'rb') as stream:
        head = stream.read(20)
    if len(head) < 20 or head[:4] != b'\x7fELF' or head[5] not in (1, 2):
        return None
    return int.from_bytes(head[18:20], 'little' if head[5] == 1 else 'big')


def translator(path, machine=None):
    machine = elf_machine(path) if machine is None else machine
    native = {'aarch64': 183, 'arm64': 183, 'x86_64': 62}.get(platform.machine().lower())
    if machine is None or machine == native:
        return []
    if machine == 62:
        box64 = shutil.which('box64')
        if box64:
            return [box64]
    if machine in (3, 62):
        fex = shutil.which('FEXInterpreter')
        if fex:
            return [fex]
        name = '32-bit x86' if machine == 3 else 'x86-64'
        raise Unsupported(f'{name} Linux application: install ' +
                          ('FEX and configure its x86 RootFS.' if machine == 3 else 'Box64 (sudo apt install box64), or FEX with an x86 RootFS.'))
    raise Unsupported(f'ELF machine {machine} is not supported by the installed runtimes on {platform.machine()}.')


def appimage_type(path):
    with open(path, 'rb') as stream:
        head = stream.read(11)
    return head[10] if len(head) == 11 and head[:4] == b'\x7fELF' and head[8:10] == b'AI' else None


def extract_appimage(path, cache):
    if appimage_type(path) != 2:
        raise Unsupported('This AppImage uses an unsupported format. Obtain a type 2 AppImage or a .deb/Flatpak build.')
    if not os.access(path, os.X_OK):
        raise Unsupported('This AppImage is not executable. Enable its executable permission before running it.')
    def fingerprint():
        st = path.stat()
        return st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns
    before = fingerprint()
    with path.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if before != fingerprint():
        raise Unsupported('The AppImage changed while being read. Wait for its download to finish.')
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    destination = cache / digest
    with (cache / (digest + '.lock')).open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not (destination / '.complete').is_file():
            with tempfile.TemporaryDirectory(prefix='.extract-', dir=cache) as temporary:
                env = dict(os.environ)
                # The extraction runtime must not recursively choose its own
                # extract-and-run path. All application toolkit paths survive.
                env.pop('APPIMAGE_EXTRACT_AND_RUN', None)
                result = subprocess.run([*translator(path), str(path), '--appimage-extract'],
                                        cwd=temporary, env=env, stdout=subprocess.DEVNULL)
                extracted = Path(temporary) / 'squashfs-root'
                if result.returncode or not (extracted / 'AppRun').is_file():
                    raise Unsupported('AppImage extraction failed or AppRun is missing. The existing cache was preserved.')
                if before != fingerprint():
                    raise Unsupported('The AppImage changed during extraction. Retry after its download finishes.')
                (extracted / '.complete').write_text(digest + '\n')
                if destination.exists():
                    shutil.rmtree(destination)
                extracted.rename(destination)
    return destination


def launch(path, arguments, cache=None):
    path = Path(path).expanduser().absolute()
    if not path.is_file():
        raise Unsupported(f'Application not found: {path}')
    env = dict(os.environ)
    kind = appimage_type(path)
    if kind is not None:
        appdir = extract_appimage(path, cache or Path(env.get('XDG_CACHE_HOME', str(Path.home() / '.cache'))) / 'portal/appimages')
        env.update(APPIMAGE=str(path), APPDIR=str(appdir), ARGV0=str(path), OWD=os.getcwd())
        target = appdir / 'AppRun'
        return [*translator(target), str(target), *arguments], env
    if path.suffix.lower() == '.jar':
        java = shutil.which('java')
        if not java:
            raise Unsupported('Java runtime missing. Install it with: sudo apt install default-jre')
        return [java, '-jar', str(path), *arguments], env
    if path.suffix.lower() in ('.exe', '.msi'):
        return windows_command(path, arguments, env)
    if path.suffix.lower() == '.dll':
        dotnet = shutil.which('dotnet')
        if not dotnet:
            raise Unsupported('.NET runtime missing. Install the runtime required by this application before launching its DLL.')
        return [dotnet, str(path), *arguments], env
    return [*translator(path), str(path), *arguments], env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--gui', action='store_true', help='Show launch errors in a desktop dialog')
    parser.add_argument('application')
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    try:
        command, environment = launch(args.application, args.arguments)
        if command[0] == str(PROTON_TOOL):
            # Steam normally creates this directory before invoking Proton.
            # Standalone launches must also provide it before Proton locks it.
            Path(environment['STEAM_COMPAT_DATA_PATH']).mkdir(mode=0o700, parents=True, exist_ok=True)
        os.execvpe(command[0], command, environment)
    except (Unsupported, OSError) as error:
        message = str(error)
        print('Portal: ' + message, file=sys.stderr)
        if args.gui and shutil.which('kdialog'):
            subprocess.run(['kdialog', '--title', 'Open application', '--error', message])
        return 126


if __name__ == '__main__':
    sys.exit(main())
