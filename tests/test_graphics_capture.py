import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from graphics_capture import (
    END_PHASE, FRAME_BYTES, PALETTE, PHASE_TILE, SIDE, START_PHASE, TILES,
    check_frames, recording_geometry,
)


def scene(phase, bad_tile=None):
    colors = TILES.copy()
    colors[PHASE_TILE] = phase
    if bad_tile is not None:
        colors[bad_tile] = 7
    tile_size = SIDE // 4
    return b''.join(bytes(PALETTE[colors[y // tile_size * 4 + x // tile_size]])
                    for y in range(SIDE) for x in range(SIDE))


class GraphicsCaptureTests(unittest.TestCase):
    def setUp(self):
        self.blank = bytes(FRAME_BYTES)
        self.sequence = [scene(phase) for phase in (START_PHASE, 0, 1, 2, 3, 0, 1, END_PHASE)]

    def test_bounds_are_relative_to_the_full_android_display(self):
        png = b'\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR' + struct.pack('>II', 3200, 2400)
        self.assertEqual(recording_geometry(png, (0, 120, 3200, 2280)), (0, 0.05, 1, 0.9))
        with self.assertRaises(ValueError):
            recording_geometry(png, (0, 0, 3200, 2500))

    def test_ignores_desktop_before_and_after_the_fixture(self):
        result = check_frames([self.blank, *self.sequence, self.blank])
        self.assertEqual(result['status'], 'passed')
        self.assertEqual((result['first_frame'], result['last_frame']), (1, 8))
        self.assertEqual(result['bad_frame_count'], 0)

    def test_reports_one_frame_flash_and_corrupted_region(self):
        frames = self.sequence[:3] + [self.blank, scene(2, bad_tile=9)] + self.sequence[4:]
        result = check_frames(frames)
        self.assertEqual(result['status'], 'failed')
        self.assertTrue(result['coverage_complete'])
        self.assertEqual(result['bad_frame_count'], 2)
        self.assertEqual(result['examples'], [
            {'frame': 3, 'pattern_missing': True},
            {'frame': 4, 'bad_tiles': [9]},
        ])

    def test_missing_markers_and_incomplete_animation_are_inconclusive(self):
        for frames in (self.sequence[1:], self.sequence[:-1], [scene(0)] * 20, [self.blank]):
            self.assertEqual(check_frames(frames)['status'], 'inconclusive')

    def test_truncated_decoded_frame_is_rejected(self):
        with self.assertRaises(ValueError):
            check_frames([self.sequence[0][:-3]])


if __name__ == '__main__':
    unittest.main()
