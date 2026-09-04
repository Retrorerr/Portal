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
        "adb", "-s", "f105b146", "exec-out",
        f"run-as app.polarbear cat /proc/{plasma_pid}/environ"
    ])

    dbus_addr = ""
    for item in env_raw.split(b"\x00"):
        if item.startswith(b"DBUS_SESSION_BUS_ADDRESS="):
            dbus_addr = item.split(b"=", 1)[1].decode("utf-8", "ignore").strip()
            break

    guest_script = (
        "#!/bin/sh\n"
        "export HOME=/root\n"
        "export TMPDIR=/tmp\n"
        "export USER=root\n"
        "export LOGNAME=root\n"
        "export LANG=C.UTF-8\n"
        "export LC_ALL=C.UTF-8\n"
        "export SHELL=/bin/bash\n"
        "export PATH=/usr/local/sbin:/usr/local/bin:/usr/lib/aarch64-linux-gnu/libexec:/usr/bin:/bin:/usr/sbin:/sbin\n"
        "export LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu:/lib/aarch64-linux-gnu\n"
        "export XDG_RUNTIME_DIR=/tmp\n"
        "export WAYLAND_DISPLAY=wayland-1\n"
        "export DISPLAY=:0\n"
        f"export DBUS_SESSION_BUS_ADDRESS='{dbus_addr}'\n"
        f"{cmd}\n"
    )

    import tempfile, os
    with tempfile.NamedTemporaryFile(mode="w", delete=False, newline="\n") as f:
        f.write(guest_script)
        local_tmp = f.name

    try:
        subprocess.check_call(["adb", "-s", "f105b146", "push", local_tmp, "/data/local/tmp/guest_exec.sh"])
    finally:
        os.unlink(local_tmp)

    # Copy into guest /tmp
    subprocess.check_call([
        "adb", "-s", "f105b146", "shell",
        "run-as app.polarbear cp /data/local/tmp/guest_exec.sh /data/data/app.polarbear/files/runtime-B/tmp/exec.sh"
    ])

    remote_cmd = (
        f"export PROOT_LOADER={proot_loader}; "
        "export PROOT_TMP_DIR=/data/data/app.polarbear/files/tmp; "
        f"{libproot} -r /data/data/app.polarbear/files/runtime-B "
        "-L --link2symlink --sysvipc --kill-on-exit --root-id "
        "-b /dev -b /proc -b /sys "
        "-b /dev/urandom:/dev/random "
        "-b /proc/self/fd:/dev/fd "
        "-b /proc/self/fd/0:/dev/stdin "
        "-b /proc/self/fd/1:/dev/stdout "
        "-b /proc/self/fd/2:/dev/stderr "
        "-b /data/data/app.polarbear/files/runtime-B/tmp:/dev/shm "
        "-w /root /bin/sh /tmp/exec.sh"
    )

    res = subprocess.run([
        "adb", "-s", "f105b146", "shell",
        f"run-as app.polarbear sh -c '{remote_cmd}'"
    ], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if res.stdout:
        print("STDOUT:", res.stdout)
    if res.stderr:
        print("STDERR:", res.stderr)

if __name__ == "__main__":
    main()
