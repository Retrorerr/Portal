"""Guest file operations exercised on a real filesystem, without Android."""
import base64
from concurrent.futures import ThreadPoolExecutor
import contextlib
import errno
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import uuid

HELPER = Path(__file__).resolve().parents[1] / "assets/portal-file-transfer.py"
spec = importlib.util.spec_from_file_location("portal_files", HELPER)
files = importlib.util.module_from_spec(spec)
spec.loader.exec_module(files)


class FileTransferTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name) / "home"
        self.home.mkdir()
        self.files = files.Files(str(self.home))
        self.downloads = self.files.defaults()["downloads"]
        quiet = contextlib.redirect_stderr(io.StringIO())
        quiet.__enter__()
        self.addCleanup(quiet.__exit__, None, None, None)

    def request(self, name="file.bin", **kwargs):
        return dict(directory=self.downloads, name=name, job=str(uuid.uuid4()), **kwargs)

    def test_xdg_downloads_are_data_and_remain_inside_home(self):
        config = self.home / ".config"
        config.mkdir()
        user_dirs = config / "user-dirs.dirs"
        user_dirs.write_text('XDG_DOWNLOAD_DIR="$HOME/Receiving"\n')
        defaults = self.files.defaults()
        self.assertEqual(defaults["label"], "Receiving")
        self.assertEqual(self.files.resolve(defaults["downloads"]), str(self.home / "Receiving"))
        user_dirs.write_text('XDG_DOWNLOAD_DIR="/tmp"\n')
        self.assertEqual(self.files.defaults()["label"], "Downloads")
        user_dirs.write_text('XDG_DOWNLOAD_DIR="$(touch injected)"\n')
        self.assertEqual(self.files.defaults()["label"], "Downloads")
        self.assertFalse((self.home / "injected").exists())

    def test_round_trip_arbitrary_binary_and_empty_file(self):
        for payload in (b"", bytes(range(256)) * 9000):
            result = self.files.import_file(self.request(), io.BytesIO(payload))
            output = io.BytesIO()
            self.files.export({"entry": files.entry_id("Downloads/" + result["name"])}, output)
            self.assertEqual(hashlib.sha256(payload).digest(), hashlib.sha256(output.getvalue()).digest())

    def test_names_are_data_and_duplicates_never_replace(self):
        for name in ("日本語 résumé.txt", "hello\nthere.txt", "$(touch injected); 'quoted'.txt", "a" * 230 + ".txt"):
            first = self.files.import_file(self.request(name), io.BytesIO(b"first"))
            second = self.files.import_file(self.request(name), io.BytesIO(b"second"))
            self.assertNotEqual(first["name"], second["name"])
            self.assertEqual((self.home / "Downloads" / first["name"]).read_bytes(), b"first")
            self.assertEqual((self.home / "Downloads" / second["name"]).read_bytes(), b"second")
        self.assertFalse((self.home / "injected").exists())

    def test_concurrent_publication_keeps_every_file(self):
        def transfer(index):
            payload = bytes([index]) * 1024
            result = self.files.import_file(self.request("same.dat"), io.BytesIO(payload))
            return result["name"], payload
        with ThreadPoolExecutor(max_workers=8) as pool:
            results = list(pool.map(transfer, range(16)))
        self.assertEqual(len({name for name, _ in results}), 16)
        for name, payload in results:
            self.assertEqual((self.home / "Downloads" / name).read_bytes(), payload)

    def test_symlinks_and_hardlinks_resolve_in_guest_home(self):
        original = self.home / "Downloads/original"
        original.write_bytes(b"linked contents")
        os.link(original, self.home / "hardlink")
        (self.home / "symlink").symlink_to("Downloads/original")
        for name in ("hardlink", "symlink"):
            output = io.BytesIO()
            self.files.export({"entry": files.entry_id(name)}, output)
            self.assertEqual(output.getvalue(), b"linked contents")

    def test_home_boundary_and_special_files(self):
        (self.home / "outside").symlink_to(self.temporary.name)
        for path in ("../secret", "/etc/passwd", "outside/secret"):
            with self.assertRaises(ValueError):
                self.files.resolve(files.entry_id(path))
        os.mkfifo(self.home / "fifo")
        with self.assertRaises(ValueError):
            self.files.export({"entry": files.entry_id("fifo")}, io.BytesIO())
        with self.assertRaises((ValueError, OSError)):
            self.files.export({"entry": files.entry_id("Downloads")}, io.BytesIO())
        names = {entry["name"] for entry in self.files.listing({"directory": files.entry_id(".")})["entries"]}
        self.assertNotIn("outside", names)
        self.assertNotIn("fifo", names)

    def test_failed_source_and_full_storage_leave_no_partial_file(self):
        class BrokenSource(io.BytesIO):
            def read(self, size=-1):
                if self.tell():
                    raise OSError("source disappeared")
                return super().read(3)
        with self.assertRaises(OSError):
            self.files.import_file(self.request(), BrokenSource(b"abcdef"))
        with patch.object(files, "copy", side_effect=OSError(errno.ENOSPC, "No space left")):
            with self.assertRaises(OSError):
                self.files.import_file(self.request(), io.BytesIO(b"test"))
        self.assertEqual(list((self.home / "Downloads").iterdir()), [])

    def test_incomplete_known_size_never_publishes(self):
        with self.assertRaisesRegex(ValueError, "reported size"):
            self.files.import_file(self.request(size=50), io.BytesIO(b"partial"))
        self.assertEqual(list((self.home / "Downloads").iterdir()), [])

    def test_staged_import_waits_for_provider_validation_before_commit(self):
        request = self.request("staged.bin", defer_publish=True)
        result = self.files.import_file(request, io.BytesIO(b"validated"))
        self.assertTrue(result["staged"])
        self.assertFalse((self.home / "Downloads/staged.bin").exists())
        self.files.commit(request)
        self.assertEqual((self.home / "Downloads/staged.bin").read_bytes(), b"validated")
        rejected = self.request("failed-provider.bin", defer_publish=True)
        self.files.import_file(rejected, io.BytesIO(b"partial"))
        self.files.cleanup(rejected)
        self.assertFalse((self.home / "Downloads/failed-provider.bin").exists())
        self.assertFalse((self.home / "Downloads" / files.temporary_name(rejected["job"])).exists())

    def test_cleanup_only_removes_exact_operation_temporary(self):
        request = self.request()
        name = files.temporary_name(request["job"])
        (self.home / "Downloads" / name).write_bytes(b"partial")
        (self.home / "Downloads/user.part").write_bytes(b"keep")
        self.files.cleanup(request)
        self.assertFalse((self.home / "Downloads" / name).exists())
        self.assertTrue((self.home / "Downloads/user.part").exists())
        with self.assertRaises(ValueError):
            self.files.cleanup(dict(directory=self.downloads, job="../../user.part"))

    def test_pagination_hidden_files_and_destination_link(self):
        for index in range(205):
            (self.home / f"file-{index:03}").touch()
        (self.home / ".secret").touch()
        (self.home / "downloads-link").symlink_to("Downloads", target_is_directory=True)
        first = self.files.listing({"directory": files.entry_id(".")})
        second = self.files.listing({"directory": files.entry_id("."), "offset": 200})
        self.assertTrue(first["more"])
        self.assertFalse(second["more"])
        self.assertFalse({e["id"] for e in first["entries"]} & {e["id"] for e in second["entries"]})
        self.assertNotIn(".secret", [e["name"] for e in first["entries"] + second["entries"]])
        request = self.request("through-link")
        request["directory"] = files.entry_id("downloads-link")
        self.files.import_file(request, io.BytesIO(b"ok"))
        self.assertEqual((self.home / "Downloads/through-link").read_bytes(), b"ok")

    def test_old_kernel_atomic_link_fallback(self):
        with patch.object(files.ctypes, "CDLL", return_value=object()):
            self.files.import_file(self.request("fallback"), io.BytesIO(b"first"))
            self.files.import_file(self.request("fallback"), io.BytesIO(b"second"))
        self.assertEqual((self.home / "Downloads/fallback").read_bytes(), b"first")
        self.assertEqual((self.home / "Downloads/fallback (1)").read_bytes(), b"second")

    def test_large_stream_uses_64_bit_counts_and_bounded_reads(self):
        length = (1 << 31) + 17
        class Source:
            remaining = length
            def read(self, size):
                self.assertion(size)
                count = min(self.remaining, size)
                self.remaining -= count
                return b"\0" * count
            def assertion(self, size):
                if size > files.CHUNK:
                    raise AssertionError("unbounded read")
        class Sink:
            count = 0
            def write(self, data): self.count += len(data)
            def flush(self): pass
        sink = Sink()
        files.copy(Source(), sink, length)
        self.assertEqual(sink.count, length)

    def test_export_rejects_a_file_that_changes_during_copy(self):
        original = self.home / "changing"
        original.write_bytes(b"a" * 1024 * 1024)
        class ChangingTarget(io.BytesIO):
            def write(self, data):
                with original.open("ab") as changed:
                    changed.write(b"changed")
                # Change only once, to keep reading bounded.
                self.write = super().write
                return super().write(data)
        with self.assertRaisesRegex(ValueError, "changed while copying"):
            self.files.export({"entry": files.entry_id("changing")}, ChangingTarget())

    def test_command_line_binary_stream_is_separate_from_control(self):
        request = dict(action="import", **self.request("cli.bin"))
        env = dict(os.environ, HOME=str(self.home), PORTAL_FILE_REQUEST=json.dumps(request))
        imported = subprocess.run(["python3", str(HELPER)], input=b"\x00\xffcontents", env=env, capture_output=True, check=True)
        self.assertTrue(json.loads(imported.stdout)["ok"])
        env["PORTAL_FILE_REQUEST"] = json.dumps(dict(action="export", entry=files.entry_id("Downloads/cli.bin")))
        exported = subprocess.run(["python3", str(HELPER)], env=env, capture_output=True, check=True)
        self.assertEqual(exported.stdout, b"\x00\xffcontents")
        self.assertEqual(json.loads(exported.stderr.splitlines()[0])["bytes"], len(exported.stdout))


if __name__ == "__main__":
    unittest.main()
