#!/usr/bin/env python3
"""Measure Portal's on-screen frame pacing from SurfaceFlinger over adb.

Clears SurfaceFlinger's per-layer latency history for Portal's SurfaceView,
performs a scripted drag on the connected device, then reports how many
presented frames landed on consecutive vsyncs and the average
queue->present latency. The vsync period comes from SurfaceFlinger itself,
so the numbers are correct at 60/120/144 Hz.

Note: OxygenOS does not treat `adb shell input` as a real touch for its
refresh-rate policy; check touch-policy behaviour with a real finger.
"""
import argparse
import collections
import re
import subprocess

LAYER_PATTERN = r"[0-9a-f]+ SurfaceView\[{pkg}/app\.polarbear\.PortalActivity\]\(BLAST\)#[0-9]+"


def adb_shell(serial, command):
    base = ["adb"] + (["-s", serial] if serial else [])
    return subprocess.run(base + ["shell", command], check=True, capture_output=True, text=True).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", help="adb device serial")
    parser.add_argument("--package", default="app.polarbear", help="Portal package (Debug: app.polarbear)")
    parser.add_argument("--drag", default="1700 1000 2200 1300", help="x0 y0 x1 y1 in screen pixels")
    parser.add_argument("--ms", type=int, default=1500, help="drag duration")
    args = parser.parse_args()

    listing = adb_shell(args.serial, "dumpsys SurfaceFlinger --list")
    match = re.search(LAYER_PATTERN.format(pkg=re.escape(args.package)), listing)
    if not match:
        raise SystemExit(f"No Portal SurfaceView layer for {args.package}; is Portal in the foreground?")
    layer = match.group(0)
    adb_shell(args.serial, f"dumpsys SurfaceFlinger --latency-clear '{layer}'")
    adb_shell(args.serial, f"input touchscreen draganddrop {args.drag} {args.ms}")
    lines = adb_shell(args.serial, f"dumpsys SurfaceFlinger --latency '{layer}'").split("\n")

    period_ns = int(lines[0].strip())
    rows = []
    for line in lines[1:]:
        fields = line.split()
        if len(fields) == 3 and 0 < int(fields[1]) < 9e18:
            rows.append(tuple(map(int, fields)))
    if len(rows) < 2:
        raise SystemExit("Too few presented frames; did the drag hit the desktop?")
    presents = [row[1] for row in rows]
    intervals = collections.Counter(
        round((b - a) / period_ns) for a, b in zip(presents, presents[1:])
    )
    latency_ms = sum((row[1] - row[0]) / 1e6 for row in rows) / len(rows)
    print(f"refresh {1e9 / period_ns:.1f} Hz, frames {len(rows)}")
    print("present intervals (vsyncs: count):", dict(sorted(intervals.items())))
    print(f"on consecutive vsyncs: {intervals.get(1, 0)}/{len(rows) - 1}")
    print(f"queue->present avg: {latency_ms:.1f} ms")


if __name__ == "__main__":
    main()
