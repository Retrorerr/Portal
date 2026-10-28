#!/usr/bin/python3
"""Portal's guest-side file boundary. Binary data never enters the JSON protocol."""

import base64
import ctypes
import errno
import json
import os
import re
import signal
import stat
import sys
import time
import uuid

CHUNK = 256 * 1024


def entry_id(path):
    return base64.urlsafe_b64encode(os.fsencode(path)).decode("ascii")


class Files:
    def __init__(self, home=None):
        self.home = os.path.realpath(home or os.path.expanduser("~"))

    def resolve(self, identifier):
        relative = os.fsdecode(base64.b64decode(identifier, altchars=b"-_", validate=True))
        if os.path.isabs(relative):
            raise ValueError("Choose a file inside your Linux home")
        path = os.path.realpath(os.path.join(self.home, relative))
        if os.path.commonpath((self.home, path)) != self.home:
            raise ValueError("This link points outside your Linux home")
        return path

    def directory(self, identifier):
        # Resolve in the guest, then walk without following further links. Pin
        # every ancestor so a concurrent rename cannot redirect a transfer.
        path = self.resolve(identifier)
        fd = os.open(self.home, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            relative = os.path.relpath(path, self.home)
            for part in (() if relative == "." else relative.split(os.sep)):
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                os.close(fd)
                fd = child
            return fd, os.path.relpath(path, self.home)
        except BaseException:
            os.close(fd)
            raise

    def defaults(self):
        download = os.path.join(self.home, "Downloads")
        # Read XDG's data format, never source a user-controlled shell file.
        try:
            with open(os.path.join(self.home, ".config/user-dirs.dirs")) as source:
                for line in source:
                    match = re.fullmatch(r'XDG_DOWNLOAD_DIR="([^"`\\]*)"\s*', line.strip())
                    if match:
                        candidate = match[1].replace("$HOME", self.home)
                        if "$" not in candidate:
                            download = candidate
        except OSError:
            pass
        for candidate in (download, os.path.join(self.home, "Downloads"), self.home):
            identifier = entry_id(os.path.relpath(candidate, self.home))
            try:
                self.resolve(identifier)
                os.makedirs(candidate, exist_ok=True)
                return {"home": entry_id("."), "downloads": identifier,
                        "label": "Home" if os.path.realpath(candidate) == self.home else os.path.basename(candidate)}
            except (ValueError, OSError):
                continue
        raise OSError("Your Linux home is unavailable")

    def listing(self, request):
        identifier = request.get("directory", entry_id("."))
        fd, relative = self.directory(identifier)
        try:
            entries = []
            with os.scandir(fd) as scan:
                for item in scan:
                    if item.name.startswith(".portal-transfer-"):
                        continue
                    if not request.get("hidden", False) and item.name.startswith("."):
                        continue
                    child_id = entry_id(os.path.join(relative, item.name))
                    try:
                        resolved = self.resolve(child_id)
                        info = os.stat(resolved)
                        if not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)):
                            continue
                    except (OSError, ValueError):
                        continue
                    entries.append({"id": child_id, "name": os.fsencode(item.name).decode("utf-8", "replace"),
                                    "directory": stat.S_ISDIR(info.st_mode), "size": info.st_size, "modified": info.st_mtime_ns})
            entries.sort(key=lambda item: (not item["directory"], item["name"].casefold()))
            offset = max(0, int(request.get("offset", 0)))
            return {"ok": True, "directory": entry_id(relative), "name": os.fsencode(relative).decode("utf-8", "replace"),
                    "parent": entry_id(os.path.dirname(relative) or "."),
                    "entries": entries[offset:offset + 200], "more": offset + 200 < len(entries)}
        finally:
            os.close(fd)

    def export(self, request, target):
        path = self.resolve(request["entry"])
        parent, _ = self.directory(entry_id(os.path.relpath(os.path.dirname(path), self.home)))
        try:
            fd = os.open(os.path.basename(path), os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
            with os.fdopen(fd, "rb") as source:
                info = os.fstat(source.fileno())
                if not stat.S_ISREG(info.st_mode):
                    raise ValueError("Only regular files can be transferred")
                count = copy(source, target, info.st_size)
                after = os.fstat(source.fileno())
                if count != info.st_size or (after.st_size, after.st_mtime_ns) != (info.st_size, info.st_mtime_ns):
                    raise ValueError("The Linux file changed while copying. Refresh Files and try again.")
            return {"ok": True, "size": info.st_size, "modified": info.st_mtime_ns}
        finally:
            os.close(parent)

    def cleanup(self, request):
        fd, _ = self.directory(request["directory"])
        try:
            try:
                os.unlink(temporary_name(request["job"]), dir_fd=fd)
            except FileNotFoundError:
                pass
            return {"ok": True}
        finally:
            os.close(fd)

    def import_file(self, request, source):
        fd, _ = self.directory(request["directory"])
        temporary = temporary_name(request["job"])
        created = False
        try:
            output_fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=fd)
            created = True
            with os.fdopen(output_fd, "wb") as target:
                expected = request.get("size", -1)
                count = copy(source, target, expected)
                if expected >= 0 and count != expected:
                    raise ValueError("The Android file ended before its reported size. Choose it again.")
                target.flush()
                os.fsync(target.fileno())
            if request.get("defer_publish", False):
                # Android checks the reliable-pipe status and closes its
                # provider descriptor before asking us to publish this file.
                created = False
                return {"ok": True, "staged": True}
            return finish_import(fd, temporary, request.get("name", "file"))
        finally:
            if created:
                try:
                    os.unlink(temporary, dir_fd=fd)
                except FileNotFoundError:
                    pass
            os.close(fd)

    def commit(self, request):
        fd, _ = self.directory(request["directory"])
        temporary = temporary_name(request["job"])
        try:
            info = os.stat(temporary, dir_fd=fd, follow_symlinks=False)
            if not stat.S_ISREG(info.st_mode):
                raise ValueError("This transfer is no longer available")
            return finish_import(fd, temporary, request.get("name", "file"))
        finally:
            try:
                os.unlink(temporary, dir_fd=fd)
            except FileNotFoundError:
                pass
            os.close(fd)


