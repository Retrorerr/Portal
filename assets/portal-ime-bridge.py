#!/usr/bin/env python3
"""
Portal IME Bridge for KWin 6.x
Connects to KWin's private WAYLAND_SOCKET as an input method (zwp_input_method_v1).
Relays text field activate/deactivate events to /tmp/portal-ime-events.fifo.
Reads text commit and delete commands from /tmp/portal-ime-commands.fifo and
forwards them directly through zwp_input_method_context_v1 (commit_string,
preedit_string, keysym) into the focused guest Wayland client.

Deletes are BackSpace keysyms only. KWin turns delete_surrounding_text into a
text-input-v3 delete that Qt clients (LibreOffice, verified) ignore while GTK
and Chromium apply it, so sending both double-deleted there. Qt in turn
applies a forwarded key after text-input events that arrive behind it in the
same burst, so text following a keysym waits KEY_SETTLE_S before going out.
"""
import base64
import os
import select
import socket
import struct
import subprocess
import sys
import time

EVENTS_FIFO = "/tmp/portal-ime-events.fifo"
COMMANDS_FIFO = "/tmp/portal-ime-commands.fifo"
LEGACY_FIFO = "/tmp/portal-ime.fifo"
LOG_PATH = "/tmp/portal_ime.log"
# Verified on LibreOffice: a BackSpace keysym and the commit right behind it
# reorder; spaced by human typing intervals they do not.
KEY_SETTLE_S = 0.06

try:
    log_file = open(LOG_PATH, "w")
except Exception:
    log_file = None


def log(msg: str):
    if log_file:
        try:
            log_file.write(f"[{time.time():.3f}] {msg}\n")
            log_file.flush()
        except Exception:
            pass


def notify_portal(active: bool):
    val = b"ACTIVATE\n" if active else b"DEACTIVATE\n"
    legacy_val = b"1\n" if active else b"0\n"

    # Write to both the events FIFO and legacy FIFO for compatibility
    for path, payload in ((EVENTS_FIFO, val), (LEGACY_FIFO, legacy_val)):
        try:
            fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
            try:
                os.write(fd, payload)
                log(f"Notified Portal ({path}): {payload.strip().decode()}")
            finally:
                os.close(fd)
        except Exception as e:
            log(f"Could not write to FIFO {path}: {e}")


def set_tablet_mode(mode: str):
    """Follow Android's input devices: tablet mode without a keyboard or pointer.

    This bridge runs inside the Plasma session, so kwriteconfig6 writes this
    user's kwinrc and --notify tells KWin (and Plasma) to apply it now.
    """
    if mode not in ("on", "off"):
        log(f"Ignoring invalid tablet mode: {mode!r}")
        return
    try:
        # Rare (accessory hotplug) and quick; waiting also reaps the child.
        subprocess.run(
            ["kwriteconfig6", "--file", "kwinrc", "--group", "Input",
             "--key", "TabletMode", mode, "--notify"],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, timeout=5, check=True,
        )
        log(f"Tablet mode set to {mode}")
    except Exception as e:
        log(f"Failed to set tablet mode: {e}")


