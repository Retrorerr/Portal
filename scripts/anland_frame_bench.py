#!/usr/bin/env python3
"""Measure Portal's on-screen frame pacing, latency, GPU completion, and power over adb.

Uses SurfaceFlinger's per-layer latency history (`dumpsys SurfaceFlinger --latency`):
  col0 = desiredPresentTime (ANativeWindow queueBuffer CLOCK_MONOTONIC ns; may be 0 or pending)
  col1 = actualPresentTime  (HWC present fence signal CLOCK_MONOTONIC ns)
  col2 = frameReadyTime     (GPU acquire fence signal CLOCK_MONOTONIC ns)

Polls `--latency` continuously during the run so SurfaceFlinger's 128-frame ring
buffer does not drop early frames at 120/144 Hz, tracks the active refresh period
while frames are landing (avoiding post-idle 50 Hz drop corruption), validates
each timestamp column before computing latencies, and samples `dumpsys battery`
for power consumption.

Note: OxygenOS does not treat `adb shell input` as a real touch for all
refresh-rate policies; verify touch-policy behaviour with a real finger too.
"""
import argparse
import collections
import json
import math
import re
import subprocess
import sys
import threading
import time

LAYER_PATTERN = r"[0-9a-f]+ SurfaceView\[{pkg}/app\.polarbear\.PortalActivity\]\(BLAST\)#[0-9]+"

# SurfaceFlinger uses INT64_MAX (9223372036854775807) for pending/unsignaled fences.
PENDING_TIMESTAMP_NS = 9_000_000_000_000_000_000
# Any single-frame queue->present or queue->ready delta above 10 s is treated as an
# invalid/uninitialized timestamp rather than a real frame latency.
MAX_SANE_DELTA_NS = 10_000_000_000
# Allow a small negative ready->present tolerance (e.g., clock/fence jitter) before
# flagging a timestamp as invalid.
MIN_SANE_READY_TO_PRESENT_NS = -50_000_000


def adb_shell(serial, command):
    base = ["adb"] + (["-s", serial] if serial else [])
    return subprocess.run(
        base + ["shell", command], check=True, capture_output=True, text=True
    ).stdout


def is_valid_timestamp(ts_ns):
    return 0 < ts_ns < PENDING_TIMESTAMP_NS


def parse_latency_dump(text):
    """Parse `dumpsys SurfaceFlinger --latency <layer>` output.

    Returns `(period_ns, rows, diagnostics)` where each row is
    `(desired_ns, present_ns, ready_ns)` for frames with a valid `present_ns`.
    """
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        return 0, [], {"empty_rows": 0, "pending_present_rows": 0, "malformed_rows": 0}
    try:
        period_ns = int(lines[0])
    except ValueError:
        return 0, [], {"empty_rows": 0, "pending_present_rows": 0, "malformed_rows": 1}

    rows = []
    empty_rows = 0
    pending_present_rows = 0
    malformed_rows = 0
    for line in lines[1:]:
        fields = line.split()
        if len(fields) != 3:
            malformed_rows += 1
            continue
        try:
            desired_ns, present_ns, ready_ns = (int(fields[0]), int(fields[1]), int(fields[2]))
        except ValueError:
            malformed_rows += 1
            continue
        if desired_ns == 0 and present_ns == 0 and ready_ns == 0:
            empty_rows += 1
            continue
        if present_ns >= PENDING_TIMESTAMP_NS:
            pending_present_rows += 1
            continue
        if present_ns <= 0:
            empty_rows += 1
            continue
        rows.append((desired_ns, present_ns, ready_ns))

    return period_ns, rows, {
        "empty_rows": empty_rows,
        "pending_present_rows": pending_present_rows,
        "malformed_rows": malformed_rows,
    }


def merge_latency_rows(store, rows, min_present_ns=0):
    """Merge polled `(desired, present, ready)` rows into `store` keyed by `present_ns`.

    Returns the number of newly added presented frames.
    """
    added = 0
    for desired_ns, present_ns, ready_ns in rows:
        if present_ns <= min_present_ns:
            continue
        prev = store.get(present_ns)
        if prev is None:
            store[present_ns] = (desired_ns, present_ns, ready_ns)
            added += 1
        else:
            prev_d, _, prev_r = prev
            merged_d = prev_d if is_valid_timestamp(prev_d) else desired_ns
            merged_r = prev_r if is_valid_timestamp(prev_r) else ready_ns
            store[present_ns] = (merged_d, present_ns, merged_r)
    return added


