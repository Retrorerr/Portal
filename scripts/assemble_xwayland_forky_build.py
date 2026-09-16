"""Assemble the Forky XWayland 24.1.13+KGSL build tree.

Copies the pristine 24.1.13 source, installs the 5 rebased patches into
debian/patches with a series file, adds the -1portal1 changelog entry, and
tars the tree for the on-device guest build. Prints hashes for provenance.
"""
import hashlib
import pathlib
import shutil
import tarfile

REPO = pathlib.Path(__file__).resolve().parent.parent
SRC = REPO / "target" / "xwayland-forky-src" / "pristine" / "xwayland-24.1.13"
PATCHDIR = REPO / "patches" / "xwayland" / "forky-24.1.13"
BUILD = REPO / "target" / "xwayland-forky-src" / "build-tree"
TARBALL = REPO / "target" / "xwayland-forky-24.1.13-portal1.tar"

PATCHES = [
    "0001-xwayland-support-kgsl-dmabuf-v3-fallback-paths.patch",
    "0002-xwayland-glamor-add-KGSL-surfaceless-backend-path.patch",
    "0003-xwayland-dri3-support-KGSL-surfaceless-client-render.patch",
    "0004-xwayland-scope-KGSL-frame-callback-recovery-per-window.patch",
    "0005-xwayland-preserve-finger-axis-source.patch",
]

CHANGELOG_ENTRY = """xwayland (2:24.1.13-1portal1) forky; urgency=medium

  * Portal KGSL surfaceless forward-port (Anland/PRoot, no /dev/dri node).
  * 0001-0004: lfdevs KGSL dmabuf/glamor/DRI3/frame-callback work rebased
    from 24.1.6-91 (lfdevs/xwayland @ 461772ae) onto exact 24.1.13 source.
  * 0005: preserve WL_POINTER_AXIS_SOURCE_FINGER into XI2 (touchpad kinetic
    scrolling; same semantics as the audited 24.1.6 candidate patch).
  * Never -92 (reserved for lfdevs) and never -2 (reserved for Debian).

 -- Portal Build <portal@localhost>  Tue, 16 Sep 2026 00:00:00 +0000

"""


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    for name in PATCHES:
        assert (PATCHDIR / name).is_file(), name
    if BUILD.exists():
        shutil.rmtree(BUILD)
    shutil.copytree(SRC, BUILD, ignore=shutil.ignore_patterns(".git"))
    assert not (BUILD / ".git").exists()
    dest = BUILD / "debian" / "patches"
    dest.mkdir(exist_ok=True)
    for name in PATCHES:
        shutil.copyfile(PATCHDIR / name, dest / name)
    (dest / "series").write_text("\n".join(PATCHES) + "\n", encoding="utf-8", newline="\n")
    old = (BUILD / "debian" / "changelog").read_text(encoding="utf-8")
    assert old.startswith("xwayland (2:24.1.13-1)"), old[:60]
    (BUILD / "debian" / "changelog").write_text(CHANGELOG_ENTRY + old, encoding="utf-8", newline="\n")
    if TARBALL.exists():
        TARBALL.unlink()
    with tarfile.open(TARBALL, "w") as tar:
        tar.add(BUILD, arcname="xwayland-24.1.13")
    print(f"tarball={TARBALL} bytes={TARBALL.stat().st_size} sha256={sha256(TARBALL)}")
    for name in PATCHES:
        print(f"patch {sha256(PATCHDIR / name)}  {name}")


if __name__ == "__main__":
    main()
