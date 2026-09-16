"""Regenerate Forky 24.1.13 XWayland patches from the pristine tree.

Applies the 5 patch files one at a time to target/xwayland-forky-src/pristine/xwayland-24.1.13,
committing after each, then emits git diffs as the rebased patch files under
patches/xwayland/forky-24.1.13/ plus a manifest of SHA-256 hashes.
"""
import os
import pathlib
import subprocess
import hashlib

REPO = pathlib.Path(__file__).resolve().parent.parent
PATCHES = [
    "target/lfdevs-xwayland-src/debian/patches/0001-xwayland-support-kgsl-dmabuf-v3-fallback-paths.patch",
    "target/lfdevs-xwayland-src/debian/patches/0002-xwayland-glamor-add-KGSL-surfaceless-backend-path.patch",
    "target/lfdevs-xwayland-src/debian/patches/0003-xwayland-dri3-support-KGSL-surfaceless-client-render.patch",
    "target/lfdevs-xwayland-src/debian/patches/0004-xwayland-scope-KGSL-frame-callback-recovery-per-window.patch",
    "patches/xwayland/0005-xwayland-preserve-finger-axis-source.patch",
]
OUT = REPO / "patches" / "xwayland" / "forky-24.1.13"
WORK = REPO / "target" / "xwayland-forky-src" / "pristine" / "xwayland-24.1.13"

ENV = dict(os.environ, PATH=r"C:\Program Files\Git\usr\bin;" + os.environ["PATH"])


def run(args, **kw):
    r = subprocess.run(args, cwd=WORK, capture_output=True, text=True, env=ENV, **kw)
    if r.returncode != 0:
        raise RuntimeError(f"{args} failed: {r.stdout[-2000:]} {r.stderr[-2000:]}")
    return r


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    # Start from the pristine commit (reset any previous run).
    root = run(["git", "rev-list", "--max-parents=0", "HEAD"]).stdout.strip().splitlines()[0]
    run(["git", "reset", "-q", "--hard", root])
    run(["git", "clean", "-fdq"])
    status = run(["git", "status", "--porcelain"]).stdout
    # Find pristine commit: the first commit.
    log = run(["git", "log", "--oneline"]).stdout.strip().splitlines()
    print(f"{len(log)} commits; status bytes={len(status)}")
    names = []
    for patch in PATCHES:
        src = REPO / patch
        name = src.name
        names.append(name)
        run(["patch", "-p1", "-s", "-V", "none", "-i", str(src.resolve())])
        rej = list(WORK.rglob("*.rej"))
        if rej:
            raise RuntimeError(f"rejects applying {name}: {rej}")
        run(["git", "add", "-A"])
        run(["git", "-c", "user.email=t", "-c", "user.name=t", "commit", "-qm", f"forky: {name}"])
        print(f"applied+committed {name}")
    # Emit one diff per patch commit (oldest first).
    commits = run(["git", "log", "--format=%H", "--reverse"]).stdout.strip().splitlines()
    # commits[0] is pristine; commits[1..6] are the patch commits.
    assert len(commits) == 6, commits
    for commit, name in zip(commits[1:], names):
        diff = run(["git", "show", "--format=", "--no-ext-diff", commit]).stdout
        # Normalize to LF.
        diff = diff.replace("\r\n", "\n")
        (OUT / name).write_text(diff, encoding="utf-8", newline="\n")
        print(f"wrote {name} ({len(diff)} bytes)")
    # Hash manifest.
    lines = []
    for name in names:
        h = hashlib.sha256((OUT / name).read_bytes()).hexdigest()
        lines.append(f"{h}  {name}")
        print(h, name)
    (OUT / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
