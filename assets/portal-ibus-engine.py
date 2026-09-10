#!/usr/bin/env python3
"""
Portal IBus engine: X11/GTK input-method bridge to the Android IME.

GTK3/X11 apps with GTK_IM_MODULE=ibus (e.g. Firefox on XWayland) open an IBus
input context per editable widget. This engine receives the real editable-focus
lifecycle (FocusIn/FocusOut) and forwards host commits arriving on
/tmp/portal-ime-engine.fifo into the client via the IBus Engine API
(CommitText / DeleteSurroundingText / ForwardKeyEvent).

Host protocol on the command FIFO (same vocabulary as portal-ime-bridge):
  COMMIT:<base64 utf-8>   commit text (UTF-8, any language)
  DELETE:<n>              delete n chars before cursor
  ENTER                   Return key press+release
  PREEDIT:<base64 utf-8>  update preedit (empty hides); currently unused by
                          the host (Android exposes commits only)

Focus is reported to the host on /tmp/portal-ime-events.fifo
(ACTIVATE/DEACTIVATE), reusing the existing Portal IME show/hide path.

Physical keys always pass through (ProcessKeyEvent returns False), so an
attached hardware keyboard keeps working and IBus never swallows typing.
"""
import base64
import os
import sys
import threading
import time

ENGINE_FIFO = "/tmp/portal-ime-engine.fifo"
EVENTS_FIFO = "/tmp/portal-ime-events.fifo"
LOG_PATH = "/tmp/portal_ibus_engine.log"

log_file = None
try:
    log_file = open(LOG_PATH, "w")
except Exception:
    pass


def log(msg):
    if log_file:
        try:
            log_file.write(f"[{time.time():.3f}] {msg}\n")
            log_file.flush()
        except Exception:
            pass


def notify_portal(active):
    val = b"ACTIVATE\n" if active else b"DEACTIVATE\n"
    try:
        fd = os.open(EVENTS_FIFO, os.O_RDWR | os.O_NONBLOCK)
        try:
            os.write(fd, val)
            log(f"Notified Portal: {val.strip().decode()}")
        finally:
            os.close(fd)
    except Exception as e:
        log(f"Could not write events FIFO: {e}")


