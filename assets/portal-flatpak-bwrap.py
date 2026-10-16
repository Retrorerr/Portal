#!/usr/bin/python3 -I
# Flatpak's bubblewrap, adapted to Android (FLATPAK_BWRAP points here).
#
# Two things differ from a Linux PC, and both break `--device=dri` apps
# (every Flatpak that asks for the GPU but not for all devices):
#
# - SELinux lets access() see some host device nodes that stat() may not
#   (/dev/udmabuf). Flatpak binds a node whenever access() finds it, and
#   bwrap then dies on "Can't get type of source ...: Permission denied".
#   A node the guest can't even stat is no use to the app: its bind is
#   dropped.
# - The GPU is /dev/kgsl-3d0. Portal's /dev/dri/renderD128 is a symlink to
#   it, so a sandbox that only gets /dev/dri finds a dangling node. It gets
#   the kgsl node too, next to /dev/dri, and the DMA heaps (ION on older
#   kernels) Mesa's kgsl winsys allocates shared buffers from.
#
# Flatpak passes its arguments through `--args FD`; those are read here and
# handed on in a new file. Arguments this does not understand go to bwrap
# unchanged.
import errno
import os
import sys

BWRAP = "/usr/bin/bwrap"
GPU_NODES = (b"/dev/kgsl-3d0", b"/dev/dma_heap", b"/dev/ion")

# bwrap 0.11 options and how many arguments each takes.
ARITY = {
    b"--args": 1, b"--argv0": 1, b"--level-prefix": 0,
    b"--unshare-all": 0, b"--share-net": 0, b"--unshare-user": 0,
    b"--unshare-user-try": 0, b"--unshare-ipc": 0, b"--unshare-pid": 0,
    b"--unshare-net": 0, b"--unshare-uts": 0, b"--unshare-cgroup": 0,
    b"--unshare-cgroup-try": 0, b"--userns": 1, b"--userns2": 1,
    b"--disable-userns": 0, b"--assert-userns-disabled": 0, b"--pidns": 1,
    b"--uid": 1, b"--gid": 1, b"--hostname": 1, b"--chdir": 1,
    b"--clearenv": 0, b"--setenv": 2, b"--unsetenv": 1, b"--lock-file": 1,
    b"--sync-fd": 1, b"--bind": 2, b"--bind-try": 2, b"--dev-bind": 2,
    b"--dev-bind-try": 2, b"--ro-bind": 2, b"--ro-bind-try": 2,
    b"--bind-fd": 2, b"--ro-bind-fd": 2, b"--remount-ro": 1,
    b"--overlay-src": 1, b"--overlay": 3, b"--tmp-overlay": 1,
    b"--ro-overlay": 1, b"--exec-label": 1, b"--file-label": 1,
    b"--proc": 1, b"--dev": 1, b"--tmpfs": 1, b"--mqueue": 1, b"--dir": 1,
    b"--file": 2, b"--bind-data": 2, b"--ro-bind-data": 2, b"--symlink": 2,
    b"--seccomp": 1, b"--add-seccomp-fd": 1, b"--block-fd": 1,
    b"--userns-block-fd": 1, b"--info-fd": 1, b"--json-status-fd": 1,
    b"--new-session": 0, b"--die-with-parent": 0, b"--as-pid-1": 0,
    b"--cap-add": 1, b"--cap-drop": 1, b"--perms": 1, b"--size": 1,
    b"--chmod": 2, b"--not-a-security-boundary": 0,
}
BINDS = {b"--bind", b"--bind-try", b"--dev-bind", b"--dev-bind-try",
         b"--ro-bind", b"--ro-bind-try"}


class Unknown(Exception):
    pass


ARG_FDS = []


def read_fd(fd):
    # pread leaves the offset alone, so bwrap can still read it unchanged.
    chunks, offset = [], 0
    while True:
        chunk = os.pread(fd, 1 << 16, offset)
        if not chunk:
            break
        chunks.append(chunk)
        offset += len(chunk)
    ARG_FDS.append(fd)
    data = b"".join(chunks)
    if data.endswith(b"\0"):
        data = data[:-1]
    return data.split(b"\0") if data else []


def options(argv):
    """Split argv into option groups and the command, expanding --args."""
    groups, i = [], 0
    while i < len(argv):
        arg = argv[i]
        if not arg.startswith(b"--") or arg == b"--":
            return groups, argv[i:]
        if arg not in ARITY:
            raise Unknown(arg)
        group = argv[i:i + 1 + ARITY[arg]]
        if len(group) != 1 + ARITY[arg]:
            raise Unknown(arg)
        i += len(group)
        if arg == b"--args":
            inner, command = options(read_fd(int(group[1])))
            groups.extend(inner)
            if command:
                return groups, command + argv[i:]
            continue
        groups.append(group)
    return groups, []


def hidden(path):
    try:
        os.stat(path)
    except OSError as error:
        return error.errno == errno.EACCES
    return False


def adapt(groups):
    out, changed = [], False
    whole_dev = any(g[0] == b"--dev-bind" and g[1] == b"/dev" for g in groups)
    for group in groups:
        if group[0] in BINDS and hidden(group[1]):
            changed = True
            continue
        out.append(group)
        if (group[0] == b"--dev-bind" and group[2] == b"/dev/dri"
                and not whole_dev):
            bound = {g[2] for g in groups if g[0] in BINDS}
            for node in GPU_NODES:
                if node not in bound and os.path.exists(node):
                    out.append([b"--dev-bind", node, node])
                    changed = True
    return out, changed


def main():
    try:
        groups, command = options([os.fsencode(a) for a in sys.argv[1:]])
        groups, changed = adapt(groups)
    except (Unknown, ValueError, OSError):
        changed = False
    if not changed:
        os.execv(BWRAP, [BWRAP] + sys.argv[1:])
    # The options go through one new --args file, as Flatpak's do, to stay
    # clear of the command-line size limit; bwrap reads only options there,
    # so the command stays on the command line.
    data = b"".join(arg + b"\0" for group in groups for arg in group)
    try:
        fd = os.memfd_create("bwrap-args", 0)
    except (AttributeError, OSError):
        import tempfile
        fd, path = tempfile.mkstemp()
        os.unlink(path)
    os.write(fd, data)
    os.lseek(fd, 0, os.SEEK_SET)
    os.set_inheritable(fd, True)
    for old in ARG_FDS:
        os.close(old)
    os.execv(BWRAP, [os.fsencode(BWRAP), b"--args", str(fd).encode()] + command)


main()