def percentile(sorted_vals, pct):
    """Linear-interpolated percentile on a non-empty pre-sorted list."""
    if not sorted_vals:
        return None
    if len(sorted_vals) == 1:
        return float(sorted_vals[0])
    pos = (len(sorted_vals) - 1) * (pct / 100.0)
    lo = int(math.floor(pos))
    hi = min(lo + 1, len(sorted_vals) - 1)
    frac = pos - lo
    return float(sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * frac)


def summarize_ms(values_ms):
    if not values_ms:
        return None
    ordered = sorted(values_ms)
    return {
        "count": len(ordered),
        "min": round(ordered[0], 3),
        "avg": round(sum(ordered) / len(ordered), 3),
        "p50": round(percentile(ordered, 50), 3),
        "p95": round(percentile(ordered, 95), 3),
        "p99": round(percentile(ordered, 99), 3),
        "max": round(ordered[-1], 3),
    }


def choose_active_period_ns(observed_periods_ns, present_timestamps_ns):
    """Select the display vsync period active while frames were presented.

    On LTPO panels (e.g. 50/60/120/144 Hz), SurfaceFlinger's period header drops
    to 50 Hz immediately after input ends. Prefer the highest active refresh rate
    observed while frames were landing, falling back cleanly if only one period
    was observed.
    """
    candidates = sorted({p for p in observed_periods_ns if p and p > 0})
    if not candidates:
        return 0
    if len(candidates) == 1:
        return candidates[0]

    if len(present_timestamps_ns) < 2:
        return candidates[0]

    dts = sorted(
        b - a
        for a, b in zip(present_timestamps_ns, present_timestamps_ns[1:])
        if b > a
    )
    if not dts:
        return candidates[0]

    # Use the faster half of intervals (immune to occasional dropped frames or pauses)
    # to identify the active refresh cadence without being misled by post-idle 50 Hz.
    fast_dts = dts[: max(1, len(dts) // 2)]
    p25_dt = percentile(dts, 25)

    # First prefer candidates whose period is close to the fast interval floor.
    compatible = [p for p in candidates if p <= p25_dt * 1.25]
    pool = compatible if compatible else candidates

    def alignment_error(period):
        # Penalize periods that force sub-vsync rounding (< 0.75 * period) and
        # prefer the largest period that accurately explains the fast intervals.
        err = 0.0
        for dt in fast_dts:
            steps = max(1, round(dt / period))
            err += abs(dt - steps * period) / period
        return err / len(fast_dts)

    # Pick the candidate with minimum phase error; tie-break toward the faster
    # refresh rate only when actual intervals are faster than the slower candidate.
    best = min(pool, key=lambda p: (round(alignment_error(p), 3), p))
    return best


def analyze_frames(rows, active_period_ns, target_hz=None):
    """Compute interval, missed-refresh, presentation latency, and GPU completion stats."""
    ordered = sorted(rows, key=lambda r: r[1])
    presents = [r[1] for r in ordered]

    ref_period_ns = (
        round(1e9 / target_hz)
        if target_hz and target_hz > 0
        else (active_period_ns if active_period_ns > 0 else 0)
    )

    dt_ns = [b - a for a, b in zip(presents, presents[1:]) if b > a]
    dt_ms = [dt / 1e6 for dt in dt_ns]

    effective_fps = 0.0
    span_ms = 0.0
    if len(presents) >= 2 and presents[-1] > presents[0]:
        span_ns = presents[-1] - presents[0]
        span_ms = span_ns / 1e6
        effective_fps = (len(presents) - 1) * 1e9 / span_ns

    intervals_vsyncs = collections.Counter()
    consecutive_vsyncs = 0
    missed_refresh_frames = 0
    missed_refresh_slots = 0
    if ref_period_ns > 0 and dt_ns:
        for dt in dt_ns:
            steps = max(1, round(dt / ref_period_ns))
            intervals_vsyncs[steps] += 1
            if steps == 1:
                consecutive_vsyncs += 1
            else:
                missed_refresh_frames += 1
                missed_refresh_slots += steps - 1

    queue_to_present_ms = []
    queue_to_ready_ms = []
    ready_to_present_ms = []
    excluded_desired_rows = 0
    excluded_ready_rows = 0
    late_gpu_frames = 0

    for desired_ns, present_ns, ready_ns in ordered:
        desired_ok = (
            is_valid_timestamp(desired_ns)
            and 0 <= (present_ns - desired_ns) < MAX_SANE_DELTA_NS
        )
        if desired_ok:
            queue_to_present_ms.append((present_ns - desired_ns) / 1e6)
        else:
            excluded_desired_rows += 1

        ready_ok = is_valid_timestamp(ready_ns)
        if ready_ok:
            r2p_ns = present_ns - ready_ns
            if MIN_SANE_READY_TO_PRESENT_NS <= r2p_ns < MAX_SANE_DELTA_NS:
                ready_to_present_ms.append(r2p_ns / 1e6)
                if ready_ns > present_ns:
                    late_gpu_frames += 1
            else:
                excluded_ready_rows += 1
                ready_ok = False
        else:
            excluded_ready_rows += 1

        if desired_ok and ready_ok:
            q2r_ns = ready_ns - desired_ns
            if 0 <= q2r_ns < MAX_SANE_DELTA_NS:
                queue_to_ready_ms.append(q2r_ns / 1e6)

    total_intervals = len(dt_ns)
    return {
        "frames": len(ordered),
        "span_ms": round(span_ms, 2),
        "effective_fps": round(effective_fps, 2),
        "active_period_ns": active_period_ns,
        "active_refresh_hz": round(1e9 / active_period_ns, 2) if active_period_ns > 0 else 0.0,
        "ref_period_ns": ref_period_ns,
        "ref_refresh_hz": round(1e9 / ref_period_ns, 2) if ref_period_ns > 0 else 0.0,
        "frame_interval_ms": summarize_ms(dt_ms),
        "intervals_vsyncs": dict(sorted(intervals_vsyncs.items())),
        "consecutive_vsyncs": consecutive_vsyncs,
        "total_intervals": total_intervals,
        "missed_refresh_frames": missed_refresh_frames,
        "missed_refresh_slots": missed_refresh_slots,
        "missed_refresh_pct": round(100.0 * missed_refresh_frames / total_intervals, 2)
        if total_intervals
        else 0.0,
        "queue_to_present_ms": summarize_ms(queue_to_present_ms),
        "gpu_completion_ms": summarize_ms(queue_to_ready_ms),
        "ready_to_present_ms": summarize_ms(ready_to_present_ms),
        "excluded_desired_rows": excluded_desired_rows,
        "excluded_ready_rows": excluded_ready_rows,
        "late_gpu_frames": late_gpu_frames,
    }


def parse_display_mode(sf_dump):
    m = re.search(
        r"activeMode=\{[^}]*resolution=(\d+x\d+)[^}]*vsyncRate=([0-9.]+)\s*Hz",
        sf_dump,
    )
    if not m:
        return None
    return {
        "resolution": m.group(1),
        "vsync_hz": float(m.group(2)),
    }


def parse_battery(dump):
    """Parse `dumpsys battery` on Android / OPLUS devices."""
    out = {}
    patterns = {
        "current_ma": (r"^\s*(?:Battery current|current now)\s*:\s*(-?\d+)", int),
        "voltage_mv": (r"^\s*voltage\s*:\s*(\d+)", int),
        "charge_counter_uah": (r"^\s*Charge counter\s*:\s*(-?\d+)", int),
        "status": (r"^\s*status\s*:\s*(\d+)", int),
        "level": (r"^\s*level\s*:\s*(\d+)", int),
        "temperature_deci_c": (r"^\s*temperature\s*:\s*(-?\d+)", int),
        "ac_powered": (r"^\s*AC powered\s*:\s*(true|false)", lambda s: s == "true"),
        "usb_powered": (r"^\s*USB powered\s*:\s*(true|false)", lambda s: s == "true"),
    }
    for key, (pat, conv) in patterns.items():
        m = re.search(pat, dump, re.MULTILINE)
        if m:
            out[key] = conv(m.group(1))
    if "current_ma" in out and "voltage_mv" in out:
        # Android BatteryService reports current in uA on some devices if huge (> 50000);
        # OPLUS `Battery current` is in mA (~200-3500). Normalize if magnitude > 50000.
        cur = out["current_ma"]
        cur_ma = cur / 1000.0 if abs(cur) > 50000 else float(cur)
        out["current_ma_norm"] = round(cur_ma, 2)
        out["power_mw"] = round(abs(cur_ma) * out["voltage_mv"] / 1000.0, 2)
    return out


def summarize_power(samples):
    """Aggregate timestamped `(monotonic_s, battery_dict)` samples."""
    if not samples:
        return None
    valid_power = [s[1]["power_mw"] for s in samples if "power_mw" in s[1]]
    valid_curr = [s[1]["current_ma_norm"] for s in samples if "current_ma_norm" in s[1]]
    valid_volt = [s[1]["voltage_mv"] for s in samples if "voltage_mv" in s[1]]
    first_t, first_b = samples[0]
    last_t, last_b = samples[-1]
    elapsed_s = max(0.0, last_t - first_t)
    charge_delta_uah = None
    charge_power_mw = None
    if "charge_counter_uah" in first_b and "charge_counter_uah" in last_b:
        charge_delta_uah = first_b["charge_counter_uah"] - last_b["charge_counter_uah"]
        if elapsed_s > 0 and valid_volt and charge_delta_uah > 0:
            avg_v = sum(valid_volt) / len(valid_volt) / 1000.0
            # uAh * V * 3600 / elapsed_s -> uW -> / 1000 -> mW
            charge_power_mw = round((charge_delta_uah * avg_v * 3.6) / elapsed_s, 2)

    status_map = {2: "charging", 3: "discharging", 4: "not_charging", 5: "full"}
    status_code = last_b.get("status")
    return {
        "samples": len(samples),
        "elapsed_s": round(elapsed_s, 2),
        "battery_state": status_map.get(status_code, str(status_code)),
        "ac_powered": last_b.get("ac_powered"),
        "usb_powered": last_b.get("usb_powered"),
        "avg_current_ma": round(sum(valid_curr) / len(valid_curr), 2) if valid_curr else None,
        "avg_voltage_mv": round(sum(valid_volt) / len(valid_volt), 1) if valid_volt else None,
        "avg_power_mw": round(sum(valid_power) / len(valid_power), 2) if valid_power else None,
        "min_power_mw": round(min(valid_power), 2) if valid_power else None,
        "max_power_mw": round(max(valid_power), 2) if valid_power else None,
        "charge_delta_uah": charge_delta_uah,
        "charge_derived_power_mw": charge_power_mw,
        "temperature_c": round(last_b["temperature_deci_c"] / 10.0, 1)
        if "temperature_deci_c" in last_b
        else None,
    }


def parse_anland_logcat(text):
    """Extract anland.fps / anland.render diagnostic lines if present."""
    fps_lines = []
    render_lines = []
    for line in text.splitlines():
        if "anland.fps" in line:
            fps_lines.append(line.strip())
        elif "anland.render" in line:
            render_lines.append(line.strip())
    return {
        "fps_lines": fps_lines[-10:],
        "render_lines": render_lines[-10:],
    }


def find_layer(serial, package):
    listing = adb_shell(serial, "dumpsys SurfaceFlinger --list")
    match = re.search(LAYER_PATTERN.format(pkg=re.escape(package)), listing)
    if not match:
        raise SystemExit(
            f"No Portal SurfaceView layer for {package}; is Portal in the foreground?"
        )
    return match.group(0)


def format_stats_line(label, stats):
    if not stats:
        return f"{label}: n/a"
    return (
        f"{label}: avg={stats['avg']:.2f} ms, "
        f"p50={stats['p50']:.2f}, p95={stats['p95']:.2f}, "
        f"p99={stats['p99']:.2f}, max={stats['max']:.2f} ms (n={stats['count']})"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", help="adb device serial")
    parser.add_argument(
        "--package", default="app.polarbear", help="Portal package (Debug: app.polarbear)"
    )
    parser.add_argument(
        "--mode",
        choices=["drag", "observe", "idle"],
        default="drag",
        help="Workload mode: scripted touch drag, observe active rendering, or idle",
    )
    parser.add_argument(
        "--drag", default="1700 1000 2200 1300", help="x0 y0 x1 y1 in screen pixels"
    )
    parser.add_argument(
        "--ms", type=int, default=1500, help="drag duration or observation window in ms"
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=1,
        help="number of back-to-back drag passes (alternates direction when > 1)",
    )
    parser.add_argument(
        "--poll-ms",
        type=int,
        default=250,
        help="SurfaceFlinger --latency polling interval in ms (prevents 128-frame overflow)",
    )
    parser.add_argument(
        "--settle-ms",
        type=int,
        default=150,
        help="post-workload settle wait in ms so final fences resolve",
    )
    parser.add_argument(
        "--target-hz",
        type=float,
        default=None,
        help="reference refresh rate (Hz) for missed-refresh calculation (default: active mode)",
    )
    parser.add_argument(
        "--no-logcat",
        action="store_true",
        help="skip capturing anland.fps / anland.render logcat lines",
    )
    parser.add_argument(
        "--json",
        dest="json_out",
        help="write full JSON report to path (or '-' for stdout)",
    )
    args = parser.parse_args()

    layer = find_layer(args.serial, args.package)

    # Record latest pre-clear timestamp so stale entries can never leak in.
    _, pre_rows, _ = parse_latency_dump(
        adb_shell(args.serial, f"dumpsys SurfaceFlinger --latency '{layer}'")
    )
    min_present_ns = max((r[1] for r in pre_rows), default=0)

    logcat_since = None
    if not args.no_logcat:
        try:
            logcat_since = adb_shell(args.serial, "date '+%m-%d %H:%M:%S.000'").strip()
        except subprocess.CalledProcessError:
            logcat_since = None

    adb_shell(args.serial, f"dumpsys SurfaceFlinger --latency-clear '{layer}'")

    samples_store = {}
    active_periods_ns = []
    all_periods_ns = []
    display_modes = []
    power_samples = []
    diag_totals = collections.Counter()
    stop_event = threading.Event()
    poll_errors = []

    def sample_battery_once():
        try:
            dump = adb_shell(args.serial, "dumpsys battery")
            power_samples.append((time.monotonic(), parse_battery(dump)))
        except subprocess.CalledProcessError as exc:
            poll_errors.append(f"battery: {exc}")

    def poll_once(capture_mode=False):
        try:
            dump = adb_shell(args.serial, f"dumpsys SurfaceFlinger --latency '{layer}'")
            period_ns, rows, diag = parse_latency_dump(dump)
            if period_ns > 0:
                all_periods_ns.append(period_ns)
            added = merge_latency_rows(samples_store, rows, min_present_ns=min_present_ns)
            if period_ns > 0 and (added > 0 or rows):
                active_periods_ns.append(period_ns)
            for k, v in diag.items():
                diag_totals[k] += v
            if capture_mode:
                sf_mode_text = adb_shell(
                    args.serial, "dumpsys SurfaceFlinger | grep -m 2 'activeMode='"
                )
                mode = parse_display_mode(sf_mode_text)
                if mode:
                    display_modes.append(mode)
        except subprocess.CalledProcessError as exc:
            poll_errors.append(f"latency: {exc}")

    sample_battery_once()

    def poller():
        interval_s = max(0.05, args.poll_ms / 1000.0)
        tick = 0
        while not stop_event.wait(interval_s):
            tick += 1
            # Capture activeMode and battery periodically during the workload.
            poll_once(capture_mode=(tick == 2 or tick % 8 == 0))
            if tick % 4 == 0:
                sample_battery_once()

    thread = threading.Thread(target=poller, daemon=True)
    thread.start()

    try:
        if args.mode == "drag":
            coords = args.drag.split()
            if len(coords) != 4:
                raise SystemExit("--drag must be 'x0 y0 x1 y1'")
            fwd = " ".join(coords)
            rev = f"{coords[2]} {coords[3]} {coords[0]} {coords[1]}"
            for idx in range(max(1, args.repeat)):
                seg = fwd if idx % 2 == 0 else rev
                adb_shell(args.serial, f"input touchscreen draganddrop {seg} {args.ms}")
        else:
            time.sleep(max(0.1, args.ms / 1000.0))
    finally:
        if args.settle_ms > 0:
            time.sleep(args.settle_ms / 1000.0)
        stop_event.set()
        thread.join(timeout=5.0)
        poll_once(capture_mode=not display_modes)
        sample_battery_once()

    rows = sorted(samples_store.values(), key=lambda r: r[1])
    if args.mode != "idle" and len(rows) < 2:
        raise SystemExit("Too few presented frames; did the workload hit the desktop?")

    presents = [r[1] for r in rows]
    period_pool = active_periods_ns if active_periods_ns else all_periods_ns
    active_period_ns = choose_active_period_ns(period_pool, presents)
    analysis = analyze_frames(rows, active_period_ns, target_hz=args.target_hz)
    power = summarize_power(power_samples)

    logcat_info = None
    if not args.no_logcat and logcat_since:
        try:
            raw_log = adb_shell(
                args.serial,
                f"logcat -d -T '{logcat_since}' | grep -E 'anland\\.(fps|render|pacing)' | tail -n 30",
            )
            logcat_info = parse_anland_logcat(raw_log)
        except subprocess.CalledProcessError:
            logcat_info = None

    # Prefer the highest-Hz display mode observed mid-run.
    best_mode = max(display_modes, key=lambda m: m["vsync_hz"]) if display_modes else None

    report = {
        "layer": layer,
        "mode": args.mode,
        "duration_ms": args.ms * (max(1, args.repeat) if args.mode == "drag" else 1),
        "display_mode": best_mode,
        "analysis": analysis,
        "power": power,
        "poll_diagnostics": dict(diag_totals),
        "logcat": logcat_info,
        "errors": poll_errors,
        # Raw (desired, present, ready) rows, for per-frame analysis.
        "rows": [list(r) for r in rows],
    }

    if args.json_out == "-":
        print(json.dumps(report, indent=2))
        return

    mode_str = (
        f"{best_mode['resolution']} @ {best_mode['vsync_hz']:.2f} Hz"
        if best_mode
        else "unknown"
    )
    print(
        f"mode={args.mode}, display={mode_str}, "
        f"active_refresh={analysis['active_refresh_hz']:.1f} Hz "
        f"(ref={analysis['ref_refresh_hz']:.1f} Hz), "
        f"frames={analysis['frames']}, eff_fps={analysis['effective_fps']:.1f}"
    )
    print(format_stats_line("frame interval", analysis["frame_interval_ms"]))
    print(
        f"present intervals (vsyncs: count): {analysis['intervals_vsyncs']} | "
        f"on consecutive vsyncs: {analysis['consecutive_vsyncs']}/{analysis['total_intervals']} | "
        f"missed refreshes: {analysis['missed_refresh_frames']} frames "
        f"({analysis['missed_refresh_pct']:.2f}%, {analysis['missed_refresh_slots']} slots)"
    )
    print(format_stats_line("queue->present (actual latency)", analysis["queue_to_present_ms"]))
    print(format_stats_line("queue->ready   (GPU completion)", analysis["gpu_completion_ms"]))
    print(format_stats_line("ready->present (compositor)    ", analysis["ready_to_present_ms"]))
    if analysis["excluded_desired_rows"] or analysis["excluded_ready_rows"] or analysis["late_gpu_frames"]:
        print(
            f"timestamp audit: excluded_desired={analysis['excluded_desired_rows']}, "
            f"excluded_ready={analysis['excluded_ready_rows']}, "
            f"late_gpu={analysis['late_gpu_frames']}"
        )
    if power:
        p_parts = [f"state={power['battery_state']}"]
        if power["avg_power_mw"] is not None:
            p_parts.append(
                f"avg={power['avg_power_mw']:.1f} mW "
                f"(min={power['min_power_mw']:.1f}, max={power['max_power_mw']:.1f}, "
                f"I={power['avg_current_ma']:.1f} mA, V={power['avg_voltage_mv']:.0f} mV)"
            )
        if power["charge_delta_uah"] is not None:
            p_parts.append(f"charge_delta={power['charge_delta_uah']} uAh/{power['elapsed_s']:.1f}s")
        print("power: " + ", ".join(p_parts))

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as fh:
            json.dump(report, fh, indent=2)


if __name__ == "__main__":
    main()
