import subprocess
import sys

def main():
    if len(sys.argv) < 2:
        print("Usage: guest_exec.py <command>")
        sys.exit(1)

    cmd = " ".join(sys.argv[1:])

    pm_path = subprocess.check_output([
        "adb", "-s", "f105b146", "shell", "pm path app.polarbear"
    ], text=True).strip().replace("package:", "")

    lib_dir = pm_path.rsplit("/", 1)[0] + "/lib/arm64"
    proot_loader = lib_dir + "/libproot_loader.so"
    libproot = lib_dir + "/libproot.so"

    pid_str = subprocess.check_output([
        "adb", "-s", "f105b146", "shell", "pidof plasmashell"
    ], text=True).strip().split()
    if not pid_str:
        print("plasmashell not running")
        sys.exit(1)
    plasma_pid = pid_str[0]

    env_raw = subprocess.check_output([
        "adb", "-s", "f105b146", "shell",
        f"run-as app.polarbear cat /proc/{plasma_pid}/environ"
    ], encoding="latin-1")

    dbus_addr = ""
    for item in env_raw.split("\x00"):
        if item.startswith("DBUS_SESSION_BUS_ADDRESS="):
            dbus_addr = item.split("=", 1)[1]
            break

    guest_script = f"export HOME=/root; export TMPDIR=/tmp; export USER=root; export LOGNAME=root; export LANG=C.UTF-8; export LC_ALL=C.UTF-8; export PATH=/usr/local/sbin:/usr/local/bin:/usr/lib/aarch64-linux-gnu/libexec:/usr/bin:/bin:/usr/sbin:/sbin; export LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu; export XDG_RUNTIME_DIR=/tmp; export WAYLAND_DISPLAY=wayland-1; export DISPLAY=:0; export DBUS_SESSION_BUS_ADDRESS='{dbus_addr}'; {cmd}"

    remote_cmd = f"export PROOT_LOADER={proot_loader}; export PROOT_TMP_DIR=/data/data/app.polarbear/files/tmp; {libproot} -k 6.6.0 -0 --link2symlink --sysvipc -r /data/data/app.polarbear/files/runtime-B -b /dev -b /proc -b /sys -b /data/data/app.polarbear/files/runtime-B/tmp:/dev/shm -w /root /usr/bin/dash -c \"{guest_script}\""

    res = subprocess.run([
        "adb", "-s", "f105b146", "shell",
        f"run-as app.polarbear sh -c '{remote_cmd}'"
    ], capture_output=True, text=True)
    if res.stdout:
        print("STDOUT:", res.stdout)
    if res.stderr:
        print("STDERR:", res.stderr)

if __name__ == "__main__":
    main()
