#!/usr/bin/python3
"""Install Valve's native ARM64 Linux Steam client.

Fetches the linuxarm64 client manifest from Valve's update CDN, downloads the
packages an ARM64 host runs (steamrtarm64/ plus shared resources), checks each
SHA-256 against the manifest and unpacks them into the Steam root. The
manifest's bootstrapper package is the i386 stub, so it is not used; the
client keeps itself updated from the same channel afterwards.

Usage: portal-steam-bootstrap.py [--root DIR] [--channel stable|publicbeta]
Progress lines on stdout: "PROGRESS <done> <total>" then "DONE <version>".
"""

import argparse
import hashlib
import io
import os
import sys
import tempfile
import urllib.request
import zipfile

CDN = "https://client-update.steamstatic.com/"
MANIFESTS = {
    "stable": "steam_client_linuxarm64",
    "publicbeta": "steam_client_publicbeta_linuxarm64",
}
PLATFORM = "linuxarm64"
USER_AGENT = "Valve/Steam HTTP Client 1.0"


def parse_vdf(text):
    """Parse Valve KeyValues text into nested dicts (strings and blocks only)."""
    tokens = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
        elif c == "/" and text.startswith("//", i):
            i = text.find("\n", i)
            i = n if i < 0 else i
        elif c in "{}":
            tokens.append(c)
            i += 1
        elif c == '"':
            j = i + 1
            out = []
            while j < n and text[j] != '"':
                if text[j] == "\\" and j + 1 < n:
                    j += 1
                out.append(text[j])
                j += 1
            tokens.append(("s", "".join(out)))
            i = j + 1
        else:
            raise ValueError(f"unexpected character {c!r} at {i}")

    def block(pos):
        result = {}
        while pos < len(tokens):
            tok = tokens[pos]
            if tok == "}":
                return result, pos + 1
            if not isinstance(tok, tuple):
                raise ValueError("expected key")
            key = tok[1]
            nxt = tokens[pos + 1]
            if nxt == "{":
                value, pos = block(pos + 2)
            elif isinstance(nxt, tuple):
                value, pos = nxt[1], pos + 2
            else:
                raise ValueError("expected value")
            result[key] = value
        return result, pos

    parsed, _ = block(0)
    return parsed


def fetch(url, base=0, overall=0):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        buf = io.BytesIO()
        done = 0
        while True:
            chunk = response.read(1 << 16)
            if not chunk:
                break
            buf.write(chunk)
            done += len(chunk)
            if overall:
                print(f"PROGRESS {base + done} {overall}", flush=True)
        return buf.getvalue()


SEED_PACKAGE = f"bins_{PLATFORM}_{PLATFORM}"


def wanted(name, entry, full):
    """Packages for a native ARM64 host: shared resources plus the
    *_linuxarm64_linuxarm64 set (steamrtarm64/). The plain *_linuxarm64 and
    *_steamrt_linuxarm64 packages are the x86 client (ubuntu12_32, steamrt64).

    By default only the ARM64 client binaries are seeded: the client's own
    updater then installs everything else (it re-fetches the full set either
    way, so pre-downloading it would only double the download)."""
    if not isinstance(entry, dict) or "file" not in entry:
        return False
    if not full:
        return name == SEED_PACKAGE
    return name.endswith("_all") or name.endswith(f"_{PLATFORM}_{PLATFORM}")


def is_executable(data):
    return data[:4] == b"ELF" or data[:2] == b"#!"


def install(root, channel, full=False):
    manifest_text = fetch(CDN + MANIFESTS[channel]).decode("utf-8")
    platform = parse_vdf(manifest_text).get(PLATFORM)
    if not isinstance(platform, dict):
        raise SystemExit(f"manifest has no {PLATFORM} section")
    packages = [(name, entry) for name, entry in platform.items() if wanted(name, entry, full)]
    if not any(name.startswith("bins_") for name, _ in packages):
        raise SystemExit("manifest has no ARM64 client binaries")
    total = sum(int(entry["size"]) for _, entry in packages)
    os.makedirs(root, exist_ok=True)
    staging = tempfile.mkdtemp(prefix=".portal-install-", dir=root)
    done = 0
    for name, entry in packages:
        data = fetch(CDN + entry["file"], done, total)
        done += len(data)
        if len(data) != int(entry["size"]):
            raise SystemExit(f"{name}: size mismatch")
        if hashlib.sha256(data).hexdigest() != entry["sha2"].lower():
            raise SystemExit(f"{name}: SHA-256 mismatch")
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            for info in archive.infolist():
                # Some of Valve's zips store Windows separators.
                name_in_zip = os.path.normpath(info.filename.replace("\\", "/"))
                if name_in_zip.startswith("..") or os.path.isabs(name_in_zip):
                    raise SystemExit(f"unsafe path in {name}: {info.filename}")
                target = os.path.join(staging, name_in_zip)
                mode = (info.external_attr >> 16) & 0o170777
                if info.filename.endswith(("/", "\\")):
                    os.makedirs(target, exist_ok=True)
                    continue
                os.makedirs(os.path.dirname(target), exist_ok=True)
                body = archive.read(info)
                if mode & 0o170000 == 0o120000:
                    if os.path.lexists(target):
                        os.unlink(target)
                    os.symlink(body.decode("utf-8"), target)
                    continue
                with open(target, "wb") as out:
                    out.write(body)
                os.chmod(target, 0o755 if is_executable(body) else 0o644)
    # Move into place only after every package verified.
    for dirpath, dirnames, filenames in os.walk(staging):
        rel = os.path.relpath(dirpath, staging)
        target_dir = os.path.normpath(os.path.join(root, rel))
        os.makedirs(target_dir, exist_ok=True)
        for filename in filenames:
            os.replace(os.path.join(dirpath, filename), os.path.join(target_dir, filename))
    for dirpath, dirnames, filenames in os.walk(staging, topdown=False):
        os.rmdir(dirpath)
    if full:
        # Tells the client's updater the whole set is in place. A seed must
        # not claim that, or the updater would skip the rest of the client.
        package_dir = os.path.join(root, "package")
        os.makedirs(package_dir, exist_ok=True)
        with open(os.path.join(package_dir, MANIFESTS[channel] + ".installed"), "w", encoding="utf-8") as out:
            out.write(manifest_text)
    print(f"DONE {platform.get('version', '?')}", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default=os.path.expanduser("~/.local/share/Steam"))
    parser.add_argument("--channel", choices=sorted(MANIFESTS), default="stable")
    parser.add_argument("--full", action="store_true", help="download every ARM64 package")
    args = parser.parse_args()
    install(args.root, args.channel, args.full)


if __name__ == "__main__":
    sys.exit(main())
