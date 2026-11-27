#!/usr/bin/env python3
"""Benchmark Portal on fixed, real desktop tasks and score their smoothness.

Each task runs a scripted user action on the device (keys and touch through
Portal's normal input path) while `anland_frame_bench.py --mode observe`
records every frame SurfaceFlinger presents for Portal's desktop surface.

Scores, per task:
  * bursts: runs of presents closer than BURST_GAP_MS (an animation, a fling,
    a playing video). Idle gaps between actions are not counted as misses.
  * smooth %: intervals inside bursts that took exactly one 144 Hz vsync
    (at the display's active rate for `--target-hz`).
  * missed: 144 Hz vsyncs inside bursts that showed no new frame.
  * p95/p99/worst interval, presented fps, queue->present latency, power.

Video tasks report cadence instead: how steady the frame intervals are for
the content's rate (a 60 fps stream on a 144 Hz panel cannot be even).

Prerequisites: Portal Debug in front with the desktop visible (veil swiped
up), Firefox running for the web tasks, the stream tab for `twitch`.

  scripts/portal_real_tasks.py --serial <adb serial> --label base \
      --tasks launcher,overview,reddit,twitter,twitch --out target/real/base
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BENCH = REPO / "scripts" / "anland_frame_bench.py"
BURST_GAP_MS = 60.0

# Screen positions for the OnePlus Pad 3 (3392x2400 landscape), Firefox
# maximized: the middle of the page content.
PAGE_X, PAGE_TOP, PAGE_BOTTOM = 1700, 800, 1900

TASKS: dict[str, dict] = {
    # Kickoff: Alt+F1 opens the application launcher, Escape closes it.
    "launcher": {
        "ms": 9000,
        "script": "; ".join(
            ["input keycombination KEYCODE_ALT_LEFT KEYCODE_F1; sleep 1.2; input keyevent KEYCODE_ESCAPE; sleep 1.2"]
            * 3
        ),
    },
    # Overview (Meta+W; Ctrl+Alt+O is the same KWin shortcut without Meta,
    # which Android keeps from apps unless Portal's key service runs).
    "overview": {
        "ms": 12000,
        "script": "; ".join(
            [
                "input keycombination KEYCODE_CTRL_LEFT KEYCODE_ALT_LEFT KEYCODE_O; sleep 1.6; "
                "input keycombination KEYCODE_CTRL_LEFT KEYCODE_ALT_LEFT KEYCODE_O; sleep 1.6"
            ]
            * 3
        ),
    },
    # Touch flings down a feed, then back up.
    "reddit": {
        "url": "https://www.reddit.com/r/popular/",
        "ms": 12000,
        "script": "; ".join(
            [f"input touchscreen swipe {PAGE_X} {PAGE_BOTTOM} {PAGE_X} {PAGE_TOP} 90; sleep 0.9"] * 6
            + [f"input touchscreen swipe {PAGE_X} {PAGE_TOP} {PAGE_X} {PAGE_BOTTOM} 90; sleep 0.9"] * 3
        ),
    },
    "twitter": {
        "url": "https://x.com/home",
        "ms": 12000,
        "script": "; ".join(
            [f"input touchscreen swipe {PAGE_X} {PAGE_BOTTOM} {PAGE_X} {PAGE_TOP} 90; sleep 0.9"] * 6
            + [f"input touchscreen swipe {PAGE_X} {PAGE_TOP} {PAGE_X} {PAGE_BOTTOM} 90; sleep 0.9"] * 3
        ),
    },
    # A live stream with emote chat, watched without input.
    "twitch": {"url": "https://www.twitch.tv/ohnepixel", "settle_s": 15.0, "ms": 15000,
               "script": "sleep 14", "video": True},
}


def adb(serial: str, *args: str, check: bool = True) -> str:
    out = subprocess.run(
        ["adb", "-s", serial, *args], capture_output=True, text=True, check=False
    )
    if check and out.returncode != 0:
        raise SystemExit(f"adb {' '.join(args)} failed: {out.stderr.strip()}")
    return out.stdout


def navigate(serial: str, url: str, settle_s: float, tap: bool = True) -> None:
    """Open a URL in a new tab of the running Firefox (no typing, so no
    on-screen keyboard), then put the pointer over the page."""
    subprocess.run(
        [sys.executable, str(REPO / "scripts" / "guest_exec.py"), "--serial", serial,
         f"firefox --new-tab '{url}' >/dev/null 2>&1"],
        capture_output=True, text=True, check=False,
    )
    time.sleep(settle_s)
    if tap:
        adb(serial, "shell", f"input tap {PAGE_X} {PAGE_BOTTOM + 250}")
        time.sleep(1.0)


def analyse(rows: list[list[int]], refresh_hz: float, video: bool) -> dict:
    period_ms = 1000.0 / refresh_hz
    presents = sorted(r[1] / 1e6 for r in rows if len(r) > 1 and r[1] > 0)
    latencies = [
        (r[1] - r[0]) / 1e6 for r in rows if len(r) > 1 and r[0] > 0 and r[1] > r[0]
    ]
    intervals = [b - a for a, b in zip(presents, presents[1:])]
    burst = [i for i in intervals if i < BURST_GAP_MS]
    vsyncs = [max(1, round(i / period_ms)) for i in burst]
    result: dict = {
        "frames": len(presents),
        "burst_intervals": len(burst),
        "smooth_pct": round(100.0 * sum(1 for v in vsyncs if v == 1) / len(vsyncs), 1)
        if vsyncs
        else None,
        "missed_vsyncs": sum(v - 1 for v in vsyncs),
        "burst_fps": round(1000.0 / statistics.mean(burst), 1) if burst else None,
        "p95_ms": round(sorted(burst)[int(len(burst) * 0.95)], 2) if burst else None,
        "p99_ms": round(sorted(burst)[min(len(burst) - 1, int(len(burst) * 0.99))], 2)
        if burst
        else None,
        "worst_ms": round(max(burst), 2) if burst else None,
        "latency_p50_ms": round(statistics.median(latencies), 2) if latencies else None,
    }
    if video and intervals:
        steady = [i for i in intervals if i < 200]
        median = statistics.median(steady) if steady else 0
        result["video_fps"] = round(1000.0 / statistics.mean(steady), 1) if steady else None
        result["video_median_interval_ms"] = round(median, 2)
        # Intervals off the median by more than a third of a 144 Hz vsync:
        # visible judder on a steady stream.
        result["video_uneven_pct"] = round(
            100.0 * sum(1 for i in steady if abs(i - median) > period_ms / 3) / len(steady), 1
        )
    return result


def landscape(serial: str) -> bool:
    """The task coordinates are for the landscape desktop; a Pad picked up
    into portrait makes the swipes miss the page."""
    out = adb(serial, "shell", "dumpsys window displays", check=False)
    for token in out.split():
        if token.startswith("mCurrentRotation="):
            return token.split("=", 1)[1] in ("ROTATION_90", "ROTATION_270", "1", "3")
    return True


def run_task(serial: str, name: str, out_dir: Path, target_hz: float) -> dict:
    task = TASKS[name]
    if not landscape(serial):
        return {"error": "display is in portrait; task coordinates need landscape"}
    if task.get("url"):
        # No focus tap: touch scrolls whatever is under the finger, and a tap
        # on a feed opens a post (Reddit's image lightbox ate every swipe).
        navigate(serial, task["url"], task.get("settle_s", 10.0), tap=False)
    json_path = out_dir / f"{name}.json"
    bench = subprocess.Popen(
        [
            sys.executable,
            str(BENCH),
            "--serial",
            serial,
            "--mode",
            "observe",
            "--ms",
            str(task["ms"]),
            "--target-hz",
            str(target_hz),
            "--json",
            str(json_path),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    time.sleep(1.0)
    # Halfway through, a screenshot shows what the task actually ran on.
    shot = subprocess.Popen(
        ["adb", "-s", serial, "exec-out", f"sleep {task['ms'] / 2000:.1f}; screencap -p"],
        stdout=open(out_dir / f"{name}.png", "wb"),
    )
    adb(serial, "shell", task["script"], check=False)
    text, _ = bench.communicate(timeout=task["ms"] / 1000 + 60)
    shot.wait(timeout=30)
    if task.get("url"):
        adb(serial, "shell", "input keycombination KEYCODE_CTRL_LEFT KEYCODE_W")
        time.sleep(1.0)
    (out_dir / f"{name}.txt").write_text(text, encoding="utf-8")
    if not json_path.exists():
        return {"error": text.strip().splitlines()[-1] if text.strip() else "no report"}
    report = json.loads(json_path.read_text(encoding="utf-8"))
    result = analyse(report.get("rows", []), target_hz, bool(task.get("video")))
    analysis = report.get("analysis") or {}
    result["active_refresh_hz"] = analysis.get("active_refresh_hz")
    result["mode_hz_after"] = (report.get("display_mode") or {}).get("vsync_hz")
    gpu = analysis.get("gpu_completion_ms") or {}
    result["gpu_ms_avg"] = gpu.get("avg") if isinstance(gpu, dict) else None
    power = report.get("power") or {}
    result["power_mw"] = power.get("avg_power_mw")
    result["power_state"] = power.get("battery_state")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--tasks", default=",".join(TASKS))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--target-hz", type=float, default=144.0)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument(
        "--ab",
        help="alternate anland-tuning contents per run, e.g. 'damage_wait=1|damage_wait=0' "
        "(for knobs the consumer re-reads live)",
    )
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    arms = args.ab.split("|") if args.ab else [None]
    summary: dict = {"label": args.label, "arms": arms, "tasks": {}}
    for name in args.tasks.split(","):
        runs = []
        for index in range(args.repeat):
            for arm in arms:
                tag = f"{name}-{index + 1}" + (f"-{arm.replace('=', '')}" if arm else "")
                if arm is not None:
                    lines = arm.replace(",", r"\n")
                    adb(args.serial, "shell",
                        f"run-as app.polarbear sh -c 'printf \"{lines}\\n\" > files/anland-tuning'")
                    time.sleep(2.0)
                run_dir = args.out / tag
                run_dir.mkdir(exist_ok=True)
                result = run_task(args.serial, name, run_dir, args.target_hz)
                result["arm"] = arm
                runs.append(result)
                print(f"[{args.label}] {tag}: {json.dumps(result)}", flush=True)
        summary["tasks"][name] = runs
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
