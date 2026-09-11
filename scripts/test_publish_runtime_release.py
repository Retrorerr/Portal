#!/usr/bin/env python3
"""Deterministic mocked tests for scripts/publish_runtime_release.py.

No real GitHub release is created: `subprocess.run`, manifest writes, and
network verification are all mocked. Run with:
  python scripts/test_publish_runtime_release.py
"""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import publish_runtime_release as pub


def completed(returncode=0, stdout="", stderr=""):
    m = mock.Mock()
    m.returncode = returncode
    m.stdout = stdout
    m.stderr = stderr
    return m


class LookupTests(unittest.TestCase):
    def test_release_exists(self):
        payload = {"tagName": "runtime-v1", "assets": [], "isDraft": False}
        with mock.patch.object(pub.subprocess, "run", return_value=completed(0, json.dumps(payload))):
            self.assertEqual(pub.get_existing_release("o/r", "runtime-v1"), payload)

    def test_release_not_found_returns_none(self):
        for msg in ["release not found", "Release not found", "HTTP 404", "could not resolve to a Release"]:
            with mock.patch.object(pub.subprocess, "run", return_value=completed(1, "", msg)):
                self.assertIsNone(pub.get_existing_release("o/r", "runtime-missing"), msg)

    def test_auth_failure_fails_closed(self):
        with mock.patch.object(pub.subprocess, "run", return_value=completed(1, "", "Bad credentials")):
            with self.assertRaises(pub.ReleaseLookupError):
                pub.get_existing_release("o/r", "runtime-v1")

    def test_network_failure_fails_closed(self):
        for msg in ["network unreachable", "HTTP 502 Bad Gateway", "rate limit exceeded", "permission denied"]:
            with mock.patch.object(pub.subprocess, "run", return_value=completed(1, "", msg)):
                with self.assertRaises(pub.ReleaseLookupError):
                    pub.get_existing_release("o/r", "runtime-v1")

    def test_malformed_json_fails_closed(self):
        with mock.patch.object(pub.subprocess, "run", return_value=completed(0, "{not json")):
            with self.assertRaises(pub.ReleaseLookupError):
                pub.get_existing_release("o/r", "runtime-v1")


class AssetTests(unittest.TestCase):
    def test_identical_asset_passes(self):
        pub.check_existing_asset(
            {"name": "a.tar.xz", "size": 10, "digest": "sha256:abc"},
            "a.tar.xz", 10, "abc",
        )

    def test_mismatched_size_refused(self):
        with self.assertRaises(RuntimeError):
            pub.check_existing_asset(
                {"name": "a.tar.xz", "size": 11, "digest": "sha256:abc"},
                "a.tar.xz", 10, "abc",
            )

    def test_mismatched_digest_refused(self):
        with self.assertRaises(RuntimeError):
            pub.check_existing_asset(
                {"name": "a.tar.xz", "size": 10, "digest": "sha256:other"},
                "a.tar.xz", 10, "abc",
            )

    def test_missing_digest_refused_not_assumed_identical(self):
        with self.assertRaises(RuntimeError):
            pub.check_existing_asset(
                {"name": "a.tar.xz", "size": 10},
                "a.tar.xz", 10, "abc",
            )


class ProvenanceTests(unittest.TestCase):
    SHA = "a" * 40

    def test_source_commit_requires_exact_sha(self):
        with mock.patch.object(pub.subprocess, "run", return_value=completed(0, self.SHA + "\n")):
            self.assertEqual(pub.get_source_commit(), self.SHA)
        for bad in ["main", "abc", "A" * 40, "g" * 40, ""]:
            with mock.patch.object(pub.subprocess, "run", return_value=completed(0, bad + "\n")):
                with self.assertRaises(RuntimeError):
                    pub.get_source_commit()

    def test_dirty_tree_refused_without_override(self):
        with mock.patch.object(pub.subprocess, "run", return_value=completed(0, "")):
            pub.check_source_clean(allow_dirty=False)
        with mock.patch.object(pub.subprocess, "run", return_value=completed(0, " M foo\n")):
            with self.assertRaises(RuntimeError):
                pub.check_source_clean(allow_dirty=False)
            pub.check_source_clean(allow_dirty=True)

    def test_tag_target_mismatch_refused(self):
        with mock.patch.object(pub, "get_release_target", return_value="b" * 40):
            with self.assertRaises(RuntimeError):
                pub.verify_tag_target("o/r", "runtime-v1", self.SHA)
        with mock.patch.object(pub, "get_release_target", return_value=self.SHA):
            pub.verify_tag_target("o/r", "runtime-v1", self.SHA)
        # Legacy branch target: left alone, bytes still gated elsewhere.
        with mock.patch.object(pub, "get_release_target", return_value="main"):
            pub.verify_tag_target("o/r", "runtime-v1", self.SHA)


