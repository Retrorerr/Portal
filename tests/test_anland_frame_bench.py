import importlib.util
from pathlib import Path
import unittest


def load_bench_module():
    path = Path(__file__).resolve().parents[1] / "scripts" / "anland_frame_bench.py"
    spec = importlib.util.spec_from_file_location("anland_frame_bench", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


bench = load_bench_module()


class AnlandFrameBenchTests(unittest.TestCase):
    def test_zero_desired_timestamp_is_excluded_from_latency(self):
        # Reproduces the live Pad 3 bug where col0 == 0 produced +8.3M ms latency.
        p = 6_944_444
        base = 8_324_923_771_406
        rows = [
            (0, base, base - 10_548_437),
            (base + p - 9_500_000, base + p, base + p - 6_000_000),
            (base + 2 * p - 10_500_000, base + 2 * p, base + 2 * p - 6_500_000),
        ]
        res = bench.analyze_frames(rows, active_period_ns=p)
        self.assertEqual(res["frames"], 3)
        self.assertEqual(res["excluded_desired_rows"], 1)
        self.assertEqual(res["queue_to_present_ms"]["count"], 2)
        self.assertAlmostEqual(res["queue_to_present_ms"]["avg"], 10.0, places=2)
        # GPU completion = ready - desired: 3.5 ms and 4.0 ms -> avg 3.75 ms
        self.assertEqual(res["gpu_completion_ms"]["count"], 2)
        self.assertAlmostEqual(res["gpu_completion_ms"]["avg"], 3.75, places=2)
        # ready->present still valid for all 3 rows
        self.assertEqual(res["ready_to_present_ms"]["count"], 3)

    def test_pending_and_bogus_timestamps_excluded(self):
        p = 6_944_444
        int64_max = 9_223_372_036_854_775_807
        dump = "\n".join(
            [
                str(p),
                "0\t0\t0",
                f"100000000\t{int64_max}\t{int64_max}",
                f"{int64_max}\t2000000000\t1995000000",
                f"2000000000\t{2000000000 + p}\t{int64_max}",
                f"{2000000000 + 2 * p - 8_000_000}\t{2000000000 + 2 * p}\t{2000000000 + 2 * p - 5_000_000}",
            ]
        )
        period_ns, rows, diag = bench.parse_latency_dump(dump)
        self.assertEqual(period_ns, p)
        self.assertEqual(diag["empty_rows"], 1)
        self.assertEqual(diag["pending_present_rows"], 1)
        self.assertEqual(len(rows), 3)

        res = bench.analyze_frames(rows, active_period_ns=p)
        self.assertEqual(res["excluded_desired_rows"], 1)
        self.assertEqual(res["excluded_ready_rows"], 1)
        self.assertEqual(res["queue_to_present_ms"]["count"], 2)
        self.assertEqual(res["gpu_completion_ms"]["count"], 1)
        self.assertAlmostEqual(res["gpu_completion_ms"]["avg"], 3.0, places=2)

    def test_merge_deduplicates_and_upgrades_pending_fields(self):
        int64_max = 9_223_372_036_854_775_807
        store = {}
        added1 = bench.merge_latency_rows(
            store,
            [
                (1000, 5000, 3000),
                (6000, 12000, int64_max),
            ],
            min_present_ns=1000,
        )
        self.assertEqual(added1, 2)
        added2 = bench.merge_latency_rows(
            store,
            [
                (6000, 12000, 9000),  # resolves pending ready_ns
                (13000, 19000, 16000),
            ],
            min_present_ns=1000,
        )
        self.assertEqual(added2, 1)
        self.assertEqual(store[12000], (6000, 12000, 9000))
        self.assertEqual(len(store), 3)

    def test_choose_active_period_ignores_post_idle_50hz_drop(self):
        p144 = 6_944_444
        p50 = 20_000_000
        presents = [1_000_000_000 + i * p144 for i in range(40)]
        chosen = bench.choose_active_period_ns([p50, p144, p50], presents)
        self.assertEqual(chosen, p144)

        # Idle/50 Hz frames should still select 50 Hz.
        presents_50 = [1_000_000_000 + i * p50 for i in range(10)]
        self.assertEqual(bench.choose_active_period_ns([p50], presents_50), p50)

    def test_missed_refresh_and_percentiles(self):
        p = 6_944_444
        # 9 consecutive vsyncs (1x) and 1 missed refresh (3x -> 2 missed slots)
        presents = [1_000_000_000]
        for mult in [1, 1, 1, 1, 1, 1, 1, 1, 1, 3]:
            presents.append(presents[-1] + mult * p)
        rows = [(t - 10_000_000, t, t - 6_000_000) for t in presents]
        res = bench.analyze_frames(rows, active_period_ns=p)
        self.assertEqual(res["consecutive_vsyncs"], 9)
        self.assertEqual(res["missed_refresh_frames"], 1)
        self.assertEqual(res["missed_refresh_slots"], 2)
        self.assertAlmostEqual(res["missed_refresh_pct"], 10.0, places=2)
        self.assertAlmostEqual(res["frame_interval_ms"]["p50"], p / 1e6, places=2)
        self.assertGreater(res["frame_interval_ms"]["max"], 20.0)

    def test_parse_battery_and_display_mode(self):
        battery_dump = """
Current OPLUS Battery Service state:
  Charger voltage : 0
  Battery current : 253
Current Battery Service state:
  AC powered: false
  USB powered: false
  Charge counter: 9481000
  status: 3
  level: 90
  voltage: 4284
  temperature: 238
"""
        b = bench.parse_battery(battery_dump)
        self.assertEqual(b["current_ma"], 253)
        self.assertEqual(b["voltage_mv"], 4284)
        self.assertEqual(b["charge_counter_uah"], 9481000)
        self.assertAlmostEqual(b["power_mw"], 253 * 4284 / 1000.0, places=2)

        sf_dump = (
            "activeMode={id=6, hwcId=6, resolution=3392x2400, vsyncRate=144.00 Hz, dpi=314.44x315.85}"
        )
        m = bench.parse_display_mode(sf_dump)
        self.assertEqual(m, {"resolution": "3392x2400", "vsync_hz": 144.0})


if __name__ == "__main__":
    unittest.main()
