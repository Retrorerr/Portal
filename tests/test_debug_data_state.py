import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
from debug_data_state import prepare, isolate, restore, validate


class Files:
    def __init__(self, root):
        self.root, self.fail_after, self.moves = Path(root), None, 0

    def entries(self):
        return sorted(p.name for p in self.root.iterdir())

    def info(self, path):
        try:
            s = (self.root / path).lstat()
            return dict(device=s.st_dev, inode=s.st_ino, mode=s.st_mode, uid=s.st_uid, gid=s.st_gid)
        except FileNotFoundError:
            return None

    def mkdir(self, path):
        (self.root / path).mkdir(parents=True, exist_ok=True)

    def rename(self, source, destination):
        if self.moves == self.fail_after:
            raise InterruptedError('Simulated interruption')
        assert not os.path.lexists(self.root / destination)
        (self.root / source).rename(self.root / destination)
        self.moves += 1

    def write(self, path, text):
        (self.root / path).write_text(text)


class DebugDataStateTests(unittest.TestCase):
    def original(self, root):
        fs = Files(root)
        for name in ('files', 'cache', 'shared_prefs'):
            fs.mkdir(name)
            (fs.root / name / 'original').write_text(name)
        os.link(fs.root / 'files/original', fs.root / 'files/hardlink')
        (fs.root / 'files/symlink').symlink_to(fs.root / 'files/original')
        return fs

    def check_original(self, fs, state):
        for name, identity in state['original'].items():
            self.assertEqual(fs.info(name), identity)
            self.assertEqual((fs.root / name / 'original').read_text(), name)
        self.assertEqual((fs.root / 'files/original').stat().st_ino,
                         (fs.root / 'files/hardlink').stat().st_ino)
        self.assertEqual((fs.root / 'files/symlink').read_text(), 'files')

    def test_fresh_state_preserved_and_original_inodes_restored(self):
        with tempfile.TemporaryDirectory() as tmp:
            fs = self.original(tmp)
            record = Path(tmp).parent / (Path(tmp).name + '.json')
            try:
                state = prepare(fs, 'device', record)
                isolate(fs, state, record)
                self.assertEqual(fs.entries(), [state['store']])
                for name in ('files', 'cache', 'no_backup'):
                    fs.mkdir(name)
                    (fs.root / name / 'fresh').write_text('fresh ' + name)
                restore(fs, state, record)
                self.check_original(fs, state)
                for name in ('files', 'cache', 'no_backup'):
                    self.assertEqual((fs.root / state['store'] / 'fresh' / name / 'fresh').read_text(), 'fresh ' + name)
                restore(fs, state, record)
            finally:
                record.unlink(missing_ok=True)

    def test_interrupted_isolation_can_resume_or_restore(self):
        for resume in (True, False):
            for cutoff in range(3):
                with self.subTest(resume=resume, cutoff=cutoff), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp) / 'data'; root.mkdir(); fs = self.original(root)
                    record = Path(tmp) / 'record.json'; state = prepare(fs, 'device', record)
                    fs.fail_after = cutoff
                    with self.assertRaises(InterruptedError):
                        isolate(fs, state, record)
                    fs.fail_after = None
                    state = json.loads(record.read_text())
                    if resume:
                        isolate(fs, state, record)
                    restore(fs, state, record)
                    self.check_original(fs, state)

    def test_interrupted_restore_can_resume_without_overwriting_either_state(self):
        for cutoff in range(5):
            with self.subTest(cutoff=cutoff), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp) / 'data'; root.mkdir(); fs = self.original(root)
                record = Path(tmp) / 'record.json'; state = prepare(fs, 'device', record)
                isolate(fs, state, record)
                for name in ('files', 'cache'):
                    fs.mkdir(name); (root / name / 'fresh').write_text(name)
                fs.moves = 0; fs.fail_after = cutoff
                with self.assertRaises(InterruptedError):
                    restore(fs, state, record)
                fs.fail_after = None
                restore(fs, json.loads(record.read_text()), record)
                self.check_original(fs, state)

    def test_conflicting_new_data_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'data'; root.mkdir(); fs = self.original(root)
            record = Path(tmp) / 'record.json'; state = prepare(fs, 'device', record)
            isolate(fs, state, record)
            fs.mkdir('files'); (root / 'files/new').write_text('new')
            fs.fail_after = fs.moves
            with self.assertRaises(InterruptedError):
                restore(fs, state, record)
            (root / 'files').rename(root / 'app_webview'); fs.mkdir('files')
            with self.assertRaisesRegex(RuntimeError, 'changed'):
                restore(fs, state, record)
            self.assertEqual((root / 'app_webview/new').read_text(), 'new')
            self.assertEqual((root / state['store'] / 'original/files/original').read_text(), 'files')

    def test_record_cannot_target_other_packages_or_parent_paths(self):
        with self.assertRaises(RuntimeError):
            validate(dict(version=1, package='app.polarbear.portal', store='.portal-e2e-' + 'a' * 32,
                          original={'files': {}}))
        for store in ('../files', '.portal-e2e-' + 'g' * 32):
            with self.assertRaises(RuntimeError):
                validate(dict(version=1, package='app.polarbear', store=store, original={'files': {}}))


if __name__ == '__main__':
    unittest.main()
