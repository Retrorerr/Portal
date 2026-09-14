#!/usr/bin/env python3
"""
Publish or restore Portal's canonical Debian runtime release to GitHub Releases.

Durable, reproducible publishing pipeline that:
1. Validates the generated runtime archive (SHA-256, size, Debian layout,
   required desktop binaries, absence of Arch/pacman).
2. Refuses to overwrite an existing release version with different bytes.
3. Creates or updates the GitHub Release using `gh` CLI.
4. Updates or verifies `assets/debian-runtime.json`.
5. Verifies the public download URL and asset digest end-to-end.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "assets/debian-runtime.json"

REQUIRED_FILES = [
    "usr/lib/os-release",
    "usr/bin/dpkg",
    "usr/bin/apt",
    "usr/bin/bash",
    "usr/bin/kwin_wayland",
    "usr/bin/plasmashell",
    "usr/bin/ksmserver",
    "usr/bin/Xwayland",
    "usr/bin/python3",
    "var/lib/dpkg/status",
]

FORBIDDEN_FILES = [
    "usr/bin/pacman",
]

FORKY_REQUIRED_PACKAGE_PREFIXES = {
    "plasma-desktop": "4:6.7.",
    "kwin-wayland": "4:6.7.",
    "xwayland": "2:24.",
}


def compute_sha256_and_size(path: Path) -> tuple[str, int]:
    size = path.stat().st_size
    with open(path, "rb") as f:
        digest = hashlib.file_digest(f, "sha256").hexdigest()
    return digest, size


def validate_archive(archive_path: Path, expected_version: str) -> None:
    print(f"Validating archive {archive_path.name} against Debian runtime specifications...")
    if not archive_path.is_file():
        raise FileNotFoundError(f"Archive not found: {archive_path}")

    with tarfile.open(archive_path, "r:xz") as tar:
        found_members = {}
        for member in tar:
            found_members[member.name] = member

        # 1. Version marker
        version_marker = "etc/portal-runtime-version"
        if version_marker not in found_members:
            raise RuntimeError(f"Archive is missing version marker: {version_marker}")
        version_file = tar.extractfile(found_members[version_marker])
        if version_file is None:
            raise RuntimeError(f"Unable to read {version_marker} from archive")
        archived_version = version_file.read().decode("utf-8").strip()
        if archived_version != expected_version:
            raise RuntimeError(
                f"Version marker mismatch in archive: found '{archived_version}', expected '{expected_version}'"
            )

        # 2. OS release check (Debian 14 / Forky)
        os_release_name = "usr/lib/os-release"
        if os_release_name not in found_members:
            raise RuntimeError(f"Archive missing {os_release_name}")
        os_release_file = tar.extractfile(found_members[os_release_name])
        if os_release_file is None:
            raise RuntimeError(f"Unable to read {os_release_name}")
        os_release_text = os_release_file.read().decode("utf-8")
        lines = [line.strip() for line in os_release_text.splitlines()]
        has_forky_identity = any(
            line in lines for line in ('VERSION_ID="14"', 'VERSION_ID=14', 'VERSION_CODENAME=forky')
        )
        if not ("ID=debian" in lines and has_forky_identity):
            raise RuntimeError(f"{os_release_name} does not match Debian 14/Forky specifications")

        # 3. Required files
        for req in REQUIRED_FILES:
            if req not in found_members:
                raise RuntimeError(f"Archive missing required runtime entry: {req}")
            member = found_members[req]
            if req.startswith("usr/bin/"):
                if member.mode & 0o111 == 0:
                    raise RuntimeError(f"Binary {req} does not have executable permissions (mode: {oct(member.mode)})")

        # 4. Forbidden files
        for forb in FORBIDDEN_FILES:
            if forb in found_members:
                raise RuntimeError(f"Archive contains forbidden entry: {forb}")

        # 5. Current Forky desktop package versions. The Portal-specific
        # Anland binary is installed by the APK, not substituted into the
        # Debian package archive.
        validate_forky_packages(tar, found_members)

    print("Archive layout and required components successfully validated.")


def parse_dpkg_status_versions(tar: tarfile.TarFile, found_members: dict) -> dict:
    status_name = "var/lib/dpkg/status"
    if status_name not in found_members:
        raise RuntimeError("Archive is missing dpkg status for Anland validation")
    status_file = tar.extractfile(found_members[status_name])
    if status_file is None:
        raise RuntimeError("Unable to read dpkg status for Anland validation")
    versions = {}
    package = None
    for line in status_file.read().decode("utf-8", errors="replace").splitlines():
        if line.startswith("Package: "):
            package = line.split(":", 1)[1].strip()
        elif line.startswith("Version: ") and package and package not in versions:
            versions[package] = line.split(":", 1)[1].strip()
            package = None
    return versions


def validate_forky_packages(tar: tarfile.TarFile, found_members: dict) -> None:
    print("Validating Forky desktop package versions...")
    versions = parse_dpkg_status_versions(tar, found_members)
    for package, prefix in FORKY_REQUIRED_PACKAGE_PREFIXES.items():
        version = versions.get(package, "")
        if not version.startswith(prefix):
            raise RuntimeError(
                f"Forky validation failed: {package} version is {version!r}, "
                f"expected a current Forky {prefix} release"
            )
    print("Forky desktop package versions validated; Anland is supplied by Portal assets.")


class ReleaseLookupError(RuntimeError):
    """GitHub release lookup failed (not a confirmed absence). Fail closed."""


# Exact `gh` stderr observed (2026-09-12, gh on Windows) for a missing tag in
# an otherwise valid repository:
#   rc=1, stdout='', stderr='release not found\n'
# This message alone is NOT sufficient: `gh release view` prints the same
# text for an invalid repository, and generic substrings ("not found",
# "404", "could not resolve") also match repository/auth/network errors.
# Decision rule: only this exact message AND a separately validated
# repository may return None. Everything else raises ReleaseLookupError.
MISSING_RELEASE_RE = re.compile(r"^release not found$")


def validate_repo(repo: str) -> None:
    """Confirm `repo` itself exists and is reachable; raise otherwise."""
    result = subprocess.run(
        ["gh", "repo", "view", repo, "--json", "name"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ReleaseLookupError(
            f"Cannot confirm repository '{repo}' while interpreting a missing release "
            f"(exit {result.returncode}): {(result.stderr or result.stdout or '').strip()}"
        )


def get_existing_release(repo: str, tag: str) -> dict | None:
    """Return the release JSON when it exists, None on confirmed absence.

    Raises ReleaseLookupError on any other failure (invalid repository,
    auth, network, rate limit, permissions, outage) and on malformed JSON.
    Only a confirmed missing RELEASE in an otherwise valid repository may
    lead to release creation.
    """
    result = subprocess.run(
        ["gh", "release", "view", tag, "--repo", repo, "--json", "tagName,assets,isDraft"],
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        try:
            return json.loads(result.stdout)
        except json.JSONDecodeError as e:
            raise ReleaseLookupError(
                f"GitHub release lookup for '{tag}' returned malformed JSON: {e}\n"
                f"stdout={result.stdout!r} stderr={result.stderr!r}"
            )
    stderr = (result.stderr or "").strip()
    stdout = (result.stdout or "").strip()
    if stdout == "" and MISSING_RELEASE_RE.fullmatch(stderr):
        # Candidate "missing release": rule out a missing/invalid repo
        # (which prints the same text) before believing absence.
        validate_repo(repo)
        return None
    raise ReleaseLookupError(
        f"GitHub release lookup for '{tag}' failed (exit {result.returncode}): "
        f"{(result.stderr or result.stdout or '').strip()}"
    )


def get_source_commit() -> str:
    """Return the exact 40-char Portal source SHA for release provenance."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"Cannot determine source commit for release provenance: "
            f"{(result.stderr or result.stdout or '').strip()}"
        )
    sha = (result.stdout or "").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise RuntimeError(
            f"Source commit is not an exact 40-char SHA for release provenance: {sha!r}"
        )
    return sha


