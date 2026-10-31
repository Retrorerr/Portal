import sys
from pathlib import Path
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from app_compat import frame_stats


class FrameStatsTests(unittest.TestCase):
    def test_discards_invalid_and_duplicate_presentations(self):
        rows = [(0, 0, 0), (0, 100_000_000, 0), (0, 100_000_000, 2),
                (0, 116_666_667, 0), (0, 133_333_334, 0), (0, 2**63-1, 0)]
        result = frame_stats(rows, 16_666_667)
        self.assertEqual(result['presented_frames'], 3)
        self.assertAlmostEqual(result['median_interval_ms'], 16.666667)
        self.assertEqual(result['gaps_over_100ms'], 0)

    def test_reports_stalls_and_insufficient_samples(self):
        self.assertEqual(frame_stats([], None)['status'], 'insufficient_samples')
        result = frame_stats([(0, 1, 0), (0, 200_000_001, 0)], 6_944_444)
        self.assertEqual(result['gaps_over_100ms'], 1)
        self.assertEqual(result['p95_interval_ms'], 200)


if __name__ == '__main__':
    unittest.main()
