#!/usr/bin/python3
"""Verify the installed Chromium with an owned WebGL page and disposable profile."""
import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
args = parser.parse_args()
if not Path('/usr/bin/chromium').is_file():
    print('Chromium runtime missing; install chromium through APT', file=sys.stderr)
    raise SystemExit(77)
receipt, start_ns, end_ns = None, None, None
page = '''<!doctype html><meta charset="utf-8"><title>Portal compatibility: Chromium WebGL</title>
<style>body{background:#192533;color:white;font:24px sans-serif}canvas{width:720px;height:480px}</style>
<h1>Native Chromium / WebGL</h1><p>Unicode: 日本語 café</p><canvas width="720" height="480"></canvas>
<script>
const gl=document.querySelector('canvas').getContext('webgl2');
let frames=0, pixels=false, renderer='unavailable', started;
if(gl){
 const debug=gl.getExtension('WEBGL_debug_renderer_info');
 renderer=gl.getParameter(debug ? debug.UNMASKED_RENDERER_WEBGL : gl.RENDERER);
 gl.clearColor(.125,.25,.5,1);gl.clear(gl.COLOR_BUFFER_BIT);
 const pixel=new Uint8Array(4);gl.readPixels(10,10,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);
 pixels=Math.abs(pixel[0]-32)<=1 && Math.abs(pixel[1]-64)<=1 && Math.abs(pixel[2]-128)<=1 && pixel[3]===255;
}
function animate(now){
 if(started===undefined){started=now;fetch('/start');}
 if(gl){gl.clearColor((frames%120)/120,.2,.4,1);gl.clear(gl.COLOR_BUFFER_BIT);}
 frames++;
 if(now-started<DURATION)requestAnimationFrame(animate);
 else fetch('/receipt',{method:'POST',body:JSON.stringify({webgl2:!!gl,correct_pixels:pixels,renderer,frames,user_agent:navigator.userAgent})});
}
requestAnimationFrame(animate);
</script>'''.replace('DURATION', str(round(args.seconds * 1000))).encode()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        global start_ns
        if self.path == '/start':
            if start_ns is None:
                start_ns = time.monotonic_ns()
            self.send_response(204)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(page)))
        self.end_headers()
        self.wfile.write(page)

    def do_POST(self):
        global receipt, end_ns
        length = int(self.headers.get('Content-Length', '0'))
        if self.path != '/receipt' or not 0 < length <= 65536:
            self.send_error(400)
            return
        receipt = json.loads(self.rfile.read(length))
        end_ns = time.monotonic_ns()
        self.send_response(204)
        self.end_headers()

    def log_message(self, *_):
        pass


with HTTPServer(('127.0.0.1', 0), Handler) as server, tempfile.TemporaryDirectory(prefix='portal-chromium-probe-') as profile, tempfile.TemporaryFile(mode='w+') as log:
    server.timeout = 0.25
    process = subprocess.Popen(['chromium', '--user-data-dir=' + profile, '--no-first-run',
                                '--no-default-browser-check', '--new-window',
                                f'http://127.0.0.1:{server.server_port}/'], stdout=log, stderr=log)
    try:
        deadline = time.monotonic() + args.seconds + 25
        while receipt is None and process.poll() is None and time.monotonic() < deadline:
            server.handle_request()
        log.seek(0)
        messages = log.read()
        print(messages, file=sys.stderr)
        if receipt is None:
            raise RuntimeError('Chromium did not complete its owned WebGL page')
        receipt.update(application='Chromium', flow_start_ns=start_ns, flow_end_ns=end_ns,
                       backend='wayland' if re.search(r'wl_surface[@#]\d+\.attach\(wl_buffer', messages) else 'unreported')
        print(json.dumps(receipt), flush=True)
        passed = receipt.get('webgl2') and receipt.get('correct_pixels') and receipt.get('frames', 0) > 1
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
raise SystemExit(0 if passed else 1)
