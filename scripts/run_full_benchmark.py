import subprocess
import time
import json
import re
import sys

ADB_DEVICE = "f105b146"
UID = "10487"

def run_adb(cmd: str) -> str:
    res = subprocess.run(["adb", "-s", ADB_DEVICE, "shell", cmd], capture_output=True, text=True, check=False)
    return res.stdout.strip()

def get_active_slot() -> str:
    return run_adb("run-as app.polarbear cat /data/data/app.polarbear/files/platform-state/active-slot 2>/dev/null") or "unknown"

def get_cgroup_pids() -> list:
    out = run_adb(f"cat /sys/fs/cgroup/apps/uid_{UID}/cgroup.procs /sys/fs/cgroup/apps/uid_{UID}/pid_*/cgroup.procs 2>/dev/null")
    pids = set()
    for line in out.splitlines():
        line = line.strip()
        if line.isdigit():
            pids.add(int(line))
    return sorted(list(pids))

def get_memory_stats():
    pids = get_cgroup_pids()
    if not pids:
        return {"count": 0, "total_rss_mb": 0, "total_anon_mb": 0, "total_pss_mb": 0, "top": []}

    paths = " ".join(f"/proc/{p}/status" for p in pids)
    raw = run_adb(f"grep -E '^(Name|VmRSS|RssAnon):' {paths} 2>/dev/null")

    proc_map = {}
    for line in raw.splitlines():
        m = re.match(r"/proc/(\d+)/status:(Name|VmRSS|RssAnon):\s+(\S+)", line.strip())
        if m:
            pid = int(m.group(1))
            k, v = m.group(2), m.group(3)
            if pid not in proc_map:
                proc_map[pid] = {"pid": pid, "name": "unknown", "rss": 0, "anon": 0}
            if k == "Name":
                proc_map[pid]["name"] = v
            elif k == "VmRSS":
                proc_map[pid]["rss"] = int(v)
            elif k == "RssAnon":
                proc_map[pid]["anon"] = int(v)

    # Kernel PSS via dumpsys meminfo
    meminfo = run_adb("dumpsys meminfo app.polarbear | grep 'TOTAL PSS:'")
    m_pss = re.search(r"TOTAL PSS:\s+(\d+)", meminfo)
    total_pss_kb = int(m_pss.group(1)) if m_pss else 0

    total_rss_kb = sum(p["rss"] for p in proc_map.values())
    total_anon_kb = sum(p["anon"] for p in proc_map.values())

    sorted_procs = sorted(proc_map.values(), key=lambda x: x["rss"], reverse=True)

    return {
        "count": len(proc_map),
        "total_pss_mb": round(total_pss_kb / 1024.0, 2),
        "total_rss_mb": round(total_rss_kb / 1024.0, 2),
        "total_anon_mb": round(total_anon_kb / 1024.0, 2),
        "top": sorted_procs[:6]
    }

def sample_cpu(duration: float = 2.0):
    pids = get_cgroup_pids()
    if not pids:
        return {"app_cpu_percent": 0.0, "system_cpu_percent": 0.0}

    def read_stats():
        paths = " ".join(f"/proc/{p}/stat" for p in pids)
        raw = run_adb(f"cat {paths} 2>/dev/null")
        proc_ticks = 0
        for line in raw.splitlines():
            parts = line.strip().split()
            if len(parts) >= 15:
                proc_ticks += int(parts[13]) + int(parts[14])
        
        stat_raw = run_adb("head -n 1 /proc/stat")
        stat_parts = stat_raw.split()
        cpu_total = sum(int(x) for x in stat_parts[1:])
        cpu_idle = int(stat_parts[4])
        return proc_ticks, cpu_total, cpu_idle

    p_t1, c_tot1, c_idl1 = read_stats()
    time.sleep(duration)
    p_t2, c_tot2, c_idl2 = read_stats()

    d_proc = max(0, p_t2 - p_t1)
    d_tot = max(1, c_tot2 - c_tot1)
    d_idl = max(0, c_idl2 - c_idl1)

    app_cpu = round((d_proc / d_tot) * 800.0, 2)
    sys_cpu = round(((d_tot - d_idl) / d_tot) * 800.0, 2)

    return {"app_cpu_percent": app_cpu, "system_cpu_percent": sys_cpu}

def measure_benchmark(label: str):
    print(f"\n--- Measuring: {label} ---")
    mem = get_memory_stats()
    cpu = sample_cpu(2.0)
    res = {
        "label": label,
        "active_processes": mem["count"],
        "kernel_pss_mb": mem["total_pss_mb"],
        "total_rss_mb": mem["total_rss_mb"],
        "total_anon_mb": mem["total_anon_mb"],
        "app_cpu_percent": cpu["app_cpu_percent"],
        "system_cpu_percent": cpu["system_cpu_percent"],
        "top_processes": mem["top"]
    }
    print(json.dumps(res, indent=2))
    return res

def main():
    slot = get_active_slot()
    print("==================================================")
    print(f"BENCHMARK RUN: Active Slot = {slot}")
    print("==================================================")

    results = []

    # 1. Idle Desktop
    results.append(measure_benchmark("Idle Desktop"))

    # 2. Background / Resume
    print("\n[Action] Testing Background (HOME key)...")
    run_adb("input keyevent KEYCODE_HOME")
    time.sleep(3)
    results.append(measure_benchmark("App In Background"))

    print("\n[Action] Testing Resume...")
    run_adb("am start -n app.polarbear/android.app.NativeActivity")
    time.sleep(3)
    results.append(measure_benchmark("App Resumed Foreground"))

    out_file = f"scripts/benchmark_results_{slot}.json"
    with open(out_file, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nSaved benchmark results to {out_file}")

if __name__ == "__main__":
    main()
