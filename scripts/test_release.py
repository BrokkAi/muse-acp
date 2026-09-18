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
        with patch.object(release, 'metadata'), patch.object(release.subprocess, 'run'), patch.object(Path, 'read_bytes', read):
            release.package(target, self.directory)

    def test_all_platform_archives_and_permissions(self):
        for target in release.TARGETS:
            self.build(target)
            entries = release.inspect_archive(self.directory, target)
            manifest = json.loads(entries['release.json'][0])
            self.assertEqual(manifest['commit'], release.SHA)
            self.assertEqual(manifest['target'], target)
        (self.directory / 'install.sh').write_bytes(Path('install.sh').read_bytes())
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

    def test_authorization_rejects_local_identity(self):
        with patch.dict(release.os.environ, {'GITHUB_ACTIONS': 'false'}):
            with self.assertRaisesRegex(RuntimeError, 'actual Actions'):
                release.authorization()


if __name__ == '__main__':
    unittest.main()
