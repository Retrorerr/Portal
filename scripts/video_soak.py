#!/usr/bin/env python3
"""Real-use soak for Portal's hardware video stack, on a physical device.

Drives a scratch Firefox over Marionette in the live desktop, launched through
the session so it has the real SELinux domain and environment. Fails on what a
user would see, not on whether a clip finished:

- Videos play with audio. A dead audio graph stops the clock of every unmuted
  video, and muted clips never notice.
- The page samples each video once a second and keeps the samples, so a stall
  is recorded even while adb or the app is away.
- A run counts only if the decoder really was hardware. The test fails on a
  silent fall back to software.
- Android-side events (leave, freezer, screen off, rotate, killed helpers) are
  injected, and playback must recover without touching the app. Leaving and
  returning is a workaround for stalls, so stalls are judged before each event.

Usage:
  scripts/video_soak.py --serial <adb serial> --plan quick
  scripts/video_soak.py --plan soak --minutes 60 --report target/soak.json

Encode the clips first with scripts/video_soak_media.sh. On-screen numbers come from
the clips' burned-in frame counter; Firefox's requestVideoFrameCallback runs near
24/s whatever reaches the screen, so shown_fps only shows that pictures arrive at all.
"""
import argparse
import json
from pathlib import Path
import re
import shlex
import socket
import subprocess
import sys
import time

import numpy

from app_compat import visible_desktop
from guest_exec import PACKAGE, Session

MEDIA_DIR = Path.home() / '.cache/portal-soak'
GUEST_MEDIA = '.cache/portal-soak'
HTTP_PORT = 28765
MARIONETTE_PORT = 28828
PROFILE = '/tmp/portal-soak-profile'
STALL_FAIL_S = 2.0
RECOVERY_FAIL_S = 8.0

CLIPS = {
    'h264': 'h264_1080p30.mp4',
    'h264-60': 'h264_1080p60.mp4',
    'vp9': 'vp9_1080p30.webm',
    'av1': 'av1_1080p30.webm',
}
FEED_CLIPS = ['h264_480p.mp4', 'h264_720p.mp4', 'h264_portrait.mp4', 'h264_1080p30.mp4']

USER_JS = """
user_pref("marionette.port", %d);
user_pref("media.hardware-video-decoding.enabled", HW);
user_pref("media.hardware-video-decoding.force-enabled", HW);
user_pref("media.ffmpeg.vaapi.enabled", HW);
user_pref("media.autoplay.default", 0);
user_pref("full-screen-api.allow-trusted-requests-only", false);
user_pref("full-screen-api.warning.timeout", 0);
user_pref("media.autoplay.blocking_policy", 0);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("termsofuse.bypassNotification", true);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("browser.termsofuse.prefCheck", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
""" % MARIONETTE_PORT

SERVER = r'''
import http.server, os, re, socketserver, sys
os.chdir(sys.argv[1])
class H(http.server.SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_GET(self):
        path = self.translate_path(self.path.split("?")[0])
        if not os.path.isfile(path):
            return super().do_GET()
        size = os.path.getsize(path)
        m = re.match(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
        a, b = (int(m[1]), int(m[2] or size - 1)) if m else (0, size - 1)
        b = min(b, size - 1)
        self.send_response(206 if m else 200)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Content-Length", str(b - a + 1))
        if m:
            self.send_header("Content-Range", f"bytes {a}-{b}/{size}")
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(a)
            left = b - a + 1
            while left > 0:
                chunk = f.read(min(left, 262144))
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except OSError:
                    return
                left -= len(chunk)
class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
# One address per video on the feed page: Firefox holds a suspended connection for each
# preloaded video and allows six per host, where a real site multiplexes over HTTP/2.
import threading
for last in range(1, 7):
    threading.Thread(target=S((f"127.0.0.{last}", int(sys.argv[2])), H).serve_forever, daemon=True).start()
threading.Event().wait()
'''

# Runs in Marionette's sandbox, which survives between calls until navigation.
MONITOR = r'''
if (!window.__soak) {
  const soak = window.__soak = {samples: [], raf: 0, seen: new WeakMap(), next: 0};
  const hook = v => {
    if (soak.seen.has(v)) return soak.seen.get(v);
    const st = {id: soak.next++, frames: 0, last: 0, gap: 0};
    soak.seen.set(v, st);
    const tick = now => {
      st.frames++;
      if (st.last) st.gap = Math.max(st.gap, now - st.last);
      st.last = now;
      v.requestVideoFrameCallback(tick);
    };
    v.requestVideoFrameCallback(tick);
    return st;
  };
  const raf = () => { soak.raf++; requestAnimationFrame(raf); };
  requestAnimationFrame(raf);
  soak.timer = setInterval(() => {
    const vids = [];
    for (const v of document.querySelectorAll('video')) {
      const st = hook(v);
      const t = v.currentTime;
      let ahead = 0;
      for (let i = 0; i < v.buffered.length; i++)
        if (v.buffered.start(i) <= t + 0.2 && v.buffered.end(i) > t) ahead = v.buffered.end(i) - t;
      const q = v.getVideoPlaybackQuality();
      const r = v.getBoundingClientRect();
      vids.push({id: st.id, t, paused: v.paused, ended: v.ended, seeking: v.seeking, rs: v.readyState,
        ahead, frames: st.frames, gap: Math.round(st.gap), total: q.totalVideoFrames, dropped: q.droppedVideoFrames,
        muted: v.muted || v.volume === 0, w: v.videoWidth, h: v.videoHeight,
        visible: r.bottom > 0 && r.top < innerHeight && r.width > 0,
        src: (v.currentSrc || '').slice(0, 80), err: v.error ? v.error.code : 0});
      st.gap = 0;
    }
    soak.samples.push({now: Date.now(), hidden: document.hidden, raf: soak.raf, vids});
    if (soak.samples.length > 7200) soak.samples.shift();
  }, 1000);
}
'''

