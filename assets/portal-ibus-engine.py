#!/usr/bin/env python3
"""
Portal IBus engine: X11/GTK input-method bridge to the Android IME.

GTK3/X11 apps with GTK_IM_MODULE=ibus (Chromium/CEF/Electron on X11, GTK,
Qt XCB) open an IBus input context per editable widget. This engine receives
the real editable-focus lifecycle (FocusIn/FocusOut) and delivers host commits
arriving on /tmp/portal-ime-engine.fifo into the client.

Delivery, per what every IBus client implements:
  text       CommitText (the IM context's commit signal; any language)
  BackSpace  a real key press, asked of the host (KEY:14:<n> on the events
  Return     FIFO, KEY:28:1), which types it through the compositor
ForwardKeyEvent and DeleteSurroundingText are not used: GTK turns a forwarded
key into a synthetic GdkEvent that Chromium/CEF (Steam) ignore, and Chromium
ignores DeleteSurroundingText from IBus, so text, deletes and Enter vanished
there. Host commands run strictly in order: text after a key request waits
until the key has passed through this engine's ProcessKeyEvent (or a short
timeout), so a correction's "delete 3, type the" cannot land as "the" then
three deletes.

Host protocol on the command FIFO (same vocabulary as portal-ime-bridge):
  COMMIT:<base64 utf-8>   commit text (UTF-8, any language; "\n" = Return)
  DELETE:<n>              delete n chars before cursor
  ENTER                   Return key press+release
  PREEDIT:<base64 utf-8>  update preedit (empty hides): the word Android is
                          composing; committed by IBus on focus-out/reset
  FLUSH                   commit the current preedit

Focus is reported to the host on /tmp/portal-ime-events.fifo
(ACTIVATE:ibus/DEACTIVATE:ibus), reusing the existing Portal IME show/hide
path. The :ibus tag keeps this focus separate from the Wayland bridge's, so
a focus hop between an X11 and a Wayland field cannot race the keyboard off.

Physical keys always pass through (ProcessKeyEvent returns False), so an
attached hardware keyboard keeps working and IBus never swallows typing.
"""
import base64
import os
import select
import signal
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


def write_events(val):
    try:
        fd = os.open(EVENTS_FIFO, os.O_RDWR | os.O_NONBLOCK)
        try:
            os.write(fd, val)
            log(f"Notified Portal: {val.strip().decode()}")
        finally:
            os.close(fd)
        return True
    except Exception as e:
        log(f"Could not write events FIFO: {e}")
        return False


def notify_portal(active):
    write_events(b"ACTIVATE:ibus\n" if active else b"DEACTIVATE:ibus\n")


