import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release


class ArchiveValidation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.directory = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def build(self, target):
        original = Path.read_bytes
        def read(path):
            if str(path).startswith('target/'):
                return b'test binary bytes'
            return original(path)
        with patch.object(release, 'metadata'), patch.object(release, 'selftest'), patch.object(Path, 'read_bytes', read):
            release.package(target, self.directory)

    def test_all_platform_archives_and_permissions(self):
        for target in release.TARGETS:
            self.build(target)
            entries = release.inspect_archive(self.directory, target)
            manifest = json.loads(entries['release.json'][0])
            self.assertEqual(manifest['commit'], release.SHA)
            self.assertEqual(manifest['target'], target)
        for installer in release.INSTALLERS:
            (self.directory / installer).write_bytes(Path(installer).read_bytes())
        committed_source_bytes = release.source_bytes

        def source_bytes(name):
            if name in release.INSTALLERS:
                return Path(name).read_bytes()
            return committed_source_bytes(name)

        with patch.object(release, 'source_bytes', side_effect=source_bytes):
            release.validate(self.directory)

    def test_all_installers_are_required(self):
        self.build(release.TARGETS[0])
        for installer in release.INSTALLERS:
            (self.directory / installer).write_bytes(Path(installer).read_bytes())
        (self.directory / release.INSTALLERS[-1]).unlink()
        with self.assertRaisesRegex(RuntimeError, 'Missing'):
            release.validate(self.directory)

    def test_corrupt_archive_fails(self):
        target = release.TARGETS[0]
        self.build(target)
        path = self.directory / release.archive_name(target)
        path.write_bytes(path.read_bytes() + b'corrupt')
        with self.assertRaisesRegex(RuntimeError, 'checksum'):
            release.inspect_archive(self.directory, target)

    def test_wrong_commit_fails(self):
        target = release.TARGETS[0]
        self.build(target)
        with patch.object(release, 'SHA', '0' * 40):
            with self.assertRaisesRegex(RuntimeError, 'metadata'):
                release.inspect_archive(self.directory, target)

    def test_missing_platform_fails(self):
        self.build(release.TARGETS[0])
        with self.assertRaisesRegex(RuntimeError, 'Missing'):
            release.validate(self.directory)

    def test_different_payloads_do_not_compare_equal(self):
        target = release.TARGETS[0]
        self.build(target)
        first = release.inspect_archive(self.directory, target)
        with patch.object(release, 'SHA', '1' * 40):
            self.build(target)
            second = release.inspect_archive(self.directory, target)
        self.assertNotEqual(first, second)

    def test_resume_archive_without_checksum_preserves_uploaded_compression(self):
        target = release.TARGETS[0]
        self.build(target)
        name = release.archive_name(target)
        raw = bytearray((self.directory / name).read_bytes())
        raw[4:8] = (123456789).to_bytes(4, 'little')  # Different gzip timestamp.
        remote = {'assets': [{'name': name, 'id': 42}]}
        with patch.object(release, 'gh', return_value=bytes(raw)):
            release.compare_remote(remote, self.directory, False, resume=True)
        self.assertEqual((self.directory / name).read_bytes(), raw)
        release.inspect_archive(self.directory, target)

    def test_authorization_rejects_local_identity(self):
        with patch.dict(release.os.environ, {'GITHUB_ACTIONS': 'false'}):
            with self.assertRaisesRegex(RuntimeError, 'actual Actions'):
                release.authorization()


if __name__ == '__main__':
    unittest.main()