DRAIN = 'const s = window.__soak ? window.__soak.samples.splice(0) : null; return s === null ? null : JSON.stringify(s);'

FEED_PAGE = r'''<!doctype html><meta charset="utf-8"><title>feed</title>
<body style="margin:0;background:#111;color:#eee;font:14px sans-serif"><script>
const clips = %s;
for (let i = 0; i < 24; i++) {
  const v = document.createElement('video');
  v.src = `http://127.0.0.${1 + i %% 6}:${location.port}/${clips[i %% clips.length]}?n=${i}`;
  v.muted = true; v.loop = true; v.preload = 'auto'; v.playsInline = true; v.controls = true;
  v.style.cssText = 'display:block;width:640px;height:480px;margin:40px auto;background:#000';
  document.body.appendChild(v);
}
const io = new IntersectionObserver(es => { for (const e of es)
  e.isIntersecting ? e.target.play().catch(()=>{}) : e.target.pause(); }, {threshold: 0.5});
document.querySelectorAll('video').forEach(v => io.observe(v));
</script>'''


# Plays the way YouTube's player does: separate audio and video SourceBuffers fed
# segment by segment, a trimmed back buffer, quality and codec switches, seeks.
MSE_PAGE = r'''<!doctype html><meta charset="utf-8"><title>adaptive</title>
<body style="margin:0;background:#000"><video id="v" controls playsinline
 style="display:block;width:100vw;height:100vh"></video><script>
const SEGMENTS = 30, SEG_S = 2, AHEAD_S = 15, BACK_S = 20;
const MIME = {h264: 'video/mp4; codecs="avc1.640028"', vp9: 'video/mp4; codecs="vp09.00.40.08"',
              av1: 'video/mp4; codecs="av01.0.08M.08"'};
const RUNGS = {h264: [1080, 720, 480], vp9: [1080, 480], av1: [1080, 480]};
const v = document.getElementById('v');
const ms = new MediaSource();
const player = window.player = {codec: new URLSearchParams(location.search).get('codec') || 'h264', rung: 0,
  errors: [], appended: 0, switches: 0, video: null, audio: null,
  setRung(i) { this.rung = i % RUNGS[this.codec].length; if (this.video) this.video.init = true; this.switches++; },
  setCodec(c) { this.codec = c; this.rung = 0; if (this.video) { this.video.change = MIME[c]; this.video.init = true; }
                this.switches++; }};
const name = t => t.kind === 'audio' ? 'mse_audio' : `mse_${player.codec}_${RUNGS[player.codec][player.rung]}`;
const done = sb => new Promise(r => sb.addEventListener('updateend', r, {once: true}));
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function get(url) {
  for (;;) {
    try { const r = await fetch(url); if (r.ok) return await r.arrayBuffer(); }
    catch (e) { player.errors.push('fetch ' + e); }
    await sleep(500);
  }
}
function ahead(sb) {
  const t = v.currentTime, b = sb.buffered;
  for (let i = 0; i < b.length; i++) if (b.start(i) <= t + 0.5 && b.end(i) > t) return b.end(i) - t;
  return 0;
}
async function pump(track) {
  const sb = track.sb;
  for (;;) {
    if (sb.updating) await done(sb);
    if (track.change) { sb.changeType(track.change); track.change = null; }
    if (track.init) {
      track.init = false;
      const data = await get(`${name(track)}_init.mp4`);
      sb.appendBuffer(data);
      await done(sb);
      continue;
    }
    if (sb.buffered.length && v.currentTime - sb.buffered.start(0) > BACK_S + 10) {
      sb.remove(0, v.currentTime - BACK_S);
      await done(sb);
      continue;
    }
    if (ahead(sb) >= AHEAD_S) { await sleep(200); continue; }
    const index = track.next++;
    const wanted = player.seekId;
    const data = await get(`${name(track)}_${String(index % SEGMENTS).padStart(3, '0')}.m4s`);
    if (wanted !== player.seekId) continue;
    sb.timestampOffset = Math.floor(index / SEGMENTS) * SEGMENTS * SEG_S;
    try {
      sb.appendBuffer(data);
      await done(sb);
      player.appended++;
    } catch (e) {
      player.errors.push(track.kind + ' append ' + e);
      if (e.name === 'QuotaExceededError') { sb.remove(0, v.currentTime - 2); await done(sb); track.next = index; continue; }
      await sleep(500);
    }
  }
}
v.addEventListener('seeking', () => {
  player.seekId = (player.seekId || 0) + 1;
  for (const t of [player.video, player.audio]) if (t) t.next = Math.floor(v.currentTime / SEG_S);
});
v.addEventListener('error', () => player.errors.push('video ' + v.error.code + ' ' + v.error.message));
ms.addEventListener('sourceopen', () => {
  ms.duration = 7200;
  player.video = {kind: 'video', next: 0, init: true, sb: ms.addSourceBuffer(MIME[player.codec])};
  player.audio = {kind: 'audio', next: 0, init: true, sb: ms.addSourceBuffer('audio/mp4; codecs="mp4a.40.2"')};
  pump(player.video); pump(player.audio);
});
v.src = URL.createObjectURL(ms);
v.muted = false; v.volume = 0.03;
v.play().catch(e => player.errors.push('play ' + e));
</script>'''


