#!/usr/bin/env python3
"""Give each Android screen size its own Plasma scale.

Portal shows one KWin output whatever Android screen it runs on, and KScreen
keeps one scale per output. A foldable's cover screen (904 px across on a
Galaxy Z Fold5) therefore inherited the inner screen's scale and left Plasma
402 logical pixels wide: the panel clock clipped and windows spilled off.

Portal rewrites /tmp/localdesktop-output whenever its window changes size.
This helper remembers the scale in use for every screen size (keyed by the
shorter and longer side, so rotating changes nothing) and, when the size
changes, restores that size's scale. A size seen for the first time keeps
the user's interface-size choice relative to the device baseline that setup
uses (src/core/install_plan.rs): Android's density, held back so the shorter
side keeps at least 640 logical pixels.
"""

import ctypes
import json
import math
import os
import select
import subprocess
import sys
import time
from pathlib import Path

OUTPUT_STATE = Path("/tmp/localdesktop-output")
STATE_PATH = Path.home() / ".local/state/portal/output-scales.json"
# Keep in step with MIN_LOGICAL_SHORT_SIDE_PX in src/core/install_plan.rs.
MIN_LOGICAL_SHORT_SIDE_PX = 640
MAX_REMEMBERED_SIZES = 16
# Setup's interface sizes span Compact (0.85x the baseline) to Large (1.15x).
# A scale that implies more than this was chosen for a different screen.
MULTIPLIER_RANGE = (0.8, 1.2)
# Android resizes in bursts (rotation, fold, freeform drags): act once the
# size has held this long.
SETTLE_SECONDS = 1.5

IN_CLOSE_WRITE = 0x00000008
IN_MOVED_TO = 0x00000080


def log(message):
    print(f"localdesktop-output-scale: {message}", file=sys.stderr, flush=True)


def read_output_state():
    """Return (short, long, density) from Portal's output file, or None."""
    values = {}
    try:
        for line in OUTPUT_STATE.read_text(encoding="utf-8").splitlines():
            key, _, value = line.partition("=")
            values[key.strip()] = value.strip()
    except OSError:
        return None
    try:
        width, height = (int(part) for part in values["LOCALDESKTOP_OUTPUT_MODE"].split("x"))
        density = int(values["LOCALDESKTOP_OUTPUT_DENSITY"])
    except (KeyError, ValueError):
        return None
    if width <= 0 or height <= 0 or density <= 0:
        return None
    return min(width, height), max(width, height), density


def baseline_scale(short_side, density):
    cap = max(160, short_side * 160 // MIN_LOGICAL_SHORT_SIDE_PX)
    return min(density, cap) / 160.0


def quantized(scale):
    # KWin keeps output scales in 1/120 steps.
    return math.floor(scale * 120.0 + 0.5) / 120.0


def seeded_scale(short_side, density, multiplier):
    low, high = MULTIPLIER_RANGE
    multiplier = min(high, max(low, multiplier))
    return quantized(min(5.0, max(1.0, baseline_scale(short_side, density) * multiplier)))


def scale_text(scale):
    text = f"{scale:.5f}".rstrip("0")
    return text.rstrip(".")


def live_output():
    """Return (id, scale) of the single enabled output."""
    result = subprocess.run(
        ["kscreen-doctor", "--json"], capture_output=True, text=True, timeout=20, check=True
    )
    outputs = json.loads(result.stdout).get("outputs", [])
    active = [
        item
        for item in outputs
        if isinstance(item, dict)
        and item.get("connected") is True
        and item.get("enabled", True) is not False
    ]
    if len(active) != 1:
        raise RuntimeError(f"expected one enabled output, found {len(active)}")
    return int(active[0]["id"]), float(active[0]["scale"])


def set_scale(output_id, scale):
    subprocess.run(
        ["kscreen-doctor", f"output.{output_id}.scale.{scale_text(scale)}"],
        capture_output=True,
        timeout=20,
        check=True,
    )


def load_state():
    try:
        state = json.loads(STATE_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {"current": None, "scales": {}}
    if not isinstance(state, dict) or not isinstance(state.get("scales"), dict):
        return {"current": None, "scales": {}}
    return state


def save_state(state):
    scales = state["scales"]
    # Dicts keep insertion order: drop the sizes seen longest ago.
    while len(scales) > MAX_REMEMBERED_SIZES:
        scales.pop(next(iter(scales)))
    STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    temporary = STATE_PATH.with_suffix(".tmp")
    temporary.write_text(json.dumps(state, indent=1), encoding="utf-8")
    os.replace(temporary, STATE_PATH)


def size_key(short_side, long_side):
    return f"{short_side}x{long_side}"


def apply_size(observed):
    short_side, long_side, density = observed
    key = size_key(short_side, long_side)
    state = load_state()
    previous = state.get("current")
    if previous == key:
        return
    output_id, live_scale = live_output()
    scales = state["scales"]
    if previous is None:
        # First run: adopt the live scale for this size, unless it plainly
        # belongs to another screen (setup ran unfolded, Portal now runs on
        # the cover screen).
        target = live_scale
        multiplier = live_scale / baseline_scale(short_side, density)
        if not MULTIPLIER_RANGE[0] <= multiplier <= MULTIPLIER_RANGE[1]:
            target = seeded_scale(short_side, density, multiplier)
            set_scale(output_id, target)
        scales[key] = target
        state["current"] = key
        save_state(state)
        log(f"tracking size={key} scale={scale_text(live_scale)}->{scale_text(target)}")
        return

    # The live scale belongs to the size Portal just left, including any
    # change the user made in System Settings while on it.
    scales.pop(previous, None)
    scales[previous] = live_scale
    target = scales.get(key)
    if target is None:
        try:
            old_short = int(previous.split("x")[0])
        except ValueError:
            old_short = short_side
        multiplier = live_scale / baseline_scale(old_short, density)
        target = seeded_scale(short_side, density, multiplier)
    scales.pop(key, None)
    scales[key] = target
    state["current"] = key
    if abs(target - live_scale) > 0.001:
        set_scale(output_id, target)
        log(f"size={previous}->{key} scale={scale_text(live_scale)}->{scale_text(target)}")
    save_state(state)


def watch_fd():
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.inotify_init1(os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1")
    mask = IN_CLOSE_WRITE | IN_MOVED_TO
    if libc.inotify_add_watch(fd, str(OUTPUT_STATE.parent).encode(), mask) < 0:
        raise OSError(ctypes.get_errno(), "inotify_add_watch")
    return fd


def main():
    fd = watch_fd()
    pending = True
    retries = 0
    while True:
        timeout = SETTLE_SECONDS if pending else None
        ready, _, _ = select.select([fd], [], [], timeout)
        if ready:
            events = os.read(fd, 4096)
            if OUTPUT_STATE.name.encode() in events:
                pending = True
                retries = 0
            continue
        if not pending:
            continue
        pending = False
        observed = read_output_state()
        if observed is None:
            continue
        try:
            apply_size(observed)
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
            log(f"could not apply the scale for {size_key(observed[0], observed[1])}: {error}")
            # KWin may still be starting or restarting: try again shortly,
            # then wait for the next size change.
            if retries < 6:
                retries += 1
                time.sleep(5)
                pending = True


if __name__ == "__main__":
    main()
