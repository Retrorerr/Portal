#!/usr/bin/env python3
"""Physical acceptance checks without guest configuration repairs or package changes."""
import argparse
import json
import os
import re
import shlex
import subprocess

PACKAGE = "app.polarbear"
FILES = f"/data/data/{PACKAGE}/files"

def device(serial=None):
    authorized = [line.split()[0] for line in subprocess.check_output(
        ["adb", "devices"], text=True).splitlines()[1:] if len(line.split()) == 2 and line.split()[1] == "device"]
    serial = serial or os.environ.get("ANDROID_SERIAL")
    if serial:
        if serial not in authorized:
            raise RuntimeError(f"ADB device {serial} is not connected and authorized")
        return serial
    if len(authorized) != 1:
        raise RuntimeError(f"Expected one authorized ADB device; found {len(authorized)}. Supply --serial.")
    return authorized[0]

def shell(serial, args):
    return subprocess.check_output(["adb", "-s", serial, "shell", shlex.join(args)], text=True)

def guest(serial, command):
    package = shell(serial, ["dumpsys", "package", PACKAGE])
    lib = re.search(r"codePath=(\S+)", package).group(1) + "/lib/arm64"
    root = FILES + "/runtime-B"
    args = [f"{lib}/libproot.so", "-r", root, "-w", "/", "-0", "--link2symlink", "--sysvipc",
        "-b", "/dev", "-b", "/proc", "-b", "/sys", "-b", f"{root}/tmp:/dev/shm",
        "/usr/bin/env", "-i", "PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/root", "LANG=C.UTF-8",
        "LD_PRELOAD=/usr/local/lib/localdesktop-crash-handler.so",
        "/bin/sh", "-c", command]
    invocation = ("export PROOT_LOADER=" + shlex.quote(lib + "/libproot_loader.so") +
        "; export PROOT_TMP_DIR=" + shlex.quote(FILES) + "; exec " + shlex.join(args))
    return shell(serial, ["run-as", PACKAGE, "sh", "-c", invocation])

def verify(serial):
    state = shell(serial, ["run-as", PACKAGE, "sh", "-c",
        f"test ! -e {FILES}/arch && test ! -e {FILES}/runtime-B.staging && cat {FILES}/runtime-B/.portal-runtime-complete"])
    print("Runtime completion:\n" + state)
    print(guest(serial, "set -e; . /etc/os-release; test \"$ID\" = debian; test \"$VERSION_ID\" = 13; "
        "cat /etc/os-release; dpkg --version | head -1; "
        "dpkg-query -W plasma-desktop kwin-wayland dolphin konsole systemsettings firefox-esr ark kate gwenview okular kcalc; "
        "! command -v pacman; echo 'pacman absent'; test -x /usr/local/bin/portal-ime-bridge; "
        "test -s /usr/local/lib/localdesktop-crash-handler.so; cat /var/lib/localdesktop/plasma-ready"))
    processes = shell(serial, ["ps", "-A", "-o", "PID,PPID,NAME"])
    for name in ["kwin_wayland", "plasmashell", "ksmserver"]:
        matches = [line for line in processes.splitlines() if line.split()[-1] == name]
        if not matches:
            raise RuntimeError(f"Missing desktop process: {name}")
        print("\n".join(matches))

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial")
    args = parser.parse_args()
    verify(device(args.serial))