def log(*parts):
    print(time.strftime('%H:%M:%S'), *parts, flush=True)


def percentile(values, q):
    if not values:
        return 0
    ordered = sorted(values)
    return round(ordered[min(len(ordered) - 1, len(ordered) * q // 100)], 1)


class Marionette:
    def __init__(self, port):
        self.sock = socket.create_connection(('127.0.0.1', port), timeout=30)
        self.next_id = 0
        self.buffer = b''
        self._read()

    def _read(self):
        while b':' not in self.buffer:
            data = self.sock.recv(65536)
            if not data:
                raise ConnectionError('Marionette closed')
            self.buffer += data
        size, rest = self.buffer.split(b':', 1)
        size = int(size)
        while len(rest) < size:
            data = self.sock.recv(1 << 20)
            if not data:
                raise ConnectionError('Marionette closed')
            rest += data
        self.buffer = rest[size:]
        return json.loads(rest[:size])

    def call(self, name, **params):
        self.next_id += 1
        body = json.dumps([0, self.next_id, name, params]).encode()
        self.sock.sendall(str(len(body)).encode() + b':' + body)
        while True:
            reply = self._read()
            if reply[1] == self.next_id:
                if reply[2]:
                    raise RuntimeError(f'{name}: {reply[2].get("error")}: {reply[2].get("message", "")[:200]}')
                return reply[3]

    def script(self, source, *args):
        return self.call('WebDriver:ExecuteScript', script=source, args=list(args), sandbox='soak',
                         newSandbox=False, scriptTimeout=20000).get('value')


class Device:
    def __init__(self, serial):
        self.session = Session.connect(serial)
        self.serial = self.session.serial
        self.rootfs = self.session.record['rootfs']

    def adb(self, *args, timeout=60, **kwargs):
        return subprocess.run(['adb', '-s', self.serial, *args], timeout=timeout,
                              capture_output=True, **kwargs)

    def shell(self, command, timeout=60):
        return self.adb('shell', command, timeout=timeout, text=True, errors='replace').stdout

    def runas(self, command, timeout=120):
        return self.adb('exec-out', 'run-as', PACKAGE, 'sh', '-c', command,
                        timeout=timeout, text=True, errors='replace').stdout

    def locked(self):
        return 'deviceLocked=1' in self.shell('dumpsys trust | grep -m1 deviceLocked')

    def enter_desktop(self):
        """Bring the desktop in front with the veil gone, or raise: a test behind
        the veil or the keyguard measures nothing."""
        if self.locked():
            raise RuntimeError('The device is locked. Unlock it and run again.')
        self.foreground()
        time.sleep(3)
        return visible_desktop(self.session, dismiss_veil=True)

    def foreground(self):
        self.shell(f'am start -n {PACKAGE}/.PortalActivity')

    def guest_pids(self, comm):
        out = self.runas(f'for p in /proc/[0-9]*; do [ "$(cat $p/comm 2>/dev/null)" = {shlex.quote(comm)} ] && echo ${{p#/proc/}}; done')
        return out.split()

    def host_log_since(self, epoch_ms):
        # The log rotates, and a long run outlives one file.
        out = self.runas(f"cat files/diagnostics/host.log.1 files/diagnostics/host.log 2>/dev/null | awk '$1>={epoch_ms}'", timeout=120)
        return out.splitlines()


class Soak:
    def __init__(self, device, args):
        self.device = device
        self.args = args
        self.events = []
        self.samples = {}
        self.findings = []
        self.onscreen_results = []
        self.decoder_mapped = False
        self.marionette = None
        self.firefox_pid = None
        self.started_ms = int(time.time() * 1000)

    def run_guest(self, command, **kw):
        return self.device.session.run(command, capture_output=True, text=True, **kw)

    def setup(self):
        self.device.enter_desktop()
        # A leftover from an aborted run would answer on the same ports.
        self.run_guest('pkill -f "portal-soak-server|--profile /tmp/portal-soak-profile"; sleep 2; pkill -9 -f "portal-soak-server|--profile /tmp/portal-soak-profile"; true')
        self.push_media()
        user_js = shlex.quote(USER_JS.replace('HW', 'false' if self.args.software else 'true'))
        self.run_guest(f'rm -rf {PROFILE}; mkdir -p {PROFILE}; printf %s {user_js} > {PROFILE}/user.js')
        media = self.run_guest('echo $HOME').stdout.strip() + '/' + GUEST_MEDIA
        self.run_guest(f'cat > {media}/feed.html', input=FEED_PAGE % json.dumps(FEED_CLIPS), timeout=20)
        self.run_guest(f'cat > {media}/adaptive.html', input=MSE_PAGE, timeout=20)
        self.run_guest('cat > /tmp/portal-soak-server.py', input=SERVER, timeout=20)
        self.device.session.run_desktop(
            f'nohup python3 -I /tmp/portal-soak-server.py {shlex.quote(media)} {HTTP_PORT} >/dev/null 2>&1 </dev/null &',
            timeout=None, capture_output=True)
        self.launch_firefox()

    def launch_firefox(self):
        self.device.session.run_desktop(
            f'nohup firefox --no-remote --new-instance --marionette --profile {PROFILE} about:blank '
            '>/tmp/portal-soak-firefox.log 2>&1 </dev/null &', timeout=None, capture_output=True)
        self.device.adb('forward', f'tcp:{MARIONETTE_PORT}', f'tcp:{MARIONETTE_PORT}')
        deadline = time.time() + 120
        while True:
            try:
                self.marionette = Marionette(MARIONETTE_PORT)
                break
            except (OSError, ValueError, ConnectionError):
                if time.time() > deadline:
                    raise RuntimeError('Firefox did not open Marionette')
                time.sleep(2)
        self.marionette.call('WebDriver:NewSession', capabilities={'alwaysMatch': {'pageLoadStrategy': 'none'}})
        log('firefox ready')

    def push_media(self):
        home = self.run_guest('echo $HOME').stdout.strip()
        self.run_guest(f'mkdir -p {home}/{GUEST_MEDIA}')
        have = self.run_guest(f'cd {home}/{GUEST_MEDIA} && stat -c "%n %s" * 2>/dev/null').stdout
        remote = dict(line.rsplit(' ', 1) for line in have.splitlines() if ' ' in line)
        for clip in sorted(MEDIA_DIR.iterdir()):
            if remote.get(clip.name) == str(clip.stat().st_size):
                continue
            log('push', clip.name)
            target = self.device.rootfs + home + '/' + GUEST_MEDIA + '/' + clip.name
            with clip.open('rb') as source:
                subprocess.run(['adb', '-s', self.device.serial, 'exec-in', 'run-as', PACKAGE, 'sh', '-c',
                                f'cat > {shlex.quote(target)}'], stdin=source, check=True, timeout=600)

    def event(self, name, **detail):
        now = int(time.time() * 1000)
        self.events.append(dict(name=name, at=now, **detail))
        log('event', name, detail or '')

    def open(self, url, tag):
        self.marionette.call('WebDriver:Navigate', url=url)
        time.sleep(1.5)
        self.marionette.script(MONITOR)
        self.current = tag
        self.event('open', tag=tag)

    def local(self, name):
        return f'http://127.0.0.1:{HTTP_PORT}/{name}'

    def play_clip(self, codec, tag=None):
        self.open(self.local(CLIPS[codec]), tag or codec)
        self.marionette.script('''const v = document.querySelector('video');
            v.muted = false; v.volume = 0.03; v.loop = true; v.play().catch(()=>{});''')

    def drain(self):
        try:
            data = self.marionette.script(MONITOR + DRAIN)
        except (RuntimeError, OSError, ConnectionError) as error:
            log('drain failed', error)
            return
        if data:
            self.samples.setdefault(self.current, []).extend(json.loads(data))

    def watch(self, seconds, step=5):
        end = time.time() + seconds
        while time.time() < end:
            time.sleep(min(step, max(0, end - time.time())))
            self.drain()

    def js(self, source, *args):
        return self.marionette.script(source, *args)

    def seek_pause(self):
        self.event('seeks')
        for frac in (0.8, 0.1, 0.5):
            self.js('const v=document.querySelector("video"); v.currentTime = v.duration * arguments[0];', frac)
            self.watch(6)
        self.event('pause')
        self.js('document.querySelector("video").pause();')
        self.watch(7)  # past Firefox's dormancy timeout, so the decoder is rebuilt
        self.event('resume')
        self.js('document.querySelector("video").play();')
        self.watch(8)

    # -- Android-side events --------------------------------------------------

    def leave(self, seconds):
        self.event('leave', seconds=seconds)
        self.device.shell('input keyevent 3')
        time.sleep(seconds)
        self.device.enter_desktop()
        self.event('return')

    def screen_off(self, wait_for_unlock=300):
        """Turning the screen off locks the device, and only its owner can
        unlock it, so this waits for them."""
        self.event('screen-off')
        self.device.shell('input keyevent 223')
        time.sleep(20)
        self.device.shell('input keyevent 224')
        log('screen on: unlock the device now')
        deadline = time.time() + wait_for_unlock
        while self.device.locked():
            if time.time() > deadline:
                raise RuntimeError('The device stayed locked')
            time.sleep(2)
        self.device.enter_desktop()
        self.event('screen-on')

    def hide_tab(self, seconds):
        """Firefox suspends a hidden video's decoder, then has to bring it back."""
        self.event('tab-hide', seconds=seconds)
        page = self.marionette.call('WebDriver:GetWindowHandle')['value']
        other = self.marionette.call('WebDriver:NewWindow', type='tab', focus=True)['handle']
        self.marionette.call('WebDriver:SwitchToWindow', handle=other, focus=True)
        time.sleep(seconds)
        self.marionette.call('WebDriver:CloseWindow')
        self.marionette.call('WebDriver:SwitchToWindow', handle=page, focus=True)
        self.event('tab-show')

    def fullscreen(self, name):
        self.event('fullscreen')
        self.js('document.querySelector("video").requestFullscreen().catch(e => { window.__fsError = String(e); });')
        self.watch(6)
        self.screenshot(name)
        self.onscreen(name)
        self.watch(5)
        self.event('fullscreen-exit')
        self.js('document.exitFullscreen().catch(() => {});')
        self.watch(6)

    def other_app_video(self, seconds=30):
        """Another Android app takes hardware decoders while Portal is in the background."""
        path = '/sdcard/Download/portal-soak.mp4'
        self.device.adb('push', str(MEDIA_DIR / CLIPS['h264']), path, timeout=300)
        try:
            self.event('leave', seconds=seconds, why='other app video')
            self.device.shell(f'am start -a android.intent.action.VIEW -t video/mp4 -d file://{path}')
            time.sleep(8)
            self.other_app_decoders = self.device.shell('dumpsys media.resource_manager | grep -c video-decoder').stdout.strip()
            log('foreground during other app', self.device.foreground(), 'decoders', self.other_app_decoders)
            self.screenshot('other-app')
            time.sleep(seconds)
            self.device.shell('input keyevent 3')
            self.device.enter_desktop()
            self.event('return')
        finally:
            self.device.shell(f'rm -f {path}')

    def onscreen(self, name, seconds=10):
        """What reached the panel: record the display and read the clip's burned-in
        frame counter. The page's counters can't see a frame lost after the browser,
        and Firefox's requestVideoFrameCallback runs near 24/s whatever is on screen."""
        local = Path(self.args.report).parent / f'{Path(self.args.report).stem}-{name}.mkv'
        local.parent.mkdir(parents=True, exist_ok=True)
        local.unlink(missing_ok=True)
        rect = json.loads(self.js("""const v = document.querySelector('video');
            const r = v.getBoundingClientRect();
            const fit = Math.min(r.width / v.videoWidth, r.height / v.videoHeight);
            const left = r.left + (r.width - v.videoWidth * fit) / 2, top = r.top + (r.height - v.videoHeight * fit) / 2;
            return JSON.stringify({dpr: devicePixelRatio, x: mozInnerScreenX + left, y: mozInnerScreenY + top,
                                   w: v.videoWidth * fit, h: v.videoHeight * fit});"""))
        subprocess.run(['scrcpy', '-s', self.device.serial, '--no-playback', '--no-window', '--no-audio', '--no-control',
                        '--max-size', '1696', '--video-bit-rate', '40M', '--record', str(local),
                        '--time-limit', str(seconds)], capture_output=True, timeout=seconds + 60)
        # testsrc2 draws the counter in the video's top-left corner, 5.5 x 3.3 % of it.
        to_panel = rect['dpr'] / 2
        # ffmpeg rounds a yuv420p crop to even numbers, and the reshape below needs the real size.
        width, height = max(32, int(rect['w'] * to_panel * 0.055)) & ~1, max(16, int(rect['h'] * to_panel * 0.033)) & ~1
        x, y = int(rect['x'] * to_panel) & ~1, int(rect['y'] * to_panel) & ~1
        raw = subprocess.run(['ffmpeg', '-nostdin', '-v', 'error', '-i', str(local), '-fps_mode', 'passthrough', '-vf',
                              f'crop={width}:{height}:{x}:{y},format=gray', '-f', 'rawvideo', '-'],
                             capture_output=True, timeout=180).stdout
        probe = subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'v', '-show_entries', 'packet=pts_time',
                                '-of', 'csv=p=0', str(local)], capture_output=True, text=True, timeout=60).stdout
        pts = numpy.array(sorted(float(value) for value in probe.split()))
        frames = numpy.frombuffer(raw, dtype=numpy.uint8).reshape(-1, height, width).astype(numpy.int16)
        count = min(len(frames), len(pts))
        changed = (numpy.abs(frames[1:count] - frames[:count - 1]) > 30).sum(axis=(1, 2)) > 3
        times = pts[1:count][changed]
        intervals = numpy.diff(times) * 1000 if len(times) > 1 else numpy.array([0])
        duration = float(pts[count - 1] - pts[0]) if count > 1 else 0
        result = dict(name=name, presents_per_s=round(count / max(duration, 1), 1),
                      pictures_per_s=round(len(times) / max(duration, 1), 1),
                      interval_ms_p99=round(float(numpy.percentile(intervals, 99))),
                      interval_ms_max=round(float(intervals.max())))
        self.onscreen_results.append(result)
        log('on screen', result)
        return result

    def screenshot(self, name):
        path = Path(self.args.report).parent / f'{Path(self.args.report).stem}-{name}.png'
        data = self.device.adb('exec-out', 'screencap', '-p', timeout=60).stdout
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def rotate(self):
        self.event('rotate')
        self.device.shell('wm user-rotation lock 1')
        time.sleep(6)
        self.device.shell('wm user-rotation lock 0')
        time.sleep(6)
        self.device.shell('wm user-rotation free')
        self.event('rotated')

    def check_decoder(self):
        """Remember whether the media process ever loaded the hardware driver. It's
        checked before a kill too, because a restarted one may run in software."""
        for pid in self.device.guest_pids('RDD Process'):
            if self.device.runas(f'grep -c termuxva /proc/{pid}/maps').strip() not in ('', '0'):
                self.decoder_mapped = True

    def kill_comm(self, comm):
        self.check_decoder()
        pids = self.device.guest_pids(comm)
        self.event('kill', comm=comm, pids=pids)
        if pids:
            self.device.runas('kill -9 ' + ' '.join(pids))

    def audio_probe(self):
        """A blocked pw-play means the audio graph is dead for everybody."""
        script = ('export PIPEWIRE_RUNTIME_DIR=/tmp; python3 -c "import wave;w=wave.open(\'/tmp/soak-probe.wav\',\'wb\');'
                  'w.setnchannels(2);w.setsampwidth(2);w.setframerate(48000);w.writeframes(bytes(48000))"; '
                  'timeout 5 pw-play --volume 0 /tmp/soak-probe.wav; echo rc=$?')
        out = self.run_guest(script, timeout=20).stdout
        ok = 'rc=0' in out
        if not ok:
            self.finding('audio-graph', 'pw-play blocked: the audio graph is not running')
        return ok

    def finding(self, kind, detail):
        self.findings.append(dict(kind=kind, detail=detail, at=int(time.time() * 1000)))
        log('FINDING', kind, detail)

    # -- Plans ----------------------------------------------------------------

    def plan_quick(self, minutes):
        for codec in ('h264', 'vp9', 'av1'):
            self.play_clip(codec)
            self.watch(15)
            self.screenshot(codec)
            self.onscreen(codec)
            self.seek_pause()
            self.audio_probe()

    def plan_feed(self):
        self.open(self.local('feed.html'), 'feed')
        self.event('feed-scroll')
        for position in list(range(0, 24)) * 2:
            self.js('window.scrollTo(0, arguments[0] * 520);', position)
            self.watch(3, step=3)
            if position == 5:
                self.screenshot('feed')
        self.js('const v=[...document.querySelectorAll("video")].find(v=>!v.paused); if(v){v.muted=false;v.volume=0.03;}')
        self.watch(15)

    def plan_events(self):
        self.play_clip('h264', 'events')
        self.watch(20)
        self.leave(10)
        self.watch(15)
        self.rotate()
        self.watch(10)
        if self.args.with_lock:
            self.screen_off()
            self.watch(15)
        self.hide_tab(30)
        self.watch(15)
        self.fullscreen('events-fullscreen')
        self.watch(5)
        self.other_app_video()
        self.watch(20)
        self.leave(240)
        self.watch(20)
        # The daemon first: killing the RDD process ends the hardware session it would interrupt.
        self.kill_comm('al_va_daemon.so')
        self.watch(25)
        self.kill_comm('RDD Process')
        self.watch(25)
        self.audio_probe()

    def plan_concurrent(self):
        self.open(self.local('feed.html'), 'concurrent')
        self.js('''for (const v of [...document.querySelectorAll("video")].slice(0, 6)) {
            v.style.width = '320px'; v.style.height = '180px'; v.style.margin = '4px'; v.style.display = 'inline-block';
            v.play().catch(()=>{}); }
            for (const v of [...document.querySelectorAll("video")].slice(6)) v.remove();''')
        self.watch(30)
        self.screenshot('concurrent')
        self.watch(30)

    def plan_adaptive(self):
        tag = 'adaptive'
        self.open(self.local('adaptive.html'), tag)
        self.watch(15)
        self.screenshot(tag)
        self.onscreen(tag)
        self.event('quality-switches')
        for rung in (1, 2, 0, 2, 1, 0):
            self.js('window.wrappedJSObject.player.setRung(arguments[0]);', rung)
            self.watch(14)
        self.fullscreen('adaptive-fullscreen')
        self.seek_pause()
        self.event('codec-switches')
        for codec in ('vp9', 'av1', 'h264'):
            self.js('window.wrappedJSObject.player.setCodec(arguments[0]);', codec)
            self.watch(25)
            self.onscreen(f'adaptive-{codec}')
        errors = self.js('return JSON.stringify(window.wrappedJSObject.player.errors);')
        if json.loads(errors):
            self.finding('player-error', errors[:400])
        self.audio_probe()

    def plan_site(self, url):
        """A real site, played as far as it lets an automated browser play."""
        self.open(url, 'site')
        self.watch(15)
        self.js('const v=document.querySelector("video"); if(v){v.muted=false;v.volume=0.03;v.play().catch(()=>{});}')
        self.watch(25)
        self.screenshot('site')
        self.watch(100)
        self.js('const v=document.querySelector("video"); if(v && isFinite(v.duration)) v.currentTime=Math.min(v.duration*0.5, v.currentTime+120);')
        self.watch(60)

    def plan_soak(self, minutes):
        end = time.time() + minutes * 60
        loop = 0
        while time.time() < end:
            loop += 1
            log('soak loop', loop)
            self.plan_quick(minutes)
            self.plan_feed()
            self.plan_concurrent()
            self.plan_adaptive()
            self.plan_events()

    # -- Judging --------------------------------------------------------------

    def analyse(self):
        report = dict(stalls=[], tags={})
        windows = self.excused_windows()
        for tag, samples in self.samples.items():
            stats = dict(samples=len(samples), videos=0, shown=0, played=0.0, dropped=0, total=0)
            per_second, gaps, rafs = [], [], []
            last_sample = None
            for sample in samples:
                if last_sample and sample['now'] > last_sample['now']:
                    rafs.append((sample['raf'] - last_sample['raf']) * 1000 / (sample['now'] - last_sample['now']))
                last_sample = sample
            previous = {}
            runs = {}
            for sample in samples:
                now = sample['now']
                for video in sample['vids']:
                    key = video['id']
                    last = previous.get(key)
                    previous[key] = (now, video, sample['hidden'])
                    if not last:
                        stats['videos'] += 1
                        continue
                    last_now, old, _ = last
                    dt = (now - last_now) / 1000
                    if dt <= 0:
                        continue
                    stats['played'] += max(0, video['t'] - old['t']) if video['t'] >= old['t'] else 0
                    stats['shown'] += max(0, video['frames'] - old['frames'])
                    if (video['visible'] and not sample['hidden'] and not video['paused'] and not old['paused']
                            and not video['seeking'] and not old['seeking']):
                        per_second.append((video['frames'] - old['frames']) / dt)
                        gaps.append(video.get('gap', 0))
                    stats['dropped'] += max(0, video['dropped'] - old['dropped'])
                    stats['total'] += max(0, video['total'] - old['total'])
                    playing = not (video['paused'] or video['ended'] or video['seeking'] or old['paused']
                                   or old['seeking'] or video['err'])
                    excused = any(a <= now <= b for a, b in windows)
                    clock_stuck = playing and video['ahead'] > 1 and abs(video['t'] - old['t']) < 0.2 * dt
                    run = runs.setdefault(key, dict(clock=0.0, picture=0.0, start=None))
                    if clock_stuck and not excused:
                        run['start'] = run['start'] or last_now
                        run['clock'] += dt
                    else:
                        self.close_run(report, tag, key, run, video, 'clock')
                    picture_stuck = (playing and video['visible'] and not sample['hidden']
                                     and video['t'] - old['t'] > 0.5 * dt
                                     and video['frames'] == old['frames'])
                    if picture_stuck and not excused:
                        run['pstart'] = run.get('pstart') or last_now
                        run['picture'] += dt
                    else:
                        self.close_run(report, tag, key, run, video, 'picture')
                    if video['err']:
                        report['stalls'].append(dict(tag=tag, kind='error', code=video['err'], at=now, src=video['src']))
            for key, run in runs.items():
                last = previous[key][1]
                self.close_run(report, tag, key, run, last, 'clock')
                self.close_run(report, tag, key, run, last, 'picture')
            stats['shown_fps_p50'] = percentile(per_second, 50)
            stats['shown_fps_p5'] = percentile(per_second, 5)
            stats['raf_hz_p50'] = percentile(rafs, 50)
            stats['frame_gap_ms_p99'] = percentile(gaps, 99)
            stats['frame_gap_ms_max'] = max(gaps, default=0)
            report['tags'][tag] = stats
        return report

    @staticmethod
    def close_run(report, tag, key, run, video, kind):
        seconds = run[kind]
        start = run['start'] if kind == 'clock' else run.get('pstart')
        if seconds >= STALL_FAIL_S:
            report['stalls'].append(dict(tag=tag, video=key, kind=kind, seconds=round(seconds, 1), at=start,
                                         src=video['src'], muted=video['muted'], t=round(video['t'], 1)))
        run[kind] = 0.0
        if kind == 'clock':
            run['start'] = None
        else:
            run['pstart'] = None

    def excused_windows(self):
        """Time around an injected event, where the event may legitimately stop playback."""
        windows = []
        pairs = {'leave': 'return', 'screen-off': 'screen-on', 'rotate': 'rotated', 'tab-hide': 'tab-show'}
        for index, event in enumerate(self.events):
            closer = pairs.get(event['name'])
            if closer:
                end = next((e['at'] for e in self.events[index + 1:] if e['name'] == closer), None)
                if end:
                    windows.append((event['at'] - 500, end + RECOVERY_FAIL_S * 1000))
            elif event['name'] == 'kill':
                windows.append((event['at'] - 500, event['at'] + RECOVERY_FAIL_S * 1000))
            elif event['name'] in ('seeks', 'open', 'resume', 'feed-scroll', 'fullscreen', 'fullscreen-exit'):
                windows.append((event['at'] - 500, event['at'] + 4000))
        return windows

    def host_log_findings(self):
        lines = self.device.host_log_since(self.started_ms)
        patterns = {
            'daemon-stall': r'decoder stalled|no free slot|drain wait timed out|queueInputBuffer failed',
            'daemon-exit': r'\[VideoDecode\] daemon exited',
            'sink-lost-cycle': r'recovered lost graph cycles',
            'sink-gave-up': r'exiting so the supervisor starts a new sink',
            'listener-dead': r'damage listener stopped',
        }
        found = {}
        for line in lines:
            for kind, pattern in patterns.items():
                if re.search(pattern, line):
                    found.setdefault(kind, []).append(line[:260])
        sessions = sum('handshake ok: decoder' in line for line in lines)
        ended = [re.search(r'session end.*?(\d+) units in, (\d+) frames out', line) for line in lines]
        frames = sum(int(match[2]) for match in ended if match)
        return dict(sessions=sessions, sessions_ended=sum(1 for match in ended if match), hw_frames=frames, events={k: dict(count=len(v), first=v[0], last=v[-1]) for k, v in found.items()})

    def leak_check(self):
        out = self.device.runas('p=$(pidof libportal_va_daemon.so al_va_daemon.so 2>/dev/null | cut -d" " -f1); '
                                'echo daemon=$p; ls /proc/$p/fd | wc -l; ls -l /proc/$p/fd | grep -c dmabuf; '
                                'ls /proc/$p/task | wc -l; grep -E "^Rss" /proc/$p/smaps_rollup')
        return out.split()

    def finish(self):
        report = self.analyse()
        report['events'] = self.events
        report['findings'] = self.findings
        report['onscreen'] = self.onscreen_results
        report['host_log'] = self.host_log_findings()
        report['daemon_before_exit'] = self.leak_check()
        self.check_decoder()
        hw_decoder_mapped = self.decoder_mapped
        report['rdd_has_va_driver'] = hw_decoder_mapped
        try:
            self.marionette.call('Marionette:Quit')
        except (RuntimeError, OSError, ConnectionError):
            pass
        time.sleep(4)
        report['daemon_after_exit'] = self.leak_check()
        # The daemon logs a session when it ends, so read this after Firefox quit.
        report['host_log'] = self.host_log_findings()
        self.run_guest('pkill -f portal-soak-server.py; true')
        self.device.adb('forward', '--remove', f'tcp:{MARIONETTE_PORT}')
        report['thermal'] = self.device.shell('dumpsys thermalservice | grep -m1 "Thermal Status"').strip()
        fails = []
        for shot in report['onscreen']:
            if shot['pictures_per_s'] < 1:
                fails.append(f"{shot['name']}: no pictures detected on screen")
            if shot['interval_ms_max'] > 200:
                fails.append(f"{shot['name']}: picture held {shot['interval_ms_max']} ms on screen")
        if report['stalls']:
            fails.append(f"{len(report['stalls'])} stalls")
        if report['findings']:
            fails.append(f"{len(report['findings'])} findings")
        if not self.args.software and (not hw_decoder_mapped or not report['host_log']['hw_frames']):
            fails.append('never decoded in hardware')
        for kind in ('daemon-stall', 'daemon-exit', 'sink-gave-up', 'listener-dead'):
            count = report['host_log']['events'].get(kind, {}).get('count', 0)
            injected = sum(e['name'] == 'kill' and 'va_daemon' in e['comm'] for e in self.events) if kind == 'daemon-exit' else 0
            if count > injected:
                fails.append(kind)
        report['result'] = 'FAIL: ' + ', '.join(fails) if fails else 'PASS'
        return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--serial')
    parser.add_argument('--plan', default='quick', choices=['quick', 'feed', 'adaptive', 'events', 'concurrent', 'site', 'soak', 'hold'])
    parser.add_argument('--minutes', type=float, default=30)
    parser.add_argument('--url', default='https://player.vimeo.com/video/76979871', help='page for --plan site')
    parser.add_argument('--clip', default='h264', choices=sorted(CLIPS), help='clip for --plan hold')
    parser.add_argument('--report', default='target/video-soak.json')
    parser.add_argument('--software', action='store_true', help='decode in software, as a baseline')
    parser.add_argument('--with-lock', action='store_true',
                        help='also turn the screen off; the device owner must unlock it')
    args = parser.parse_args()
    soak = Soak(Device(args.serial), args)
    soak.setup()
    aborted = None
    try:
        if args.plan == 'quick':
            soak.plan_quick(args.minutes)
        elif args.plan == 'feed':
            soak.plan_feed()
        elif args.plan == 'events':
            soak.plan_events()
        elif args.plan == 'concurrent':
            soak.plan_concurrent()
        elif args.plan == 'adaptive':
            soak.plan_adaptive()
        elif args.plan == 'site':
            soak.plan_site(args.url)
        elif args.plan == 'soak':
            soak.plan_soak(args.minutes)
        elif args.plan == 'hold':
            soak.play_clip(args.clip, 'hold')
            soak.watch(15)
            soak.screenshot('hold')
            soak.onscreen('hold')
            soak.watch(10)
            soak.audio_probe()
            soak.audio_probe()
    except Exception as error:  # A plan that dies is a failed run, and the report still has to be written.
        aborted = f'{type(error).__name__}: {str(error)[:300]}'
        log('ABORTED', aborted)
    finally:
        report = soak.finish()
        if aborted:
            report['result'] = f'FAIL: aborted: {aborted}'
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        Path(args.report).write_text(json.dumps(report, indent=1))
    print(json.dumps({k: report[k] for k in ('result', 'tags', 'stalls', 'findings', 'onscreen', 'host_log', 'rdd_has_va_driver', 'daemon_before_exit', 'daemon_after_exit', 'thermal')}, indent=1))
    return 0 if report['result'] == 'PASS' else 1


if __name__ == '__main__':
    sys.exit(main())
