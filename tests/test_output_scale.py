import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SOURCE = Path(__file__).resolve().parents[1] / "assets/localdesktop-output-scale.py"
SPEC = importlib.util.spec_from_file_location("output_scale", SOURCE)
HELPER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HELPER)

PHONE = (1272, 2772, 560)
MONITOR = (1080, 1920, 160)


class OutputScaleTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        state_path = Path(directory.name) / "output-scales.json"
        patcher = mock.patch.object(HELPER, "STATE_PATH", state_path)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.live_scale = None
        self.applied = []
        for name, replacement in (("live_output", self.live_output), ("set_scale", self.set_scale)):
            patcher = mock.patch.object(HELPER, name, replacement)
            patcher.start()
            self.addCleanup(patcher.stop)

    def live_output(self):
        return 1, self.live_scale

    def set_scale(self, output_id, scale):
        self.applied.append(scale)
        self.live_scale = scale

    def test_a_monitor_keeps_the_phones_interface_size(self):
        # Compact on the phone: 0.8 of its density baseline (1.9875).
        self.live_scale = HELPER.quantized(1.9875 * 0.8)
        HELPER.apply_size(PHONE)
        HELPER.apply_size(MONITOR)
        # 0.8 of the monitor's baseline (1.0), held at the 1.0 minimum.
        self.assertEqual(self.applied, [1.0])

        self.live_scale = 1.0
        HELPER.apply_size(PHONE)
        self.assertEqual(self.applied, [1.0, HELPER.quantized(1.9875 * 0.8)])

    def test_large_interface_carries_to_the_monitor(self):
        self.live_scale = HELPER.quantized(1.9875 * 1.15)
        HELPER.apply_size(PHONE)
        HELPER.apply_size(MONITOR)
        self.assertEqual(self.applied, [HELPER.quantized(1.15)])

    def test_state_without_a_density_falls_back_to_the_new_one(self):
        # Written by an earlier helper: no density recorded.
        baseline = HELPER.quantized(1.9875)
        HELPER.save_state({"current": "1272x2772", "scales": {"1272x2772": baseline}})
        self.live_scale = baseline
        HELPER.apply_size((1272, 2000, 560))
        self.assertEqual(self.applied, [])
        self.assertEqual(HELPER.load_state()["density"], 560)

    def test_same_size_records_a_new_density_without_rescaling(self):
        self.live_scale = HELPER.quantized(1.9875 * 0.8)
        HELPER.apply_size(PHONE)
        HELPER.apply_size((1272, 2772, 480))
        self.assertEqual(self.applied, [])
        self.assertEqual(HELPER.load_state()["density"], 480)


if __name__ == "__main__":
    unittest.main()
