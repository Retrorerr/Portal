#!/usr/bin/env python3
"""
Portal IME Bridge for KWin 6.x
Connects to KWin's private WAYLAND_SOCKET as an input method (zwp_input_method_v1).
Relays text field activate/deactivate events to /tmp/portal-ime-events.fifo.
Reads text commit and delete commands from /tmp/portal-ime-commands.fifo and
forwards them directly through zwp_input_method_context_v1 (commit_string,
delete_surrounding_text) into the focused guest Wayland client.
"""
import base64
import os
import select
import socket
import struct
import sys
import time

EVENTS_FIFO = "/tmp/portal-ime-events.fifo"
COMMANDS_FIFO = "/tmp/portal-ime-commands.fifo"
LEGACY_FIFO = "/tmp/portal-ime.fifo"
LOG_PATH = "/tmp/portal_ime.log"

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
    wayland_buf = b""
    cmd_buf = b""

    def send_commit_string(text: str):
        nonlocal active_context_id, latest_serial, s
        if active_context_id is None:
            log(f"Warning: commit_string requested with no active context (text={text!r})")
            return
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
            log(f"Sent commit_string(serial={latest_serial}, text={text!r}, bytes={len(utf8_bytes)})")
        except Exception as e:
            log(f"Failed to send commit_string: {e}")

    def send_keysym(sym: int):
        nonlocal active_context_id, latest_serial, s
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
            log(f"Sent keysym({hex(sym)}) Pressed+Released via input_method_context")
        except Exception as e:
            log(f"Failed to send keysym {hex(sym)}: {e}")

    def send_enter():
        # XKB_KEY_Return is 0xff0d
        send_keysym(0xff0d)

    def send_delete_surrounding_text(count: int):
        nonlocal active_context_id, latest_serial, s
        if active_context_id is None:
            log(f"Warning: delete requested with no active context (count={count})")
            return
        try:
            # 1. Opcode 5: delete_surrounding_text(index: int32, length: uint32)
            body = struct.pack("<ii", -count, count)
            req_size = 8 + len(body)
            msg = struct.pack("<II", active_context_id, (req_size << 16) | 5) + body
            s.sendall(msg)
            log(f"Sent delete_surrounding_text(index={-count}, length={count})")

            # 2. Opcode 8: keysym(serial, time, sym, state, modifiers) for XKB_KEY_BackSpace (0xff08)
            # This ensures clients where the text widget handles backspace via keysym (like Kate/Qt) delete reliably.
            for _ in range(count):
                now_ms = int(time.time() * 1000) & 0xFFFFFFFF
                # state = 1 (pressed)
                k_body = struct.pack("<IIIII", latest_serial, now_ms, 0xff08, 1, 0)
                k_msg = struct.pack("<II", active_context_id, (28 << 16) | 8) + k_body
                s.sendall(k_msg)
                # state = 0 (released)
                k_body_rel = struct.pack("<IIIII", latest_serial, (now_ms + 10) & 0xFFFFFFFF, 0xff08, 0, 0)
                k_msg_rel = struct.pack("<II", active_context_id, (28 << 16) | 8) + k_body_rel
                s.sendall(k_msg_rel)
                log("Sent keysym(BackSpace, 0xff08) Pressed+Released via input_method_context")
        except Exception as e:
            log(f"Failed to send delete_surrounding_text: {e}")

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
            send_delete_surrounding_text(count)
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
                        notify_portal(False)
                elif active_context_id is not None and obj_id == active_context_id:
                    # zwp_input_method_context_v1 events:
                    # 0: surrounding_text(text, cursor, anchor)
                    # 1: reset
                    # 2: content_type(hint, purpose)
                    # 3: invoke_action(button, index)
                    # 4: commit_state(serial)
                    # 5: preferred_language(language)
                    if opcode == 4:  # commit_state
                        latest_serial = struct.unpack("<I", msg_body[:4])[0]
                        log(f"Updated latest_serial={latest_serial}")


if __name__ == "__main__":
    main()