def main():
    from gi import require_version
    require_version('IBus', '1.0')
    from gi.repository import IBus, GLib, GObject

    IBus.init()

    class PortalEngine(IBus.EngineSimple):
        __gtype_name__ = 'PortalEngine'
        _bus_connection = None
        _next_path = "/org/freedesktop/IBus/Engine/Portal/0"

        def __init__(self):
            # has_focus_id: modern FocusIn(object_path, client) variant.
            super().__init__(connection=PortalEngine._bus_connection,
                             object_path=PortalEngine._next_path,
                             has_focus_id=True)
            self.focused = False

        def do_focus_in(self):
            self.do_focus_in_id('', '')

        def do_focus_in_id(self, object_path, client):
            # The daemon also emits probe FocusIns ('fake' = focus where input
            # is impossible, '' = warmup) that must not steal focus state or
            # summon the keyboard: only a real client context arms commits.
            if client in ('fake', ''):
                log(f"FocusIn probe ignored (client={client!r})")
                return
            log(f"FocusIn (editable focused, client={client})")
            self.focused = True
            self.focus_client = client
            notify_portal(True)

        def do_focus_out(self):
            log("FocusOut")
            self.focused = False
            try:
                self.hide_preedit_text()
            except Exception:
                pass
            notify_portal(False)

        def do_enable(self):
            log("Enable")

        def do_disable(self):
            log("Disable")
            self.focused = False

        def do_reset(self):
            log("Reset")
            try:
                self.hide_preedit_text()
            except Exception:
                pass

        def do_process_key_event(self, keyval, keycode, state):
            # Never consume physical keys: hardware keyboards and Portal's
            # evdev key path keep working untouched.
            return False

        def do_set_cursor_location(self, x, y, w, h):
            log(f"SetCursorLocation({x},{y},{w},{h})")

        def do_set_content_type(self, purpose, hints):
            log(f"SetContentType(purpose={purpose},hints={hints})")

        def do_set_capabilities(self, caps):
            log(f"SetCapabilities({caps})")

        # --- host-driven actions (called on the main loop) ---
        def host_commit_keys(self, text):
            # Delivery via a single ForwardKeyEvent per keysym (Unicode-safe:
            # keysyms, not keycodes, so no layout guessing; UCS keysyms
            # 0x01000000|codepoint cover all of Unicode incl. non-BMP).
            # NOTE: exactly ONE call per char — press+release pairs double
            # every character (verified: the release is implied). Plain
            # CommitText is deliberately not used: it silently no-ops on
            # clients whose IM context isn't in a committable state.
            if not self.focused:
                log("commit-keys ignored, no focus")
                return
            log(f"CommitKeys ({len(text)} chars)")
            try:
                for ch in text:
                    o = ord(ch)
                    if ch == "\n":
                        sym = 0xFF0D
                    elif o < 0x100:
                        sym = o
                    elif o <= 0x10FFFF:
                        sym = 0x01000000 | o
                    else:
                        continue
                    self.forward_key_event(sym, 0, 0)
            except Exception as e:
                log(f"CommitKeys failed: {e}")

        def host_delete(self, count):
            # BackSpace keysyms (DeleteSurroundingText is ignored by some
            # widgets; forwarded keysyms land everywhere commits do).
            if not self.focused:
                log("delete ignored, no focus")
                return
            log(f"DeleteKeys({count})")
            try:
                for _ in range(min(count, 64)):
                    self.forward_key_event(0xFF08, 0, 0)
            except Exception as e:
                log(f"DeleteKeys failed: {e}")

        def host_enter(self):
            if not self.focused:
                log("enter ignored, no focus")
                return
            log("ForwardKeyEvent(Return)")
            try:
                # keysym 0xff0d Return; keycode 0 lets the client map it.
                # Single call (press+release pairs double input).
                self.forward_key_event(0xff0d, 0, 0)
            except Exception as e:
                log(f"ForwardKeyEvent failed: {e}")

        def host_preedit(self, text):
            if not self.focused:
                return
            try:
                if text:
                    self.update_preedit_text(
                        IBus.Text.new_from_string(text), len(text), True)
                    log(f"UpdatePreeditText ({len(text)} chars)")
                else:
                    self.hide_preedit_text()
                    log("HidePreeditText")
            except Exception as e:
                log(f"Preedit failed: {e}")

    class PortalFactory(IBus.Factory):
        __gtype_name__ = 'PortalFactory'

        def __init__(self, bus):
            super().__init__(connection=bus.get_connection(),
                             object_path=IBus.PATH_FACTORY)
            self.bus = bus
            self.counter = 0

        def do_create_engine(self, engine_name):
            self.counter += 1
            PortalEngine._bus_connection = self.bus.get_connection()
            PortalEngine._next_path = (
                f"/org/freedesktop/IBus/Engine/Portal/{self.counter}")
            log(f"CreateEngine {engine_name} -> {PortalEngine._next_path}")
            engine = PortalEngine()
            active.append(engine)
            return engine

    active = []

    bus = IBus.Bus()
    if not bus.is_connected():
        log("FATAL: cannot connect to IBus daemon")
        sys.exit(1)

    exec_by_ibus = "--ibus" in sys.argv
    factory = PortalFactory(bus)
    if exec_by_ibus:
        # Launched on demand by ibus-daemon.
        bus.request_name("org.freedesktop.IBus.Portal", 0)
        log("running as daemon-spawned engine")
    else:
        component = IBus.Component(
            name="org.freedesktop.IBus.Portal",
            description="Portal Android IME bridge",
            version="1.0",
            license="GPL-3.0",
            author="Portal",
            homepage="",
            textdomain="portal")
        engine_desc = IBus.EngineDesc(
            name="portal",
            longname="Portal",
            description="Portal Android IME bridge (X11/GTK)",
            language="en",
            license="GPL-3.0",
            author="Portal",
            icon="",
            layout="us")
        component.add_engine(engine_desc)
        bus.register_component(component)
        log("component registered (standalone)")

    def fifo_loop():
        try:
            fd = os.open(ENGINE_FIFO, os.O_RDWR | os.O_NONBLOCK)
        except Exception as e:
            log(f"FATAL: cannot open {ENGINE_FIFO}: {e}")
            return
        buf = b""
        while True:
            try:
                chunk = os.read(fd, 4096)
            except Exception:
                time.sleep(0.2)
                continue
            if chunk:
                buf += chunk
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    handle_command(line.decode("utf-8", errors="ignore").strip())
            else:
                time.sleep(0.1)

    def handle_command(line):
        if not line:
            return
        log(f"cmd: {line[:40]}")
        if not active:
            log("no engine instance yet; dropping")
            return
        eng = active[-1]
        if line.startswith("COMMIT:"):
            try:
                text = base64.b64decode(line[7:]).decode("utf-8", errors="ignore")
                if text in ("\n", "\r\n", "\r"):
                    GLib.idle_add(eng.host_enter)
                else:
                    GLib.idle_add(eng.host_commit_keys, text)
            except Exception as e:
                log(f"COMMIT decode failed: {e}")
        elif line.startswith("COMMITKEYS:"):
            try:
                text = base64.b64decode(line[11:]).decode("utf-8", errors="ignore")
            except Exception as e:
                log(f"COMMITKEYS decode failed: {e}")
                return
            GLib.idle_add(eng.host_commit_keys, text)
        elif line.startswith("DELETE:"):
            try:
                count = int(line[7:])
            except Exception:
                count = 1
            GLib.idle_add(eng.host_delete, count)
        elif line == "ENTER" or line.startswith("ENTER:"):
            GLib.idle_add(eng.host_enter)
        elif line.startswith("PREEDIT:"):
            try:
                text = base64.b64decode(line[8:]).decode("utf-8", errors="ignore")
            except Exception:
                text = ""
            GLib.idle_add(eng.host_preedit, text)
        else:
            log(f"unknown command: {line[:30]}")

    for path in (ENGINE_FIFO,):
        try:
            if not os.path.exists(path):
                os.mkfifo(path, 0o666)
            os.chmod(path, 0o666)
        except Exception:
            pass

    t = threading.Thread(target=fifo_loop, daemon=True)
    t.start()

    log("portal-ibus-engine ready")
    # NOTE: plain context iteration instead of GLib.MainLoop().run(): the gi
    # MainLoop.run override installs a SIGINT wakeup fd (signal.set_wakeup_fd)
    # that fails under this supervised/proot fd environment (ENOENT). Pumping
    # the default context directly is equivalent here (fifo thread posts via
    # GLib.idle_add onto the same context).
    ctx = GLib.main_context_default()
    while True:
        ctx.iteration(True)


if __name__ == "__main__":
    main()
