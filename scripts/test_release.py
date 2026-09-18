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


class DraftRecovery(unittest.TestCase):
    def test_hidden_draft_found_on_later_page_and_read_by_id(self):
        draft = {'id': 42, 'tag_name': release.TAG, 'draft': True}
        first = [{'id': i, 'tag_name': f'other-{i}'} for i in range(100)]
        with patch.object(release, 'optional', return_value=None), patch.object(
            release, 'api', side_effect=[first, [draft], draft]
        ) as api:
            self.assertEqual(release.find_release(release.TAG), draft)
        self.assertEqual(api.call_args_list[-1].args, ('releases/42',))

    def test_rest_hidden_draft_resolves_via_graphql(self):
        draft = {'id': 42, 'tag_name': release.TAG, 'draft': True}
        response = {'data': {'repository': {'release': {'databaseId': 42}}}}
        with patch.object(release, 'optional', return_value=None), patch.object(
            release, 'api', side_effect=[[], draft]
        ) as api, patch.object(release, 'gh', return_value=json.dumps(response).encode()):
            self.assertEqual(release.find_release(release.TAG), draft)
        self.assertEqual(api.call_args_list[-1].args, ('releases/42',))

    def test_duplicate_drafts_fail_closed(self):
        draft = {'id': 42, 'tag_name': release.TAG}
        with patch.object(release, 'optional', return_value=None), patch.object(
            release, 'api', return_value=[draft, draft]
        ):
            with self.assertRaisesRegex(RuntimeError, 'Multiple releases'):
                release.find_release(release.TAG)

    def test_upload_readback_and_completion_use_release_id(self):
        draft = {'id': 42, 'draft': True, 'assets': []}
        uploaded = dict(draft, assets=[{'name': 'install.sh', 'id': 99}])
        with tempfile.TemporaryDirectory() as temp:
            staged = Path(temp)
            (staged / 'install.sh').write_bytes(b'installer')
            with patch.dict(release.os.environ, {'GITHUB_REF': 'refs/tags/' + release.TAG}), \
                 patch.object(release, 'metadata'), patch.object(release, 'validate'), \
                 patch.object(release, 'release_state', return_value=draft), \
                 patch.object(release, 'authorization'), patch.object(release, 'compare_remote') as compare, \
                 patch.object(release, 'expected_names', return_value={'install.sh'}), \
                 patch.object(release, 'gh', return_value=b'installer'), \
                 patch.object(release, 'api', return_value=uploaded) as api:
                release.publish(staged)
            self.assertEqual([c.args[0] for c in api.call_args_list], ['releases/42'] * 4)
            self.assertEqual(compare.call_count, 3)


if __name__ == '__main__':
    unittest.main()
