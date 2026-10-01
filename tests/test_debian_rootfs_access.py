"""Image-builder side of passwordless sudo, and the lock-extension helper.

The launch-time counterpart (existing installs) is covered by
tests/guest_access.rs; these check that a *new* image starts with the same
group and drop-in, and that the packages behind the everyday tools are seeded
and locked.
"""

import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "scripts"))

import build_debian_rootfs as builder  # noqa: E402
import package_debian_runtime as packager  # noqa: E402


class GuestAccessImageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "etc").mkdir()

    def test_group_database_has_a_sudo_group_with_desktop_in_it(self):
        builder.write_guest_access(self.root)
        lines = (self.root / "etc/group").read_text().splitlines()
        self.assertIn("sudo:x:27:desktop", lines)
        # The pre-existing entries are unchanged.
        for line in ("root:x:0:", "desktop:x:1000:", "audio:x:29:desktop", "video:x:44:desktop"):
            self.assertIn(line, lines)
        self.assertEqual(len(lines), len(set(lines)))

    def test_dropin_is_passwordless_for_the_sudo_group_and_only_that(self):
        builder.write_guest_access(self.root)
        text = (self.root / "etc/sudoers.d/portal-desktop").read_bytes().decode()
        self.assertEqual(
            text,
            "# Managed by Portal: the desktop account has no password, so sudo asks for none.\n"
            "%sudo ALL=(ALL:ALL) NOPASSWD: ALL\n",
        )
        self.assertNotIn("\r", text)

    @unittest.skipIf(os.name == "nt", "POSIX modes")
    def test_dropin_mode_is_0440(self):
        builder.write_guest_access(self.root)
        mode = stat.S_IMODE((self.root / "etc/sudoers.d/portal-desktop").stat().st_mode)
        self.assertEqual(mode, 0o440)

    def test_release_archive_records_the_dropin_as_0440(self):
        self.assertEqual(packager.SUDOERS_DROPIN_MODE, 0o440)
        self.assertEqual(packager.SUDOERS_DROPIN_PATH, "etc/sudoers.d/portal-desktop")

    def test_a_dropin_sudo_would_reject_stops_the_build(self):
        for bad in (
            "%sudo ALL=(ALL:ALL) NOPASWD: ALL\n",
            "%sudo ALL=(ALL:ALL) NOPASSWD: ALL",  # no final newline
            "%sudo ALL=(ALL:ALL) NOPASSWD: ALL\ndesktop ALL=(ALL) ALL\n",
        ):
            with self.subTest(bad=bad), patch("shutil.which", return_value=None):
                with self.assertRaises(ValueError):
                    builder.validate_sudoers(bad)

    def test_the_shipped_dropin_passes_validation(self):
        with patch("shutil.which", return_value=None):
            builder.validate_sudoers(builder.SUDOERS_DROPIN)


class SeedAndLockTests(unittest.TestCase):
    NEW_TOOLS = ("sudo", "wget", "curl", "ffmpeg", "fonts-noto-cjk")

    def test_the_everyday_tools_are_seeded_and_locked(self):
        lock = json.loads((REPO / "assets/debian-runtime-packages.json").read_text())
        for package in self.NEW_TOOLS:
            with self.subTest(package=package):
                self.assertIn(package, builder.SEED_PACKAGES)
                self.assertIn(package, lock)
                for key in ("Version", "Filename", "SHA256", "Size"):
                    self.assertTrue(lock[package][key])

    def test_the_ffmpeg_cli_is_locked_with_the_codec_libraries_the_image_already_has(self):
        lock = json.loads((REPO / "assets/debian-runtime-packages.json").read_text())
        self.assertEqual(lock["ffmpeg"]["Version"], lock["libavcodec61"]["Version"])


class ExtendLockTests(unittest.TestCase):
    def entry(self, version):
        return {"Version": version, "Filename": f"pool/{version}.deb", "SHA256": "0" * 64, "Size": "1"}

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.lock = Path(self.temporary.name) / "lock.json"
        self.locked = {"libb": self.entry("1"), "other": self.entry("7")}
        self.lock.write_text(json.dumps(self.locked))
        self.index = {
            "tool": {"Package": "tool", "Depends": "libb (>= 1), libc", **self.entry("2")},
            "libb": {"Package": "libb", **self.entry("1")},
            "libc": {"Package": "libc", **self.entry("3")},
            "other": {"Package": "other", **self.entry("7")},
            "unlocked-seed": {"Package": "unlocked-seed", **self.entry("9")},
        }
        for name, package in self.index.items():
            package["Filename"] = f"pool/{name}.deb"

    def extend(self, seeds, seeded=("tool", "unlocked-seed")):
        with patch.object(packager, "LOCK", self.lock), \
                patch.object(packager, "fetch_package_index", return_value=self.index), \
                patch.object(packager, "SEED_PACKAGES", list(seeded)):
            packager.extend_lock(seeds)
        return json.loads(self.lock.read_text())

    def test_only_the_named_seed_and_its_new_dependencies_are_added(self):
        locked = self.extend(["tool"])
        self.assertEqual(set(locked), {"libb", "other", "tool", "libc"})
        self.assertNotIn("unlocked-seed", locked, "a seed nobody asked for stays out")

    def test_existing_locked_entries_keep_their_exact_entries(self):
        locked = self.extend(["tool"])
        self.assertEqual(locked["libb"], self.locked["libb"])
        self.assertEqual(locked["other"], self.locked["other"])

    def test_a_stale_index_refuses_to_write_anything(self):
        self.index["libb"]["Version"] = "2"
        before = self.lock.read_text()
        with self.assertRaises(ValueError):
            self.extend(["tool"])
        self.assertEqual(self.lock.read_text(), before)

    def test_a_package_that_is_not_a_seed_is_refused(self):
        with self.assertRaises(ValueError):
            self.extend(["libc"])

    def test_running_twice_changes_nothing(self):
        first = self.extend(["tool"])
        second = self.extend(["tool"])
        self.assertEqual(first, second)


if __name__ == "__main__":
    unittest.main()
