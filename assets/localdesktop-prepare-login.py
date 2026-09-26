#!/usr/bin/python3
"""Prepare Portal's persistent Debian login without running a desktop as root.

This runs only from Portal's privileged provisioning/pre-launch boundary.  A
completed root-era installation is imported once, without removing /root or
overwriting an independently populated desktop home.
"""

import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path


USER = "desktop"
UID = GID = 1000
HOME = Path("/home/desktop")
ROOT_HOME = Path("/root")
PASSWD = Path("/etc/passwd")
GROUP = Path("/etc/group")
RUNTIME = Path("/run/user/1000")
SESSION = Path("/var/lib/localdesktop/session")
MARKER = Path("/var/lib/localdesktop/desktop-login-v1")
NSS_MARKER = Path("/var/lib/localdesktop/desktop-nss-v1")
STAGING = Path("/home/.portal-desktop-import-v1")
STAGING_SENTINEL = STAGING / ".portal-owned-staging"


def require_directory(path):
    if path.is_symlink() or (path.exists() and not path.is_dir()):
        raise RuntimeError(f"unsafe login directory: {path}")
    path.mkdir(parents=True, exist_ok=True)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def own_tree(path):
    # Used on staged copies and once on the target home before its marker.
    if path.is_dir() and not path.is_symlink():
        for child in path.iterdir():
            own_tree(child)
    os.lchown(path, UID, GID)


def clear_runtime_directory():
    """Recreate logind's per-login cleanup for this persistent PRoot rootfs."""
    for entry in RUNTIME.iterdir():
        mode = entry.lstat().st_mode
        if stat.S_ISDIR(mode):
            # rmtree does not follow symlink entries; symlinks are handled by
            # unlink below. /run/user is session IPC, never user profile data.
            shutil.rmtree(entry)
        else:
            entry.unlink()
    sync_directory(RUNTIME)


def copy_entry(source, destination):
    mode = source.lstat().st_mode
    if stat.S_ISDIR(mode):
        destination.mkdir()
        for child in source.iterdir():
            if child.name.startswith(".proot-meta-file."):
                continue
            copy_entry(child, destination / child.name)
        shutil.copystat(source, destination, follow_symlinks=False)
    elif stat.S_ISREG(mode):
        shutil.copy2(source, destination, follow_symlinks=False)
    elif stat.S_ISLNK(mode):
        destination.symlink_to(os.readlink(source))
    else:
        # Sockets/FIFOs/devices are session state, not persistent profile data.
        return False
    own_tree(destination)
    return True


def import_legacy_home():
    if MARKER.is_file():
        return
    old_home = ROOT_HOME
    if old_home.is_symlink() or not old_home.is_dir():
        raise RuntimeError("legacy root home is not a real directory")
    require_directory(STAGING)
    if not STAGING_SENTINEL.exists():
        if any(STAGING.iterdir()):
            raise RuntimeError("unrecognized legacy import staging contents")
        STAGING_SENTINEL.write_text("Portal desktop import v1\n")
    conflicts = []
    imported = 0
    def merge(source, destination, relative):
        nonlocal imported
        if destination.is_dir() and not destination.is_symlink() and source.is_dir() and not source.is_symlink():
            for child in source.iterdir():
                if child.name.startswith(".proot-meta-file."):
                    continue
                merge(child, destination / child.name, relative / child.name)
            return
        if destination.exists() or destination.is_symlink():
            conflicts.append(relative.as_posix())
            return
        # A single flat staging slot avoids following any path inherited
        # from a partially completed import.
        staged = STAGING / "entry"
        if staged.exists() or staged.is_symlink():
            # An interrupted copy is untrusted. Remove only this Portal-owned
            # staging entry, never the old or destination profile.
            if staged.is_dir() and not staged.is_symlink():
                shutil.rmtree(staged)
            else:
                staged.unlink()
        if not copy_entry(source, staged):
            return
        os.rename(staged, destination)
        sync_directory(destination.parent)
        imported += 1

    for source in old_home.iterdir():
        if source.name in ("Android", ".cache") or source.name.startswith(".proot-meta-file."):
            continue
        merge(source, HOME / source.name, Path(source.name))
    if conflicts:
        print("legacy home paths retained under /root because desktop already has them: "
              + ", ".join(conflicts), file=sys.stderr)
    # The source remains untouched. The marker is written after validation.
    return imported, conflicts


