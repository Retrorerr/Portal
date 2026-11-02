import sys
from pathlib import Path
import unittest
import tempfile
import json
import hashlib
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from app_compat import frame_stats, desktop_visibility, presentation_for_flow, fixture_files, unavailable_launch, android_codec_components


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

    def test_only_active_flow_counts_for_frame_pacing(self):
        rows = [(0, t, 0) for t in (1, 1_000_000_000, 1_016_000_000, 1_032_000_000, 2_000_000_000)]
        periods = {row[1]: 16_000_000 for row in rows}
        result = presentation_for_flow(rows, periods, [{'flow_start_ns': 1_000_000_000,
                                                       'flow_end_ns': 1_032_000_000}])
        self.assertEqual(result['scope'], 'active_flow')
        self.assertEqual(result['presented_frames'], 3)
        self.assertEqual(result['gaps_over_100ms'], 0)
        self.assertEqual(result['pacing_status'], 'no_gaps_over_100ms')
        self.assertFalse(presentation_for_flow(rows, periods, [])['continuous_flow'])
        self.assertEqual(presentation_for_flow(rows, periods, [])['pacing_status'], 'not_evaluated')


class DesktopVisibilityTests(unittest.TestCase):
    def ui(self, text='', package='app.polarbear'):
        return ('<?xml version="1.0"?><hierarchy><node package="' + package + '" '
                'class="android.view.SurfaceView" bounds="[0,0][3392,2400]"/>'
                '<node package="' + package + '" text="' + text + '" bounds="[1541,2269][1851,2303]"/>'
                '</hierarchy>UI hierarchy dumped')

    def test_live_surface_behind_veil_is_not_visible(self):
        state = desktop_visibility(self.ui('Swipe up to enter Portal'))
        self.assertFalse(state['visible'])
        self.assertEqual(state['entry_bounds'], (1541, 2269, 1851, 2303))
        self.assertFalse(desktop_visibility(self.ui('Returning to Plasma'))['visible'])

    def test_only_debug_desktop_without_veil_is_accepted(self):
        self.assertTrue(desktop_visibility(self.ui())['visible'])
        self.assertFalse(desktop_visibility(self.ui(package='app.polarbear.portal'))['visible'])
        with self.assertRaises(RuntimeError):
            desktop_visibility('ERROR: no UI hierarchy')


class FixtureTests(unittest.TestCase):
    def test_fixture_integrity_is_checked_before_upload(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            clip = directory / 'clip.mp4'
            clip.write_bytes(b'fixture')
            manifest = {'h264': dict(file=clip.name, sha256=hashlib.sha256(clip.read_bytes()).hexdigest())}
            (directory / 'manifest.json').write_text(json.dumps(manifest))
            self.assertEqual(fixture_files(directory), {'h264': clip})
            clip.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'SHA256 mismatch'):
                fixture_files(directory)
            manifest['h264']['file'] = '../outside'
            (directory / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, 'filenames'):
                fixture_files(directory)


class MissingCapabilityTests(unittest.TestCase):
    def test_codec_evidence_uses_active_component_messages(self):
        text = ('1793615557.173 651 28224 D CCodecBuffers: [c2.qti.avc.decoder#147:1D-Input.Impl[N]] converted buffers\n'
                '1793615558.173 651 28225 W OplusMediaMonitor: c2.qti.hevc.decoder inputFps=30\n'
                '1793615559.173 651 28225 I Registry: supports c2.android.avc.decoder\n')
        self.assertEqual(android_codec_components(text), ['c2.qti.avc.decoder', 'c2.qti.hevc.decoder'])

    def test_missing_runtimes_are_reported_precisely(self):
        for message in ('ModuleNotFoundError: No module named PySide6',
                        '/bin/sh: 1: glxinfo: not found',
                        "FileNotFoundError: [Errno 2] No such file or directory: 'ffmpeg'",
                        'OSError: libgtk-x11-2.0.so.0: cannot open shared object file: No such file or directory',
                        'Windows runtime missing. Install Wine'):
            self.assertTrue(unavailable_launch(message), message)

    def test_optional_plugin_messages_do_not_hide_a_failed_flow(self):
        message = ('VLC debug: optional service not found\n'
                   "FileNotFoundError: [Errno 2] No such file or directory: '/tmp/pv-owned/rc'\n")
        self.assertFalse(unavailable_launch(message))


if __name__ == '__main__':
    unittest.main()
