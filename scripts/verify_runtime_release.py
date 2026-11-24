#!/usr/bin/env python3
"""Fail the APK release build if its pinned public runtime asset is unavailable."""
import json
import os
import urllib.request
from pathlib import Path

def verify():
    manifest = json.loads((Path(__file__).resolve().parent.parent / "assets/debian-runtime.json").read_text())
    tag = "runtime-" + manifest["version"]
    prefix = f"https://github.com/Retrorerr/Portal/releases/download/{tag}/"
    if not manifest["url"].startswith(prefix):
        raise RuntimeError("Runtime must be a versioned Portal release asset")
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "Portal-release-verifier"}
    # Anonymous API calls share a small per-IP limit that CI runners exhaust.
    if os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    request = urllib.request.Request(f"https://api.github.com/repos/Retrorerr/Portal/releases/tags/{tag}",
        headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        release = json.load(response)
    asset = next(a for a in release["assets"] if a["browser_download_url"] == manifest["url"])
    if asset["size"] != manifest["compressed_bytes"] or asset.get("digest") != "sha256:" + manifest["sha256"]:
        raise RuntimeError("Published runtime digest or size differs from APK manifest")
    with urllib.request.urlopen(urllib.request.Request(manifest["url"], method="HEAD"), timeout=30) as response:
        if response.status != 200 or int(response.headers["Content-Length"]) != manifest["compressed_bytes"]:
            raise RuntimeError("Public runtime download failed")
    print(f"Verified public runtime {manifest['version']} ({asset['size']} bytes, SHA-256 {manifest['sha256']})")

if __name__ == "__main__":
    verify()