def write_marker(imported, conflicts):
    # Once committed, no later cold start may re-import old root settings.
    tmp = MARKER.with_name(MARKER.name + ".tmp")
    with tmp.open("w") as stream:
        stream.write(f"imported={imported}\nconflicts={','.join(conflicts)}\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(tmp, MARKER)
    sync_directory(MARKER.parent)


def prepare_account_database():
    if NSS_MARKER.is_file():
        return
    for path in (PASSWD, GROUP):
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"unsafe Debian account database: {path}")
        # The published image accidentally gave these public NSS databases
        # root-only permissions. D-Bus and desktop applications need to look
        # up UID 1000; /etc/shadow remains private and is never touched.
        os.chmod(path, 0o644)
    for database, expected in (("passwd", "desktop:x:1000:1000:"),
                               ("group", "desktop:x:1000:")):
        result = subprocess.run(
            ["/usr/sbin/runuser", "-u", USER, "--", "/usr/bin/getent", database, USER],
            text=True,
            capture_output=True,
            check=True,
        )
        if not result.stdout.startswith(expected):
            raise RuntimeError(f"desktop cannot resolve its Debian {database} record")
    temporary = NSS_MARKER.with_name(NSS_MARKER.name + ".tmp")
    with temporary.open("w") as stream:
        stream.write("desktop account lookup validated\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, NSS_MARKER)
    sync_directory(NSS_MARKER.parent)


def validate_desktop_ownership():
    # PRoot's --root-id deliberately reports app-owned files as UID 0 to
    # privileged setup. Query from the intended login instead: that is the
    # identity Plasma and its applications will actually use.
    paths = (HOME, RUNTIME, SESSION)
    result = subprocess.run(
        ["/usr/sbin/runuser", "-u", USER, "--", "/usr/bin/stat", "-c", "%u:%g:%a",
         *(str(path) for path in paths)],
        text=True,
        capture_output=True,
        check=True,
    )
    values = result.stdout.splitlines()
    if len(values) != len(paths):
        raise RuntimeError(f"desktop login ownership probe returned {len(values)} paths")
    for path, value in zip(paths, values):
        parts = value.split(":")
        if len(parts) != 3 or parts[:2] != ["1000", "1000"]:
            raise RuntimeError(f"desktop login ownership validation failed: {path} ({value})")
        mode = int(parts[2], 8)
        if path == HOME:
            if mode & 0o300 != 0o300:
                raise RuntimeError(f"desktop home is not writable: {path} ({value})")
        elif mode != 0o700:
            raise RuntimeError(f"desktop runtime directory is not private: {path} ({value})")


def main():
    import pwd
    import grp
    if len(sys.argv) != 2 or sys.argv[1] not in ("--prepare", "--migrate"):
        raise RuntimeError("expected --prepare or --migrate")
    if os.geteuid() != 0:
        raise RuntimeError("desktop login preparation requires guest root")
    account = pwd.getpwnam(USER)
    group = grp.getgrnam(USER)
    if (account.pw_uid, account.pw_gid, account.pw_dir, group.gr_gid) != (
        UID, GID, str(HOME), GID
    ):
        raise RuntimeError("desktop account identity does not match the shipped Debian image")
    home_prepared = MARKER.is_file()
    if home_prepared:
        if HOME.is_symlink() or not HOME.is_dir():
            raise RuntimeError("previously prepared desktop home is missing or unsafe")
    else:
        require_directory(HOME)
    require_directory(RUNTIME)
    # Unlike a normal Debian host, Portal's rootfs persists between logins.
    # Keep /run/user/1000 private and empty at the start of each Plasma login
    # so stale Wayland, D-Bus, KIO, and lock sockets cannot poison a retry.
    clear_runtime_directory()
    require_directory(SESSION)
    for path in (RUNTIME, SESSION):
        os.chown(path, UID, GID)
        os.chmod(path, 0o700)
    prepare_account_database()
    result = None
    if not home_prepared:
        os.chown(HOME, UID, GID)
        os.chmod(HOME, 0o700)
        own_tree(HOME)
    if sys.argv[1] == "--prepare" and not home_prepared:
        # First-run setup wrote its initial defaults as privileged files.
        # This mode is never used on an already completed user-owned desktop.
        result = (0, [])
    elif sys.argv[1] == "--migrate" and not home_prepared:
        # The image may already contain a desktop skeleton; make that whole
        # login tree writable before merging only missing legacy entries.
        result = import_legacy_home()
    validate_desktop_ownership()
    if result is not None:
        write_marker(*result)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Portal desktop login preparation failed: {error}", file=sys.stderr)
        sys.exit(1)