def check_source_clean(allow_dirty: bool = False) -> None:
    """Require a clean-enough tree for publication unless explicitly allowed."""
    result = subprocess.run(
        ["git", "status", "--porcelain"],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )
    if result.returncode != 0:
        raise RuntimeError(
            "Cannot verify source cleanliness for release provenance: "
            f"{(result.stderr or result.stdout or '').strip()}"
        )
    dirty = (result.stdout or "").strip()
    if dirty and not allow_dirty:
        raise RuntimeError(
            "Refusing to publish from a dirty working tree (use --allow-dirty "
            f"to override explicitly):\n{dirty}"
        )


def get_release_target(repo: str, tag: str) -> str | None:
    """Return the release's target commitish (SHA or branch) or None."""
    result = subprocess.run(
        ["gh", "release", "view", tag, "--repo", repo, "--json", "targetCommitish"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ReleaseLookupError(
            f"Cannot verify tag target for '{tag}' (exit {result.returncode}): "
            f"{(result.stderr or result.stdout or '').strip()}"
        )
    try:
        data = json.loads(result.stdout)
    except json.JSONDecodeError as e:
        raise ReleaseLookupError(
            f"Tag target lookup for '{tag}' returned malformed JSON: {e}"
        )
    return data.get("targetCommitish")


def verify_tag_target(repo: str, tag: str, expected_sha: str) -> None:
    """Verify an existing tag's provenance instead of assuming correctness.

    A 40-char target must equal the current source SHA; anything else is a
    mismatch and aborts (never silently retarget a published runtime tag).
    A branch target (e.g. legacy releases pinned to `main`) is left alone
    with a warning: bytes-immutability checks below still apply.
    """
    target = get_release_target(repo, tag)
    if target is None:
        raise ReleaseLookupError(
            f"Existing release '{tag}' exposes no target commitish; refusing to assume provenance"
        )
    if re.fullmatch(r"[0-9a-f]{40}", target or ""):
        if target != expected_sha:
            raise RuntimeError(
                f"Existing release tag '{tag}' targets {target}, not current source {expected_sha}; "
                "refusing to retarget a published runtime tag"
            )
        return
    print(
        f"Note: existing release '{tag}' targets branch '{target}' (predates source-pinning); "
        "leaving tag provenance untouched, enforcing byte immutability only."
    )


def check_existing_asset(remote_asset: dict, local_name: str, local_size: int, local_digest: str) -> None:
    """Fail closed when a published asset differs; require digest proof.

    Size equality alone never proves byte equality. When GitHub exposes a
    SHA-256 digest it must match exactly; when digest metadata is absent,
    reuse is refused instead of assumed identical.
    """
    remote_size = remote_asset.get("size")
    remote_digest = remote_asset.get("digest")
    expected_digest = f"sha256:{local_digest}"
    if remote_size != local_size:
        raise RuntimeError(
            f"Refusing to overwrite existing GitHub release asset '{local_name}' with different bytes!\n"
            f"  Remote: {remote_size} bytes, digest={remote_digest}\n"
            f"  Local:  {local_size} bytes, sha256={local_digest}"
        )
    if remote_digest:
        if remote_digest != expected_digest:
            raise RuntimeError(
                f"Refusing to overwrite existing GitHub release asset '{local_name}' with different bytes!\n"
                f"  Remote: {remote_size} bytes, digest={remote_digest}\n"
                f"  Local:  {local_size} bytes, sha256={local_digest}"
            )
        return
    raise RuntimeError(
        f"Existing asset '{local_name}' exposes no SHA-256 digest; size equality "
        "does not prove byte equality, refusing to reuse"
    )


def verify_public_url(url: str, expected_size: int) -> None:
    print(f"Verifying public download URL: {url}...")
    req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": "Portal-Release-Verifier"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        if resp.status != 200:
            raise RuntimeError(f"Download URL returned HTTP {resp.status}")
        content_len = resp.headers.get("Content-Length")
        if content_len is not None and int(content_len) != expected_size:
            raise RuntimeError(
                f"Content-Length mismatch: server reports {content_len}, expected {expected_size}"
            )
    print("Public download URL verified successfully.")


def publish(archive_path: Path, repo: str = "Retrorerr/Portal", version: str | None = None,
            tag: str | None = None, skip_validation: bool = False, dry_run: bool = False,
            allow_dirty: bool = False) -> None:
    if not archive_path.exists():
        raise FileNotFoundError(f"Archive not found: {archive_path}")

    # Determine version and tag
    if not version:
        match = re.search(r"portal-(debian14-arm64-[0-9.]+)\.tar\.xz", archive_path.name)
        if match:
            version = match.group(1)
        elif MANIFEST_PATH.exists():
            current_manifest = json.loads(MANIFEST_PATH.read_text())
            version = current_manifest.get("version")
        if not version:
            raise ValueError("Version could not be determined. Please specify with --version.")

    if not tag:
        tag = f"runtime-{version}"

    print(f"Target repository: {repo}")
    print(f"Target tag:        {tag}")
    print(f"Runtime version:   {version}")
    print(f"Archive path:      {archive_path}")

    digest, size = compute_sha256_and_size(archive_path)
    print(f"Archive size:      {size} bytes")
    print(f"Archive SHA-256:   {digest}")

    if not skip_validation:
        validate_archive(archive_path, version)

    # Provenance: pin the exact source commit; never float on `main`.
    check_source_clean(allow_dirty=allow_dirty)
    source_sha = get_source_commit()
    print(f"Source commit:     {source_sha}")

    # Safety check: never overwrite an existing version with different bytes!
    if MANIFEST_PATH.exists():
        manifest = json.loads(MANIFEST_PATH.read_text())
        if manifest.get("version") == version:
            if manifest.get("compressed_bytes") != size or manifest.get("sha256") != digest:
                raise RuntimeError(
                    f"Refusing to overwrite existing version '{version}' in {MANIFEST_PATH.name} with different bytes!\n"
                    f"  Manifest: {manifest.get('compressed_bytes')} bytes, sha256={manifest.get('sha256')}\n"
                    f"  Archive:  {size} bytes, sha256={digest}"
                )

    existing_release = get_existing_release(repo, tag)
    asset_needs_upload = True

    if existing_release:
        print(f"GitHub release '{tag}' exists.")
        verify_tag_target(repo, tag, source_sha)
        assets = existing_release.get("assets", [])
        for a in assets:
            if a.get("name") == archive_path.name:
                check_existing_asset(a, archive_path.name, size, digest)
                print(f"Asset '{archive_path.name}' is already published with identical digest ({size} bytes, sha256={digest}).")
                asset_needs_upload = False
                break
    else:
        print(f"GitHub release '{tag}' does not exist yet (confirmed not-found).")

    if dry_run:
        print(f"Dry-run mode: source {source_sha}; skipping GitHub release creation, upload, and manifest write (zero mutation).")
        return

    # Create or upload release
    if not existing_release:
        print(f"Creating GitHub release '{tag}' and uploading '{archive_path.name}'...")
        cmd = [
            "gh", "release", "create", tag,
            str(archive_path),
            "--repo", repo,
            "--title", tag,
            "--target", source_sha,
            "--notes", f"Canonical Debian 14/Forky ARM64 runtime {version} for Portal.",
        ]
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(f"Failed to create release: {result.stderr or result.stdout}")
        print("Release created and asset uploaded.")
    elif asset_needs_upload:
        print(f"Uploading '{archive_path.name}' to existing release '{tag}'...")
        cmd = [
            "gh", "release", "upload", tag,
            str(archive_path),
            "--repo", repo,
        ]
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(f"Failed to upload asset: {result.stderr or result.stdout}")
        print("Asset uploaded.")

    # Update or verify assets/debian-runtime.json (backwards-compatible:
    # Rust's serde model ignores unknown fields, so `source_commit` is
    # safe for older APKs while new tooling can audit provenance).
    expected_url = f"https://github.com/{repo}/releases/download/{tag}/{archive_path.name}"
    manifest_data = {
        "version": version,
        "url": expected_url,
        "sha256": digest,
        "compressed_bytes": size,
        "source_commit": source_sha,
    }
    MANIFEST_PATH.write_text(json.dumps(manifest_data, indent=2) + "\n")
    print(f"Updated {MANIFEST_PATH.name} with release details.")

    # Verify release end-to-end
    verify_public_url(expected_url, size)
    verify_script = REPO_ROOT / "scripts/verify_runtime_release.py"
    if verify_script.exists():
        print("Running scripts/verify_runtime_release.py...")
        subprocess.run([sys.executable, str(verify_script)], check=True)

    print("\nSUCCESS: Runtime release successfully published and verified!")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", nargs="?", type=Path, help="Path to runtime .tar.xz archive")
    parser.add_argument("--repo", default="Retrorerr/Portal", help="GitHub repo (default: Retrorerr/Portal)")
    parser.add_argument("--version", help="Runtime version (e.g. debian14-arm64-2026.09.14.2)")
    parser.add_argument("--tag", help="Release tag (default: runtime-<version>)")
    parser.add_argument("--skip-validation", action="store_true", help="Skip inspecting archive internals")
    parser.add_argument("--dry-run", action="store_true", help="Validate and check status without publishing")
    parser.add_argument("--allow-dirty", action="store_true", help="Allow publishing from a dirty tree (explicit override)")
    args = parser.parse_args()

    archive_path = args.archive
    if not archive_path:
        if MANIFEST_PATH.exists():
            manifest = json.loads(MANIFEST_PATH.read_text())
            ver = manifest.get("version")
            default_path = REPO_ROOT / f"target/portal-{ver}.tar.xz"
            if default_path.exists():
                archive_path = default_path

    if not archive_path or not archive_path.exists():
        candidates = list((REPO_ROOT / "target").glob("portal-debian14-arm64-*.tar.xz"))
        if len(candidates) == 1:
            archive_path = candidates[0]
        else:
            parser.error("Archive path not found. Please specify archive path explicitly.")

    publish(
        archive_path=archive_path,
        repo=args.repo,
        version=args.version,
        tag=args.tag,
        skip_validation=args.skip_validation,
        dry_run=args.dry_run,
        allow_dirty=args.allow_dirty,
    )


if __name__ == "__main__":
    main()