def finish_import(directory, temporary, name):
    stem, extension = os.path.splitext(safe_name(name))
    index = 0
    while True:
        suffix = f" ({index})" if index else ""
        budget = 240 - len((extension + suffix).encode("utf-8"))
        candidate = stem.encode("utf-8")[:budget].decode("utf-8", "ignore") + suffix + extension
        try:
            publish(directory, temporary, candidate)
            break
        except FileExistsError:
            index += 1
    os.fsync(directory)
    return {"ok": True, "name": candidate}


def temporary_name(job):
    return ".portal-transfer-" + str(uuid.UUID(job)) + ".part"


def safe_name(name):
    name = name.replace("/", "_").replace("\\", "_").replace("\0", "_")
    if name in ("", ".", ".."):
        name = "file"
    stem, extension = os.path.splitext(name)
    extension = extension.encode("utf-8")[:64].decode("utf-8", "ignore")
    stem = stem.encode("utf-8")[:240 - len(extension.encode("utf-8"))].decode("utf-8", "ignore")
    return stem + extension


def publish(directory, temporary, name):
    libc = ctypes.CDLL(None, use_errno=True)
    rename = getattr(libc, "renameat2", None)
    if rename is not None:
        if rename(directory, os.fsencode(temporary), directory, os.fsencode(name), 1) == 0:
            return
        error = ctypes.get_errno()
        if error not in (errno.ENOSYS, errno.EINVAL, errno.EOPNOTSUPP):
            raise OSError(error, os.strerror(error))
    # Both guest engines emulate hardlinks. This also publishes without
    # replacing an existing destination on older kernels.
    os.link(temporary, name, src_dir_fd=directory, dst_dir_fd=directory, follow_symlinks=False)
    os.unlink(temporary, dir_fd=directory)


def copy(source, target, total):
    count, last = 0, 0.0
    while True:
        data = source.read(CHUNK)
        if not data:
            break
        target.write(data)
        count += len(data)
        now = time.monotonic()
        if now - last >= 0.1:
            progress(count, total)
            last = now
    target.flush()
    progress(count, total)
    return count


def progress(count, total):
    print(json.dumps({"bytes": count, "total": total}), file=sys.stderr, flush=True)


def interrupted(_signal, _frame):
    raise InterruptedError("Transfer canceled")


def main():
    signal.signal(signal.SIGTERM, interrupted)
    request = json.loads(os.environ["PORTAL_FILE_REQUEST"])
    files = Files()
    action = request["action"]
    if action == "defaults":
        result = {"ok": True, **files.defaults()}
    elif action == "list":
        result = files.listing(request)
    elif action == "import":
        result = files.import_file(request, sys.stdin.buffer)
    elif action == "commit":
        result = files.commit(request)
    elif action == "export":
        result = files.export(request, sys.stdout.buffer)
        print(json.dumps(result), file=sys.stderr, flush=True)
        return  # stdout belongs entirely to the binary stream.
    elif action == "cleanup":
        result = files.cleanup(request)
    else:
        raise ValueError("Unknown file operation")
    print(json.dumps(result, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=True), file=sys.stderr, flush=True)
        sys.exit(1)
