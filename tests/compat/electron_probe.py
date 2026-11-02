#!/usr/bin/python3
"""Exercise an Electron runtime with an isolated owned app and local CDP.

The panel tests rendering and text composition without application accounts.
It does not validate third-party applications' account or network features.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from urllib.request import urlopen
import websocket

parser = argparse.ArgumentParser()
parser.add_argument('--seconds', type=float, default=4)
parser.add_argument('--runtime', default='electron')
parser.add_argument('--launch-flag', action='append', default=[])
args = parser.parse_args()
binary = shutil.which(args.runtime)
if not binary:
    print('Electron runtime missing: provide an independently installed Electron runtime', file=sys.stderr)
    raise SystemExit(77)

with tempfile.TemporaryDirectory(prefix='portal-electron-probe-') as tmp, tempfile.TemporaryFile(mode='w+') as log:
    profile = Path(tmp)
    app = profile / 'app'
    app.mkdir()
    (app / 'package.json').write_text(json.dumps(dict(name='portal-compatibility-probe', version='1.0.0', main='main.js')))
    (app / 'main.js').write_text('''const {app,BrowserWindow}=require('electron');
app.whenReady().then(()=>{const window=new BrowserWindow({width:800,height:600});
window.webContents.once('did-finish-load',()=>console.log('PORTAL_ELECTRON_READY'));
window.loadURL('data:text/html,<title>Electron compatibility</title><h1>Native Electron</h1>');});
app.on('window-all-closed',()=>app.quit());''')
    env = dict(os.environ, XDG_CONFIG_HOME=str(profile / 'config'), XDG_CACHE_HOME=str(profile / 'cache'))
    process = subprocess.Popen([binary, str(app), '--user-data-dir=' + str(profile / 'data'),
                                '--remote-debugging-port=0', *args.launch_flag],
                               env=env, stdout=log, stderr=log, start_new_session=True)
    connection = None
    try:
        deadline = time.monotonic() + 30
        target = None
        while time.monotonic() < deadline and process.poll() is None:
            log.seek(0)
            messages = log.read()
            match = re.search(r'DevTools listening on ws://127\.0\.0\.1:(\d+)/', messages)
            if match and 'PORTAL_ELECTRON_READY' in messages:
                try:
                    with urlopen('http://127.0.0.1:' + match[1] + '/json/list', timeout=1) as response:
                        targets = json.load(response)
                    target = next((t for t in targets if t.get('type') == 'page' and t.get('webSocketDebuggerUrl')), None)
                    if target:
                        break
                except OSError:
                    pass
            time.sleep(0.1)
        if target is None:
            raise RuntimeError('Electron did not create a debuggable application window')
        connection = websocket.create_connection(target['webSocketDebuggerUrl'], timeout=args.seconds + 10,
                                                 suppress_origin=True, http_no_proxy=['127.0.0.1'])
        sequence, start_ns = 0, None
        def call(method, **params):
            global sequence, start_ns
            sequence += 1
            connection.send(json.dumps(dict(id=sequence, method=method, params=params)))
            while True:
                reply = json.loads(connection.recv())
                if (reply.get('method') == 'Runtime.bindingCalled' and
                        reply.get('params', {}).get('name') == 'portalProbeFlowStart'):
                    start_ns = time.monotonic_ns()
                if reply.get('id') == sequence:
                    if 'error' in reply:
                        raise RuntimeError(reply['error'])
                    return reply['result']
        def evaluate(expression, **extra):
            result = call('Runtime.evaluate', expression=expression, returnByValue=True, **extra)
            if 'exceptionDetails' in result:
                raise RuntimeError(result['exceptionDetails'])
            return result['result'].get('value')
        call('Page.bringToFront')
        # Use the runtime's default backend and renderer selection.
        evaluate('''(() => {
          const panel=document.createElement('div');panel.id='portal-probe';
          panel.style='position:fixed;inset:0;z-index:2147483647;background:#192533;color:white;padding:24px';
          panel.innerHTML='<h1>Electron compatibility</h1><input id="portal-input"><canvas width="640" height="360"></canvas>';
          document.documentElement.appendChild(panel);document.querySelector('#portal-input').focus();
          return true;
        })()''')
        call('Input.insertText', text='café ')
        call('Input.imeSetComposition', text='日本語', selectionStart=3, selectionEnd=3)
        call('Input.insertText', text='日本語')
        text = evaluate("document.querySelector('#portal-input').value")
        call('Runtime.addBinding', name='portalProbeFlowStart')
        observation = evaluate('''new Promise(resolve => {
          const gl=document.querySelector('#portal-probe canvas').getContext('webgl2');
          let renderer='unavailable',pixels=false,frames=0,start,warmStart,lastWarm,warmFrames=0,warmMaxGap=0;
          if(gl){const debug=gl.getExtension('WEBGL_debug_renderer_info');
            renderer=gl.getParameter(debug?debug.UNMASKED_RENDERER_WEBGL:gl.RENDERER);
            gl.clearColor(.125,.25,.5,1);gl.clear(gl.COLOR_BUFFER_BIT);
            const p=new Uint8Array(4);gl.readPixels(1,1,1,1,gl.RGBA,gl.UNSIGNED_BYTE,p);
            pixels=Math.abs(p[0]-32)<=1&&Math.abs(p[1]-64)<=1&&Math.abs(p[2]-128)<=1&&p[3]===255;}
          function animate(now){
            if(warmStart===undefined)warmStart=now;
            if(now-warmStart<2000){
              if(lastWarm!==undefined)warmMaxGap=Math.max(warmMaxGap,now-lastWarm);
              lastWarm=now;warmFrames++;
              if(gl){gl.clearColor((warmFrames%120)/120,.2,.4,1);gl.clear(gl.COLOR_BUFFER_BIT);}
              requestAnimationFrame(animate);return;
            }
            if(start===undefined){start=now;portalProbeFlowStart('begin');}
            if(gl){gl.clearColor((frames%120)/120,.2,.4,1);gl.clear(gl.COLOR_BUFFER_BIT);}frames++;
            if(now-start<DURATION)requestAnimationFrame(animate);
            else resolve({webgl2:!!gl,correct_pixels:pixels,renderer,frames,dpr:devicePixelRatio,
                          warmup_frames:warmFrames,warmup_max_gap_ms:warmMaxGap});}
          requestAnimationFrame(animate);
        })'''.replace('DURATION', str(round(args.seconds * 1000))), awaitPromise=True)
        observation.update(runtime=args.runtime, launch_flags=args.launch_flag,
                           text_composition=text == 'café 日本語', flow_start_ns=start_ns,
                           flow_end_ns=time.monotonic_ns(), coverage='owned_electron_application')
        print(json.dumps(observation), flush=True)
        assert start_ns is not None, 'No active-flow timing receipt from Electron'
        assert observation['webgl2'] and observation['correct_pixels'] and observation['text_composition']
        assert observation['frames'] >= args.seconds * 15
    finally:
        if connection:
            connection.close()
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=3)
        log.seek(0)
        print(log.read(), file=sys.stderr)
