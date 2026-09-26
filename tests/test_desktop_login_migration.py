import os
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[1] / "assets" / "localdesktop-prepare-login.py"


class DesktopLoginMigrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        base = Path(self.temporary.name)
        self.script = types.ModuleType("desktop_login")
        exec(compile(SOURCE.read_text(encoding="utf-8"), str(SOURCE), "exec"),
             self.script.__dict__)
        self.script.ROOT_HOME = base / "root"
        self.script.HOME = base / "home" / "desktop"
        self.script.STAGING = base / "home" / ".portal-desktop-import-v1"
        self.script.STAGING_SENTINEL = self.script.STAGING / ".portal-owned-staging"
        self.script.MARKER = base / "state" / "desktop-login-v1"
        self.script.NSS_MARKER = base / "state" / "desktop-nss-v1"
        self.script.PASSWD = base / "etc" / "passwd"
        self.script.GROUP = base / "etc" / "group"
        self.script.MARKER.parent.mkdir()
        self.script.PASSWD.parent.mkdir()
        self.script.PASSWD.write_text("desktop:x:1000:1000:desktop:/home/desktop:/bin/bash\n")
        self.script.GROUP.write_text("desktop:x:1000:\n")
        self.script.ROOT_HOME.mkdir()
        self.script.HOME.mkdir(parents=True)
        # Ownership syscalls are validated on-device through PRoot; these
        # fixture tests exercise the non-destructive transaction on Windows.
        self.script.own_tree = lambda path: None
        self.script.sync_directory = lambda path: None

    def test_import_preserves_root_and_existing_desktop_conflict(self):
        (self.script.ROOT_HOME / ".config").mkdir()
        (self.script.ROOT_HOME / ".config" / "settings").write_text("root")
        (self.script.ROOT_HOME / ".config" / "kwinrc").write_text("legacy KWin")
        (self.script.HOME / ".config").mkdir()
        (self.script.HOME / ".config" / "settings").write_text("desktop")
        (self.script.ROOT_HOME / "Documents").mkdir()
        (self.script.ROOT_HOME / "Documents" / "note.txt").write_text("kept")

        imported, conflicts = self.script.import_legacy_home()
        self.assertEqual((imported, conflicts), (2, [".config/settings"]))
        self.assertEqual((self.script.HOME / ".config" / "settings").read_text(), "desktop")
        self.assertEqual((self.script.HOME / ".config" / "kwinrc").read_text(), "legacy KWin")
        self.assertEqual((self.script.HOME / "Documents" / "note.txt").read_text(), "kept")
        self.assertEqual((self.script.ROOT_HOME / "Documents" / "note.txt").read_text(), "kept")
        self.script.write_marker(imported, conflicts)
        (self.script.HOME / "Documents" / "note.txt").write_text("user change")
        self.assertIsNone(self.script.import_legacy_home())
        self.assertEqual((self.script.HOME / "Documents" / "note.txt").read_text(), "user change")

    def test_interrupted_portal_staging_is_replaced_without_touching_homes(self):
        (self.script.ROOT_HOME / "Documents").mkdir()
        (self.script.ROOT_HOME / "Documents" / "note.txt").write_text("complete")
        self.script.STAGING.mkdir()
        self.script.STAGING_SENTINEL.write_text("Portal desktop import v1\n")
        (self.script.STAGING / "entry").mkdir()
        (self.script.STAGING / "entry" / "note.txt").write_text("partial")
        imported, conflicts = self.script.import_legacy_home()
        self.assertEqual((imported, conflicts), (1, []))
        self.assertEqual((self.script.HOME / "Documents" / "note.txt").read_text(), "complete")
        self.assertEqual((self.script.ROOT_HOME / "Documents" / "note.txt").read_text(), "complete")

    def test_runtime_directory_is_cleaned_without_touching_profile_or_symlink_targets(self):
        runtime = Path(self.temporary.name) / "run" / "user-1000"
        runtime.mkdir(parents=True)
        (runtime / "wayland-0").write_text("stale socket placeholder")
        nested = runtime / "dbus-1"
        nested.mkdir()
        (nested / "session-bus").write_text("stale socket placeholder")
        outside = Path(self.temporary.name) / "outside"
        outside.mkdir()
        (outside / "keep").write_text("user-owned target")
        link = runtime / "external"
        try:
            link.symlink_to(outside, target_is_directory=True)
        except (OSError, NotImplementedError):
            self.skipTest("directory symlinks are unavailable on this host")

        profile = self.script.HOME / "Documents"
        profile.mkdir()
        (profile / "keep.txt").write_text("user profile")
        self.script.RUNTIME = runtime
        self.script.sync_directory = lambda path: None

        self.script.clear_runtime_directory()

        self.assertTrue(runtime.is_dir())
        self.assertEqual(list(runtime.iterdir()), [])
        self.assertEqual((outside / "keep").read_text(), "user-owned target")
        self.assertEqual((profile / "keep.txt").read_text(), "user profile")

    def test_unknown_staging_contents_fail_closed(self):
        self.script.STAGING.mkdir()
        (self.script.STAGING / "unknown").write_text("user data")
        with self.assertRaisesRegex(RuntimeError, "unrecognized legacy import"):
            self.script.import_legacy_home()
        self.assertTrue((self.script.STAGING / "unknown").exists())
        self.assertFalse(self.script.MARKER.exists())

    def test_ownership_is_checked_as_desktop_not_proot_fake_root(self):
        good = types.SimpleNamespace(stdout="1000:1000:755\n" + "1000:1000:700\n" * 2)
        with patch.object(self.script.subprocess, "run", return_value=good) as probe:
            self.script.validate_desktop_ownership()
        self.assertEqual(
            probe.call_args.args[0][:4],
            ["/usr/sbin/runuser", "-u", "desktop", "--"],
        )
        wrong = types.SimpleNamespace(stdout="0:0:700\n" + "1000:1000:700\n" * 2)
        with patch.object(self.script.subprocess, "run", return_value=wrong):
            with self.assertRaisesRegex(RuntimeError, "ownership validation failed"):
                self.script.validate_desktop_ownership()

    def test_account_database_is_repaired_once_and_validated_as_desktop(self):
        results = [
            types.SimpleNamespace(stdout="desktop:x:1000:1000:desktop:/home/desktop:/bin/bash\n"),
            types.SimpleNamespace(stdout="desktop:x:1000:\n"),
        ]
        with patch.object(self.script.subprocess, "run", side_effect=results) as probe:
            self.script.prepare_account_database()
        self.assertEqual(probe.call_count, 2)
        self.assertTrue(self.script.NSS_MARKER.is_file())
        with patch.object(self.script.subprocess, "run") as probe:
            self.script.prepare_account_database()
        probe.assert_not_called()


if __name__ == "__main__":
    unittest.main()