def main():
    sock_fd_str = os.environ.get("WAYLAND_SOCKET")
    if not sock_fd_str:
        log("No WAYLAND_SOCKET provided in environment!")
        sys.exit(1)

    try:
        sock_fd = int(sock_fd_str)
        s = socket.fromfd(sock_fd, socket.AF_UNIX, socket.SOCK_STREAM)
    except Exception as e:
        log(f"Failed to wrap WAYLAND_SOCKET ({sock_fd_str}): {e}")
        sys.exit(1)

    log(f"portal-ime-bridge started with WAYLAND_SOCKET={sock_fd}")

    # Open commands FIFO (create if missing). Using O_RDWR ensures read never gets EOF.
    for path in (EVENTS_FIFO, COMMANDS_FIFO, LEGACY_FIFO):
        try:
            if not os.path.exists(path):
                os.mkfifo(path, 0o666)
            os.chmod(path, 0o666)
        except Exception:
            pass

    cmd_fd = None
    try:
        cmd_fd = os.open(COMMANDS_FIFO, os.O_RDWR | os.O_NONBLOCK)
        log(f"Opened {COMMANDS_FIFO} fd={cmd_fd}")
    except Exception as e:
        log(f"Failed to open {COMMANDS_FIFO}: {e}")

    # 1. Send get_registry on wl_display (id=1, opcode=1, size=12, new_id=2)
    s.sendall(struct.pack("<III", 1, (12 << 16) | 1, 2))
    # 2. Send sync on wl_display (id=1, opcode=0, size=12, new_id=3)
    s.sendall(struct.pack("<III", 1, (12 << 16) | 0, 3))
    log("Sent get_registry(id=2) and sync(id=3)")

    im_global_name = None
    im_obj_id = 4
    active_context_id = None
    latest_serial = 0
    current_preedit = ""
    last_key_time = 0.0
    wayland_buf = b""
    cmd_buf = b""

    def settle():
        wait = last_key_time + KEY_SETTLE_S - time.monotonic()
        if wait > 0:
            time.sleep(wait)

    def wl_string(value: str) -> bytes:
        utf8_bytes = value.encode("utf-8")
        str_len = len(utf8_bytes) + 1  # include null terminator
        pad_len = ((str_len + 3) // 4) * 4
        return struct.pack("<I", str_len) + utf8_bytes + b"\0" * (pad_len - len(utf8_bytes))

    def send_preedit(text: str):
        # The word Android is still composing. commit=text makes KWin commit it
        # if focus moves or the user taps into the window before Android does.
        nonlocal current_preedit
        if active_context_id is None:
            return
        settle()
        try:
            # Opcode 4: preedit_cursor(index) applies to the next preedit_string.
            body = struct.pack("<i", len(text.encode("utf-8")))
            s.sendall(struct.pack("<II", active_context_id, ((8 + len(body)) << 16) | 4) + body)
            # Opcode 2: preedit_string(serial, text, commit)
            body = struct.pack("<I", latest_serial) + wl_string(text) + wl_string(text)
            s.sendall(struct.pack("<II", active_context_id, ((8 + len(body)) << 16) | 2) + body)
            current_preedit = text
            log(f"Sent preedit_string(text={text!r})")
        except Exception as e:
            log(f"Failed to send preedit_string: {e}")

    def send_commit_string(text: str):
        nonlocal active_context_id, latest_serial, s, current_preedit
        if active_context_id is None:
            log(f"Warning: commit_string requested with no active context (text={text!r})")
            return
        settle()
        try:
            utf8_bytes = text.encode("utf-8")
            str_len = len(utf8_bytes) + 1  # include null terminator
            pad_len = ((str_len + 3) // 4) * 4
            padded_str = utf8_bytes + b"\0" * (pad_len - len(utf8_bytes))
            body = struct.pack("<II", latest_serial, str_len) + padded_str
            req_size = 8 + len(body)
            # Opcode 1 on active_context_id: commit_string(serial, text)
            msg = struct.pack("<II", active_context_id, (req_size << 16) | 1) + body
            s.sendall(msg)
            # KWin replaces the preedit with the commit in one text-input frame.
            current_preedit = ""
            log(f"Sent commit_string(serial={latest_serial}, text={text!r}, bytes={len(utf8_bytes)})")
        except Exception as e:
            log(f"Failed to send commit_string: {e}")

    def send_keysym(sym: int):
        nonlocal active_context_id, latest_serial, s, last_key_time
        if active_context_id is None:
            log(f"Warning: keysym requested with no active context (sym={hex(sym)})")
            return
        try:
            now_ms = int(time.time() * 1000) & 0xFFFFFFFF
            # state = 1 (pressed)
            k_body_press = struct.pack("<IIIII", latest_serial, now_ms, sym, 1, 0)
            k_msg_press = struct.pack("<II", active_context_id, (28 << 16) | 8) + k_body_press
            s.sendall(k_msg_press)
            # state = 0 (released)
            k_body_rel = struct.pack("<IIIII", latest_serial, (now_ms + 10) & 0xFFFFFFFF, sym, 0, 0)
            k_msg_rel = struct.pack("<II", active_context_id, (28 << 16) | 8) + k_body_rel
            s.sendall(k_msg_rel)
            last_key_time = time.monotonic()
            log(f"Sent keysym({hex(sym)}) Pressed+Released via input_method_context")
        except Exception as e:
            log(f"Failed to send keysym {hex(sym)}: {e}")

    def send_enter():
        # XKB_KEY_Return is 0xff0d
        send_keysym(0xff0d)

    def send_delete(count: int):
        if active_context_id is None:
            log(f"Warning: delete requested with no active context (count={count})")
            return
        if current_preedit:
            send_preedit("")
        for _ in range(count):
            send_keysym(0xff08)  # XKB_KEY_BackSpace

    def process_command(line: str):
        line = line.strip()
        if not line:
            return
        log(f"Processing command: {line[:60]}")
        if line == "ENTER" or line.startswith("ENTER:"):
            send_enter()
        elif line.startswith("COMMIT:"):
            b64_data = line[7:]
            try:
                raw_bytes = base64.b64decode(b64_data)
                text = raw_bytes.decode("utf-8", errors="ignore")
                if text == "\n" or text == "\r\n":
                    send_enter()
                else:
                    send_commit_string(text)
            except Exception as e:
                log(f"Failed to decode COMMIT payload: {e}")
        elif line.startswith("DELETE:"):
            try:
                count = int(line[7:])
            except Exception:
                count = 1
            send_delete(count)
        elif line.startswith("PREEDIT:"):
            try:
                send_preedit(base64.b64decode(line[8:]).decode("utf-8", errors="ignore"))
            except Exception as e:
                log(f"Failed to decode PREEDIT payload: {e}")
        elif line == "FLUSH":
            # Android dropped its composing state while the field kept focus.
            if current_preedit:
                send_commit_string(current_preedit)
        elif line.startswith("TABLET_MODE:"):
            set_tablet_mode(line[12:])
        else:
            log(f"Unknown command: {line}")

    poll_fds = [s]
    if cmd_fd is not None:
        poll_fds.append(cmd_fd)

    while True:
        try:
            rlist, _, _ = select.select(poll_fds, [], [])
        except Exception as e:
            log(f"select error: {e}")
            break

        # Handle Portal command FIFO
        if cmd_fd is not None and cmd_fd in rlist:
            try:
                chunk = os.read(cmd_fd, 4096)
                if chunk:
                    cmd_buf += chunk
                    while b"\n" in cmd_buf:
                        raw_line, cmd_buf = cmd_buf.split(b"\n", 1)
                        process_command(raw_line.decode("utf-8", errors="ignore"))
            except Exception as e:
                log(f"cmd_fd read error: {e}")

        # Handle Wayland events from KWin
        if s in rlist:
            try:
                data = s.recv(4096)
            except Exception as e:
                log(f"recv error: {e}")
                break

            if not data:
                log("EOF on WAYLAND_SOCKET from KWin")
                break

            wayland_buf += data

            while len(wayland_buf) >= 8:
                obj_id, size_opcode = struct.unpack("<II", wayland_buf[:8])
                size = size_opcode >> 16
                opcode = size_opcode & 0xFFFF
                if len(wayland_buf) < size:
                    break
                msg_body = wayland_buf[8:size]
                wayland_buf = wayland_buf[size:]

                if obj_id == 2:  # wl_registry
                    if opcode == 0:  # global(name, interface, version)
                        name = struct.unpack("<I", msg_body[:4])[0]
                        str_len = struct.unpack("<I", msg_body[4:8])[0]
                        pad_len = ((str_len + 3) // 4) * 4
                        iface_name = msg_body[8 : 8 + str_len - 1].decode("utf-8", errors="ignore")
                        version = struct.unpack("<I", msg_body[8 + pad_len : 12 + pad_len])[0]
                        if iface_name == "zwp_input_method_v1":
                            im_global_name = name
                            log(f"Found zwp_input_method_v1 name={name} version={version}")
                elif obj_id == 3:  # wl_callback
                    if opcode == 0:  # done
                        log("Sync completed by KWin")
                        if im_global_name is not None:
                            iface_bytes = b"zwp_input_method_v1\0"
                            pad_bytes = b"\0" * (((len(iface_bytes) + 3) // 4) * 4 - len(iface_bytes))
                            body = (
                                struct.pack("<II", im_global_name, len(iface_bytes))
                                + iface_bytes
                                + pad_bytes
                                + struct.pack("<II", 1, im_obj_id)
                            )
                            req_size = 8 + len(body)
                            s.sendall(struct.pack("<II", 2, (req_size << 16) | 0) + body)
                            log(f"Sent bind zwp_input_method_v1 obj_id={im_obj_id}")
                        else:
                            log("zwp_input_method_v1 was not advertised by KWin!")
                elif obj_id == im_obj_id:
                    if opcode == 0:  # activate(new_id<zwp_input_method_context_v1>)
                        context_id = struct.unpack("<I", msg_body[:4])[0]
                        active_context_id = context_id
                        log(f">>> KWIN EVENT: ACTIVATE context_id={context_id} <<<")
                        notify_portal(True)
                    elif opcode == 1:  # deactivate(object<zwp_input_method_context_v1>)
                        context_id = struct.unpack("<I", msg_body[:4])[0]
                        log(f">>> KWIN EVENT: DEACTIVATE context_id={context_id} <<<")
                        active_context_id = None
                        # KWin commits a pending preedit itself on focus changes.
                        current_preedit = ""
                        notify_portal(False)
                elif active_context_id is not None and obj_id == active_context_id:
                    # zwp_input_method_context_v1 events:
                    # 0: surrounding_text(text, cursor, anchor)
                    # 1: reset
                    # 2: content_type(hint, purpose)
                    # 3: invoke_action(button, index)
                    # 4: commit_state(serial)
                    # 5: preferred_language(language)
                    if opcode == 1:  # reset: KWin committed or dropped the preedit
                        current_preedit = ""
                    elif opcode == 4:  # commit_state
                        latest_serial = struct.unpack("<I", msg_body[:4])[0]
                        log(f"Updated latest_serial={latest_serial}")


if __name__ == "__main__":
    main()
