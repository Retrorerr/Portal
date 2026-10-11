"""Wireless adb without the port juggling.

  python scripts/adb_wifi.py pair     show a QR code; scan it on the phone under
                                      Developer options > Wireless debugging >
                                      Pair device with QR code
  python scripts/adb_wifi.py connect  find the phone's current debug port over
                                      mDNS and connect (the port changes every
                                      time wireless debugging restarts)
  python scripts/adb_wifi.py watch    keep reconnecting whenever it drops

Pairing is remembered, so `pair` is needed once per PC; afterwards `connect`
(or `watch`) is enough even after the phone reboots or changes port.
"""

import re
import secrets
import subprocess
import sys
import time

PAIRING = "_adb-tls-pairing._tcp"
CONNECT = "_adb-tls-connect._tcp"


def adb(*args, timeout=15):
    try:
        out = subprocess.run(
            ["adb", *args], capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return ""
    return (out.stdout + out.stderr).strip()


def mdns(service):
    """[(instance, ip:port)] advertised for `service`."""
    found = []
    for line in adb("mdns", "services").splitlines():
        if service in line:
            parts = line.split()
            addr = next((p for p in parts if re.fullmatch(r"[\d.]+:\d+", p)), None)
            if addr:
                found.append((parts[0], addr))
    return found


def wireless_devices():
    """{serial: state} for every network adb device, online or not."""
    devices = {}
    for line in adb("devices").splitlines()[1:]:
        parts = line.split()
        if len(parts) >= 2 and (":" in parts[0] or "._adb-tls" in parts[0]):
            devices[parts[0]] = parts[1]
    return devices


def drop_stale():
    for serial, state in wireless_devices().items():
        if state != "device":
            adb("disconnect", serial)


def connect_once(quiet=False):
    drop_stale()
    online = [s for s, st in wireless_devices().items() if st == "device"]
    if online:
        if not quiet:
            print("already connected:", ", ".join(online))
        return online
    for _, addr in mdns(CONNECT):
        result = adb("connect", addr)
        print(result)
    online = [s for s, st in wireless_devices().items() if st == "device"]
    if not online and not quiet:
        print(
            "no phone found. Wake it, check it is on this Wi-Fi, and that "
            "Wireless debugging is on (run `pair` first if this PC is new to it)."
        )
    return online


def pair():
    import qrcode

    name = "portal-" + secrets.token_hex(3)
    password = secrets.token_hex(4)
    qr = qrcode.QRCode(border=2)
    qr.add_data(f"WIFI:T:ADB;S:{name};P:{password};;")
    qr.print_ascii(invert=True)
    print("Phone: Developer options > Wireless debugging > Pair device with QR code")
    print("Waiting for the phone (2 min)...")
    deadline = time.time() + 120
    while time.time() < deadline:
        match = [addr for inst, addr in mdns(PAIRING) if inst == name]
        if match:
            print(adb("pair", match[0], password, timeout=30))
            break
        time.sleep(1)
    else:
        sys.exit("timed out: the phone never advertised the pairing service")
    # The phone advertises its connect port a moment after pairing.
    for _ in range(15):
        if connect_once(quiet=True):
            break
        time.sleep(1)
    print(adb("devices", "-l"))


def watch():
    print("watching; Ctrl+C to stop")
    was_online = None
    while True:
        online = bool(connect_once(quiet=True))
        if online != was_online:
            print(time.strftime("%H:%M:%S"), "connected" if online else "lost, retrying")
            was_online = online
        time.sleep(5)


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else "connect"
    {"pair": pair, "connect": connect_once, "watch": watch}.get(
        command, lambda: sys.exit(__doc__)
    )()
