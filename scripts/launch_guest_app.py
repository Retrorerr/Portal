import subprocess
import sys
import os
import tempfile

def get_device():
    if os.environ.get("ANDROID_SERIAL"):
        return os.environ["ANDROID_SERIAL"]
    try:
        out = subprocess.check_output(["adb", "devices"], text=True)
        for line in out.strip().splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 2 and parts[1] == "device":
                return parts[0]
    except Exception:
        pass
    raise RuntimeError("No active authorized ADB device found")

def main():
    if len(sys.argv) < 2:
        print("Usage: launch_guest_app.py <command>")
        sys.exit(1)

    cmd = " ".join(sys.argv[1:])
    device_id = get_device()

    pm_path = subprocess.check_output([
        "adb", "-s", device_id, "shell", "pm path app.polarbear"
    ], text=True).strip().replace("package:", "")

    lib_dir = pm_path.rsplit("/", 1)[0] + "/lib/arm64"
    proot_loader = lib_dir + "/libproot_loader.so"
    libproot = lib_dir + "/libproot.so"

    pid_str = subprocess.check_output([
        "adb", "-s", device_id, "shell", "pidof plasmashell"
    ], text=True).strip().split()
    if not pid_str:
        print("plasmashell not running")
        sys.exit(1)
    plasma_pid = pid_str[0]

    env_raw = subprocess.check_output([
        "adb", "-s", device_id, "exec-out",
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
        f"nohup {cmd} >/tmp/app_launch.log 2>&1 &\n"
        "sleep 1\n"
    )

    with tempfile.NamedTemporaryFile(mode="w", delete=False, newline="\n") as f:
        f.write(guest_script)
        local_tmp = f.name

    try:
        subprocess.check_call(["adb", "-s", device_id, "push", local_tmp, "/data/local/tmp/app_launch.sh"])
    finally:
        os.unlink(local_tmp)

    subprocess.check_call([
        "adb", "-s", device_id, "shell",
        "run-as app.polarbear cp /data/local/tmp/app_launch.sh /data/data/app.polarbear/files/runtime-B/tmp/app_launch.sh"
    ])

    remote_cmd = (
        f"export PROOT_LOADER={proot_loader}; "
        "export PROOT_TMP_DIR=/data/data/app.polarbear/files/tmp; "
        f"nohup {libproot} -r /data/data/app.polarbear/files/runtime-B "
        "-L --link2symlink --sysvipc --root-id "
        "-b /dev -b /proc -b /sys "
        "-b /dev/urandom:/dev/random "
        "-b /data/data/app.polarbear/files/runtime-B/tmp:/dev/shm "
        "-b /data/local/tmp:/data/local/tmp "
        "-w /root /bin/sh /tmp/app_launch.sh >/dev/null 2>&1 &"
    )

    subprocess.check_call([
        "adb", "-s", device_id, "shell",
        f"run-as app.polarbear sh -c '{remote_cmd}'"
    ])
    print(f"Launched '{cmd}' in background")

if __name__ == "__main__":
    main()
