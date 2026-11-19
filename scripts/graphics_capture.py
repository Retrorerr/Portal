"""Record Android composition and check a known Portal graphics scene."""

import contextlib
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time


# Four-by-four asymmetric scene, in top-left order. Tile 5 changes every 250 ms.
PALETTE = [(235, 48, 45), (42, 226, 82), (52, 91, 235), (232, 226, 46),
           (228, 62, 204), (43, 221, 225), (237, 237, 237), (40, 40, 40)]
TILES = [0, 1, 2, 3, 4, 7, 6, 0, 6, 3, 5, 1, 2, 4, 0, 5]
PHASE_TILE = 5
PHASES = [0, 1, 2, 3]
START_PHASE, END_PHASE = 4, 5
PATTERN = dict(palette=PALETTE, tiles=TILES, phase_tile=PHASE_TILE, phases=PHASES,
               start=START_PHASE, end=END_PHASE)
SIDE = 64
FRAME_BYTES = SIDE * SIDE * 3
MAX_RECORDING_SIZE = 1920


def recording_geometry(png, bounds):
    if png[:8] != b'\x89PNG\r\n\x1a\n' or png[12:16] != b'IHDR' or len(png) < 24:
        raise ValueError('Expected an Android screenshot with a PNG size header')
    width, height = struct.unpack_from('>II', png, 16)
    x0, y0, x1, y1 = bounds
    if not (0 <= x0 < x1 <= width and 0 <= y0 < y1 <= height):
        raise ValueError('Portal SurfaceView bounds do not fit the recorded display')
    return x0 / width, y0 / height, (x1 - x0) / width, (y1 - y0) / height


@contextlib.contextmanager
def screen_recording(serial, destination, seconds, scrcpy):
    metadata = dict(status='disabled')
    if destination is None:
        yield metadata
        return
    with tempfile.TemporaryDirectory(dir=destination.parent, prefix='.recording-') as work, \
         tempfile.TemporaryFile() as log:
        video = Path(work) / 'capture.mp4'
        command = [scrcpy, '--serial', serial, '--no-window', '--no-playback', '--no-control',
                   '--no-audio', '--max-size', str(MAX_RECORDING_SIZE), '--video-bit-rate', '12M',
                   '--time-limit', str(seconds), '--record', str(video)]
        # Use the same ADB as the compatibility runner. A portable scrcpy
        # bundle may include a different ADB version and restart the daemon.
        environment = os.environ | {'ADB': shutil.which('adb') or 'adb'}
        started = time.monotonic()
        try:
            recorder = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log,
                                        stderr=subprocess.STDOUT, env=environment)
        except OSError as error:
            metadata.update(status='unavailable', reason=str(error))
            yield metadata
            return
        try:
            while recorder.poll() is None and not video.exists() and time.monotonic() - started < 10:
                time.sleep(0.1)
            yield metadata
        finally:
            try:
                status = recorder.wait(timeout=max(0, started + seconds + 15 - time.monotonic()))
                if status == 0 and video.exists() and video.stat().st_size > 0:
                    video.replace(destination)
                    metadata.update(status='captured', file=destination.name)
                else:
                    log.seek(0)
                    detail = log.read().decode(errors='replace').strip()
                    metadata.update(status='unavailable', reason=detail or f'scrcpy exited with status {status}')
            except (OSError, subprocess.TimeoutExpired) as error:
                metadata.update(status='unavailable', reason=str(error))
                if recorder.poll() is None:
                    recorder.kill()
                    recorder.wait(timeout=5)


def tile_offsets(tile):
    step = SIDE // 4
    x, y = tile % 4 * step, tile // 4 * step
    # Test interiors. Chroma subsampling blends across the tile boundaries.
    return [(row * SIDE + col) * 3 for row in range(y + 3, y + step - 3, 2)
            for col in range(x + 3, x + step - 3, 2)]


SAMPLE_OFFSETS = [tile_offsets(tile) for tile in range(len(TILES))]


def matches(pixels, color):
    return all(abs(actual - expected) <= 36 for actual, expected in zip(pixels, color))


def classify_frame(frame):
    """Return tile failures and a phase, if its tile matches."""
    bad = []
    phase = None
    for tile, offsets in enumerate(SAMPLE_OFFSETS):
        pixels = [frame[i:i + 3] for i in offsets]
        candidates = [START_PHASE, *PHASES, END_PHASE] if tile == PHASE_TILE else [TILES[tile]]
        for color in candidates:
            if sum(not matches(pixel, PALETTE[color]) for pixel in pixels) <= 1:
                if tile == PHASE_TILE:
                    phase = color
                break
        else:
            bad.append(tile)
    return bad, phase


def check_frames(frames):
    """Ignore recording lead-in/out and check every decoded frame within the scene."""
    first = last = None
    phases = set()
    start_seen = end_seen = False
    bad_frames = []
    bad_count = 0
    pending_unrecognized = 0
    for index, frame in enumerate(frames):
        if len(frame) != FRAME_BYTES:
            raise ValueError('Truncated decoded frame')
        bad, phase = classify_frame(frame)
        if len(bad) > 5:
            if last is not None:
                pending_unrecognized += 1
            continue
        if first is None:
            first = index
        if pending_unrecognized:
            bad_frames.extend(dict(frame=i, pattern_missing=True) for i in
                              range(last + 1, last + 1 + min(pending_unrecognized, 20 - len(bad_frames))))
            bad_count += pending_unrecognized
            pending_unrecognized = 0
        last = index
        if phase == START_PHASE:
            start_seen = True
        elif phase == END_PHASE:
            end_seen |= start_seen
        elif start_seen and not end_seen and phase is not None:
            phases.add(phase)
        if bad:
            if len(bad_frames) < 20:
                bad_frames.append(dict(frame=index, bad_tiles=bad))
            bad_count += 1
    complete = end_seen and last - first >= 7 and phases == set(PHASES)
    result = dict(status='inconclusive', first_frame=first, last_frame=last, phases_seen=sorted(phases),
                  start_marker_seen=start_seen, end_marker_seen=end_seen, coverage_complete=complete,
                  bad_frame_count=bad_count, examples=bad_frames)
    if bad_count:
        result['status'] = 'failed'
    elif complete:
        result['status'] = 'passed'
    else:
        result['reason'] = 'The recording did not show enough of the expected fullscreen scene'
    return result


def check_recording(path, crop, ffmpeg='ffmpeg'):
    x, y, width, height = crop
    filters = (f'crop=iw*{width:.9f}:ih*{height:.9f}:iw*{x:.9f}:ih*{y:.9f},'
               f'scale={SIDE}:{SIDE}:flags=area')
    try:
        decoded = subprocess.run([ffmpeg, '-nostdin', '-v', 'error', '-xerror', '-i', str(path), '-an',
                                  '-vf', filters, '-fps_mode', 'passthrough', '-pix_fmt', 'rgb24',
                                  '-f', 'rawvideo', '-'], capture_output=True, check=True, timeout=90)
        pixels = memoryview(decoded.stdout)
        return check_frames(pixels[i:i + FRAME_BYTES] for i in range(0, len(pixels), FRAME_BYTES))
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        detail = getattr(error, 'stderr', None)
        return dict(status='inconclusive', reason=detail.decode(errors='replace').strip() if detail else str(error))