class PublishFlowTests(unittest.TestCase):
    SHA = "c" * 40

    def _archive(self, directory: Path) -> Path:
        archive = directory / "portal-debian13-arm64-2099.01.01.9.tar.xz"
        archive.write_bytes(b"fake-runtime-bytes")
        return archive

    def _patch_common(self, tmp: Path, existing=None):
        patches = [
            mock.patch.object(pub, "compute_sha256_and_size", return_value=("deadbeef", 18)),
            mock.patch.object(pub, "validate_archive", return_value=None),
            mock.patch.object(pub, "check_source_clean", return_value=None),
            mock.patch.object(pub, "get_source_commit", return_value=self.SHA),
            mock.patch.object(pub, "get_existing_release", return_value=existing),
            mock.patch.object(pub, "verify_public_url", return_value=None),
            mock.patch.object(pub, "MANIFEST_PATH", tmp / "debian-runtime.json"),
            mock.patch.object(pub, "REPO_ROOT", tmp),
        ]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)

    def test_create_uses_exact_source_sha_target(self):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            archive = self._archive(tmp)
            self._patch_common(tmp, existing=None)
            with mock.patch.object(pub.subprocess, "run", return_value=completed(0, "")) as run:
                pub.publish(archive, version="debian13-arm64-2099.01.01.9", skip_validation=True)
            create_calls = [c for c in run.call_args_list if "create" in str(c)]
            self.assertTrue(create_calls, "expected a gh release create call")
            cmd = str(create_calls[0])
            self.assertIn("--target", cmd)
            self.assertIn(self.SHA, cmd)
            self.assertNotIn("main", cmd.replace("debian13-arm64-2099.01.01.9", ""))
            manifest = json.loads((tmp / "debian-runtime.json").read_text())
            self.assertEqual(manifest["source_commit"], self.SHA)

    def test_dry_run_causes_zero_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            archive = self._archive(tmp)
            self._patch_common(tmp, existing=None)
            with mock.patch.object(pub.subprocess, "run", return_value=completed(0, "")) as run:
                pub.publish(archive, version="debian13-arm64-2099.01.01.9",
                            skip_validation=True, dry_run=True)
            for call in run.call_args_list:
                self.assertNotIn("create", str(call))
                self.assertNotIn("upload", str(call))
            self.assertFalse((tmp / "debian-runtime.json").exists())

    def test_existing_identical_asset_needs_no_upload(self):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            archive = self._archive(tmp)
            existing = {
                "tagName": "runtime-debian13-arm64-2099.01.01.9",
                "assets": [{"name": archive.name, "size": 18, "digest": "sha256:deadbeef"}],
                "isDraft": False,
            }
            self._patch_common(tmp, existing=existing)
            with mock.patch.object(pub, "verify_tag_target", return_value=None), \
                 mock.patch.object(pub.subprocess, "run", return_value=completed(0, "")) as run:
                pub.publish(archive, version="debian13-arm64-2099.01.01.9", skip_validation=True)
            for call in run.call_args_list:
                self.assertNotIn("upload", str(call))

    def test_rust_manifest_tolerates_provenance_field(self):
        src = (REPO_ROOT / "src/core/provisioning.rs").read_text()
        # Backwards compatibility: no deny_unknown_fields on the manifest
        # model, and the new provenance field is optional with a default.
        runtime_struct = src.split("struct RuntimeArtifact")[1].split("}")[0]
        self.assertNotIn("deny_unknown_fields)]", runtime_struct)
        self.assertIn("source_commit", src)
        self.assertIn("#[serde(default)]", src)


if __name__ == "__main__":
    unittest.main(verbosity=2)
