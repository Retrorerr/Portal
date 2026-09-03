#!/usr/bin/env python3
"""
Repeatable benchmark script that accurately measures the complete Local Desktop
cgroup on the OnePlus Pad 3 (f105b146), aggregating true VmRSS and RssAnon across
all guest processes (KWin, Plasma, D-Bus, XWayland, PipeWire, apps) and the
host app, total PSS from Android's memory subsystem, and distinguishing process-tree
CPU from total system CPU over a sampling window.
"""

import subprocess
import time
import re
import sys

ADB_DEVICE = "f105b146"
PKG = "app.polarbear"
UID = "10487"

def adb_shell(cmd: str) -> str:
    res = subprocess.run(
        ["adb", "-s", ADB_DEVICE, "shell", cmd],
        capture_output=True,
        text=True,
        check=False
    )
    return res.stdout

def get_cgroup_pids() -> list:
    out = adb_shell(f"cat /sys/fs/cgroup/apps/uid_{UID}/cgroup.procs /sys/fs/cgroup/apps/uid_{UID}/pid_*/cgroup.procs 2>/dev/null")
    pids = set()
    for line in out.splitlines():
        line = line.strip()
        if line.isdigit():
            pids.add(int(line))
    return sorted(list(pids))

def get_cgroup_memory():
    pids = get_cgroup_pids()
    if not pids:
        return {"count": 0, "total_rss_mb": 0, "total_anon_mb": 0, "total_pss_mb": 0, "processes": []}

    # Fetch status for all PIDs in a single command
    paths = " ".join(f"/proc/{p}/status" for p in pids)
    raw = adb_shell(f"grep -E '^(Name|VmRSS|RssAnon):' {paths} 2>/dev/null")
    
    proc_map = {}
    for line in raw.splitlines():
        line = line.strip()
        m = re.match(r"/proc/(\d+)/status:(Name|VmRSS|RssAnon):\s+(\S+)", line)
        if m:
            pid = int(m.group(1))
            key = m.group(2)
            val = m.group(3)
            if pid not in proc_map:
                proc_map[pid] = {"pid": pid, "name": "unknown", "rss": 0, "anon": 0}
            if key == "Name":
                proc_map[pid]["name"] = val
            elif key == "VmRSS":
                proc_map[pid]["rss"] = int(val)
            elif key == "RssAnon":
                proc_map[pid]["anon"] = int(val)

    # Also query dumpsys meminfo for the kernel PSS summary
    dumpsys_raw = adb_shell(f"dumpsys meminfo {PKG}")
    pss_match = re.search(r"TOTAL PSS:\s+(\d+)", dumpsys_raw)
    total_pss_kb = int(pss_match.group(1)) if pss_match else sum(p["anon"] for p in proc_map.values())

    total_rss_kb = sum(p["rss"] for p in proc_map.values())
    total_anon_kb = sum(p["anon"] for p in proc_map.values())

    entries = sorted(proc_map.values(), key=lambda x: x["rss"], reverse=True)

    return {
        "count": len(proc_map),
        "total_rss_kb": total_rss_kb,
        "total_rss_mb": round(total_rss_kb / 1024.0, 2),
        "total_anon_kb": total_anon_kb,
        "total_anon_mb": round(total_anon_kb / 1024.0, 2),
        "total_pss_kb": total_pss_kb,
        "total_pss_mb": round(total_pss_kb / 1024.0, 2),
        "processes": entries
    }

def get_cgroup_cpu(sample_sec=2.0):
    pids = get_cgroup_pids()
    if not pids:
        return {"app_cpu_pct": 0.0, "total_sys_cpu_pct": 0.0, "sample_duration": sample_sec}

    paths = " ".join(f"/proc/{p}/stat" for p in pids)
    
    t1_stat = adb_shell("cat /proc/stat | grep '^cpu '")
    t1_procs = adb_shell(f"cat {paths} 2>/dev/null")
    time.sleep(sample_sec)
    t2_stat = adb_shell("cat /proc/stat | grep '^cpu '")
    t2_procs = adb_shell(f"cat {paths} 2>/dev/null")

    def parse_proc_stat(raw):
        ticks = {}
        for l in raw.splitlines():
            l = l.strip()
            parts = l.split()
            if len(parts) >= 15:
                try:
                    pid = int(parts[0])
                    ticks[pid] = int(parts[13]) + int(parts[14])
                except ValueError:
                    pass
        return ticks

    def parse_sys_stat(raw):
        parts = raw.strip().split()
        if len(parts) >= 5:
            user = int(parts[1])
            nice = int(parts[2])
            system = int(parts[3])
            idle = int(parts[4])
            total = sum(int(x) for x in parts[1:])
            return total, idle
        return 0, 0

    s1_total, s1_idle = parse_sys_stat(t1_stat)
    s2_total, s2_idle = parse_sys_stat(t2_stat)
    p1 = parse_proc_stat(t1_procs)
    p2 = parse_proc_stat(t2_procs)

    sys_total_delta = max(1, s2_total - s1_total)
    sys_busy_delta = max(0, (s2_total - s1_total) - (s2_idle - s1_idle))

    app_ticks_delta = 0
    for pid, ticks in p2.items():
        if pid in p1:
            app_ticks_delta += max(0, ticks - p1[pid])

    # 8 cores total on OnePlus Pad 3
    app_pct = (app_ticks_delta / sys_total_delta) * 800.0 if sys_total_delta > 0 else 0.0
    sys_pct = (sys_busy_delta / sys_total_delta) * 800.0 if sys_total_delta > 0 else 0.0

    return {
        "app_cpu_pct": round(app_pct, 2),
        "total_sys_cpu_pct": round(sys_pct, 2),
        "sample_duration": sample_sec
    }

def run_benchmark(label=""):
    print(f"============================================================")
    print(f"Local Desktop Benchmark: [{label}]")
    print(f"============================================================")
    mem = get_cgroup_memory()
    print(f"Active Cgroup Processes: {mem['count']}")
    print(f"Kernel Total PSS:        {mem['total_pss_mb']} MB ({mem['total_pss_kb']} KB)")
    print(f"Aggregated Total RSS:    {mem['total_rss_mb']} MB ({mem['total_rss_kb']} KB)")
    print(f"Aggregated Private Anon: {mem['total_anon_mb']} MB ({mem['total_anon_kb']} KB)")
    
    print("\nTop 7 Memory Consumers:")
    for p in mem["processes"][:7]:
        print(f"  PID {p['pid']:<6} {p['name']:<22} RSS: {p['rss']/1024.0:6.1f} MB  Anon: {p['anon']/1024.0:6.1f} MB")
        
    print("\nSampling CPU utilization (2s)...")
    cpu = get_cgroup_cpu(2.0)
    print(f"Local Desktop CPU Load:  {cpu['app_cpu_pct']}% of 1 core (out of 800% capacity)")
    print(f"Total Device CPU Load:   {cpu['total_sys_cpu_pct']}% of 1 core (Device Idle: {800.0 - cpu['total_sys_cpu_pct']:.1f}%)")
    print(f"============================================================\n")
    return {"mem": mem, "cpu": cpu}

if __name__ == "__main__":
    label = sys.argv[1] if len(sys.argv) > 1 else "Current State"
    run_benchmark(label)