# Host key requests: evdev code -> the keysyms its press arrives as.
KEY_BACKSPACE = 14
KEY_ENTER = 28
KEY_SYMS = {KEY_BACKSPACE: (0xFF08,), KEY_ENTER: (0xFF0D, 0xFF8D)}
MAX_KEYS_PER_REQUEST = 64
# How long text waits for a requested key to come back through
# ProcessKeyEvent before going anyway (a client that does not route keys
# through IBus), and how long it waits after the last one so the client has
# applied it: the engine sees a key before the client handles it.
KEY_TIMEOUT_MS = 400
KEY_SETTLE_MS = 40


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
            self.preedit = ""

        def do_focus_in(self):
            self.do_focus_in_id('', '')

        def do_focus_in_id(self, object_path, client):
            # The daemon also emits probe FocusIns ('fake' = focus where input
            # is impossible, '' = warmup) that must not steal focus state or
            # summon the keyboard: only a real client context arms commits.
            if client in ('fake', ''):
                # With IBus's shared engine a real context losing focus
                # arrives as this FocusIn, not FocusOut (Chromium/Steam blur
                # the editable on every click elsewhere). Keeping the old
                # focus made every tap summon the keyboard.
                if self.focused:
                    log(f"FocusIn probe (client={client!r}) ends editable focus")
                    self.do_focus_out()
                else:
                    log(f"FocusIn probe ignored (client={client!r})")
                return
            log(f"FocusIn (editable focused, client={client})")
            self.focused = True
            self.focus_client = client
            notify_portal(True)

        def do_focus_out(self):
            log("FocusOut")
            self.focused = False
            # PreeditFocusMode.COMMIT: the daemon commits the preedit itself.
            self.preedit = ""
            try:
                self.hide_preedit_text()
            except Exception:
                pass
            notify_portal(False)

        def do_enable(self):
            log("Enable")

        def do_disable(self):
            log("Disable")
            # Portal must hear it too, or a stale ACTIVATE:ibus re-shows the
            # keyboard on every tap.
            if self.focused:
                self.do_focus_out()

        def do_reset(self):
            log("Reset")
            self.preedit = ""
            try:
                self.hide_preedit_text()
            except Exception:
                pass

        def do_process_key_event(self, keyval, keycode, state):
            # Never consume physical keys: hardware keyboards and Portal's
            # evdev key path keep working untouched.
            if not state & IBus.ModifierType.RELEASE_MASK:
                queue.key_seen(keyval)
            return False

        def do_set_cursor_location(self, x, y, w, h):
            log(f"SetCursorLocation({x},{y},{w},{h})")

        def do_set_content_type(self, purpose, hints):
            log(f"SetContentType(purpose={purpose},hints={hints})")

        def do_set_capabilities(self, caps):
            log(f"SetCapabilities({caps})")

        # --- host-driven actions (called on the main loop) ---
        def clear_preedit(self):
            if not self.preedit:
                return
            self.preedit = ""
            try:
                self.update_preedit_text_with_mode(
                    IBus.Text.new_from_string(""), 0, False,
                    IBus.PreeditFocusMode.CLEAR)
            except Exception as e:
                log(f"Preedit clear failed: {e}")

        def host_flush(self):
            if self.focused and self.preedit:
                text = self.preedit
                self.clear_preedit()
                self.host_commit(text)

        def host_commit(self, text):
            if not self.focused:
                log("commit ignored, no focus")
                return
            log(f"CommitText ({len(text)} chars)")
            self.clear_preedit()
            try:
                self.commit_text(IBus.Text.new_from_string(text))
            except Exception as e:
                log(f"CommitText failed: {e}")

        def host_preedit(self, text):
            if not self.focused:
                return
            if not text:
                self.clear_preedit()
                return
            try:
                # COMMIT mode: a focus-out or toolkit reset (click elsewhere)
                # commits the word instead of dropping it, like KWin does.
                self.update_preedit_text_with_mode(
                    IBus.Text.new_from_string(text), len(text), True,
                    IBus.PreeditFocusMode.COMMIT)
                self.preedit = text
                log(f"UpdatePreeditText ({len(text)} chars)")
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

    # Outliving the daemon (session ended) left an engine that still read the
    # shared command FIFO and swallowed every other line meant for the next
    # session's engine.
    def on_disconnected(_bus):
        log("IBus daemon connection lost; exiting")
        notify_portal(False)
        os._exit(0)

    bus.connect("disconnected", on_disconnected)

    # An engine that died with a field focused never sent DEACTIVATE, which
    # left Portal summoning the keyboard for a field nobody has. A new engine
    # has no focus; one being stopped gives its focus up.
    # GLib's handler, not signal.signal: Python runs those only between
    # bytecodes, and the main loop below blocks inside GLib.
    def on_term():
        log("Terminated; exiting")
        notify_portal(False)
        os._exit(0)

    GLib.unix_signal_add(GLib.PRIORITY_HIGH, signal.SIGTERM, on_term)
    notify_portal(False)

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
            # Block until the host writes; polling the empty FIFO woke this
            # thread five times a second for the whole session.
            try:
                select.select([fd], [], [])
                chunk = os.read(fd, 4096)
            except (BlockingIOError, InterruptedError):
                continue
            except Exception:
                time.sleep(0.2)
                continue
            if chunk:
                buf += chunk
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    handle_command(line.decode("utf-8", errors="ignore").strip())
            else:
                # O_RDWR keeps a writer open, so EOF should never happen;
                # never let it turn into a busy loop.
                time.sleep(0.1)

    def focused_engine():
        # Each GTK editable gets its own engine; target the focused one.
        return next((e for e in reversed(active) if e.focused), None)

    class HostQueue:
        """Host commands in arrival order, run on the main loop. A key
        request holds everything after it until the key is seen or times
        out."""

        def __init__(self):
            self.items = []
            self.waiting = None  # [evdev code, presses still expected]
            self.timer = 0

        def push(self, item):
            # A run of the same key (a held-down delete) still queued becomes
            # one request.
            last = self.items[-1] if self.items else None
            if (item[0] == "key" and last and last[0] == "key"
                    and last[1] == item[1]
                    and last[2] + item[2] <= MAX_KEYS_PER_REQUEST):
                last[2] += item[2]
            else:
                self.items.append(item)
            self.run()
            return False

        def key_seen(self, keyval):
            if not self.waiting or keyval not in KEY_SYMS[self.waiting[0]]:
                return
            self.waiting[1] -= 1
            if self.waiting[1] <= 0:
                self.rearm(KEY_SETTLE_MS)

        def rearm(self, delay):
            if self.timer:
                GLib.source_remove(self.timer)
            self.timer = GLib.timeout_add(delay, self.release)

        def release(self):
            if self.waiting and self.waiting[1] > 0:
                log(f"key {self.waiting[0]}: {self.waiting[1]} press(es) "
                    "not seen; continuing")
            self.timer = 0
            self.waiting = None
            self.run()
            return False

        def run(self):
            while self.items and not self.waiting:
                item = self.items.pop(0)
                eng = focused_engine()
                if eng is None:
                    log(f"{item[0]} ignored, no focus")
                    continue
                kind = item[0]
                if kind == "key":
                    code = item[1]
                    count = min(item[2], MAX_KEYS_PER_REQUEST)
                    eng.clear_preedit()
                    log(f"KeyRequest({code} x{count})")
                    if write_events(f"KEY:{code}:{count}\n".encode()):
                        self.waiting = [code, count]
                        self.rearm(KEY_TIMEOUT_MS)
                elif kind == "commit":
                    eng.host_commit(item[1])
                elif kind == "preedit":
                    eng.host_preedit(item[1])
                elif kind == "flush":
                    eng.host_flush()

    queue = HostQueue()

    def enqueue(*items):
        for item in items:
            GLib.idle_add(queue.push, list(item))

    def commit_items(text):
        # A line break is the Return key: CommitText of "\n" is dropped or
        # inserted literally depending on the widget.
        text = text.replace("\r\n", "\n").replace("\r", "\n")
        items = []
        for i, part in enumerate(text.split("\n")):
            if i:
                items.append(("key", KEY_ENTER, 1))
            if part:
                items.append(("commit", part))
        return items

    def handle_command(line):
        if not line:
            return
        log(f"cmd: {line[:40]}")
        if line.startswith("COMMIT:") or line.startswith("COMMITKEYS:"):
            try:
                text = base64.b64decode(line.split(":", 1)[1]).decode(
                    "utf-8", errors="ignore")
            except Exception as e:
                log(f"COMMIT decode failed: {e}")
                return
            enqueue(*commit_items(text))
        elif line.startswith("DELETE:"):
            try:
                count = int(line[7:])
            except Exception:
                count = 1
            if count > 0:
                enqueue(("key", KEY_BACKSPACE, count))
        elif line == "ENTER" or line.startswith("ENTER:"):
            enqueue(("key", KEY_ENTER, 1))
        elif line.startswith("PREEDIT:"):
            try:
                text = base64.b64decode(line[8:]).decode("utf-8", errors="ignore")
            except Exception:
                text = ""
            enqueue(("preedit", text))
        elif line == "FLUSH":
            enqueue(("flush",))
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
