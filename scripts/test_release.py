import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

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
        release.validate(self.directory)

    def test_modified_installers_are_rejected(self):
        for target in release.TARGETS:
            self.build(target)
        for installer in release.INSTALLERS:
            (self.directory / installer).write_bytes(release.source_bytes(installer))
        for installer in release.INSTALLERS:
            with self.subTest(installer=installer):
                staged = self.directory / installer
                original = staged.read_bytes()
                staged.write_bytes(original + b'\n# changed after tagging\n')
                with self.assertRaisesRegex(RuntimeError, 'Installer differs'):
                    release.validate(self.directory)
                staged.write_bytes(original)

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


class TagOnlyRelease(unittest.TestCase):
    def test_master_push_with_released_version_is_a_no_op(self):
        env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': 'refs/heads/master'}
        with patch.dict(release.os.environ, env), patch.object(release, 'tag_commit', return_value='0' * 40):
            self.assertTrue(release.preflight_superseded())

    def test_unreleased_version_on_master_is_still_checked(self):
        env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': 'refs/heads/master'}
        with patch.dict(release.os.environ, env), patch.object(release, 'tag_commit', return_value=None):
            self.assertFalse(release.preflight_superseded())

    def test_release_branches_and_tags_keep_the_strict_check(self):
        for ref in ['refs/heads/brb/release-9.9.9', 'refs/tags/' + release.TAG]:
            env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': ref}
            with patch.dict(release.os.environ, env), patch.object(release, 'tag_commit', return_value='0' * 40):
                self.assertFalse(release.preflight_superseded(), ref)
        with patch.object(release, 'tag_commit', return_value='0' * 40):
            with self.assertRaisesRegex(RuntimeError, 'another commit'):
                release.tag_check()

    def test_authorization_evidence_needs_only_push_runs(self):
        push_runs = [{'databaseId': n, 'workflowName': w, 'headSha': release.SHA,
                      'headBranch': 'master', 'event': 'push', 'status': 'completed',
                      'conclusion': 'success'} for n, w in [(1, 'ci'), (2, 'release')]]
        ci_detail = {'headSha': release.SHA, 'conclusion': 'success',
                     'jobs': [{'name': 'check', 'conclusion': 'success'}]}
        with patch.object(release, 'metadata'), patch.object(
            release, 'gh', side_effect=[json.dumps(push_runs).encode(),
                                        json.dumps(ci_detail).encode(), RuntimeError('release run read')]
        ):
            # Reaching the release push run's detail proves no manual
            # workflow_dispatch run was demanded.
            with self.assertRaisesRegex(RuntimeError, 'release run read'):
                release.evidence('authorization')

    def test_tag_provenance_needs_no_branch_artifacts(self):
        push_runs = [{'databaseId': n, 'workflowName': w, 'headSha': release.SHA,
                      'headBranch': 'master', 'event': 'push', 'status': 'completed',
                      'conclusion': 'success'} for n, w in [(1, 'ci'), (2, 'release')]]
        ci_detail = {'headSha': release.SHA, 'conclusion': 'success',
                     'jobs': [{'name': 'check', 'conclusion': 'success'}]}
        with patch.object(release, 'metadata'), patch.object(
            release, 'gh', side_effect=[json.dumps(push_runs).encode(),
                                        json.dumps(ci_detail).encode(),
                                        json.dumps(self.release_detail()).encode()]) as gh:
            release.evidence('commit')
        # The run list and its two details: no Actions artifact lookup, so a
        # branch artifact that expired before the tag was pushed cannot block
        # publication.
        self.assertEqual(gh.call_count, 3)

    def release_detail(self):
        jobs = [{'name': f'build ({t})', 'conclusion': 'success'} for t in release.TARGETS]
        jobs.append({'name': 'publisher', 'conclusion': 'success', 'steps': [
            {'name': 'Check publisher authorization', 'conclusion': 'success'},
            {'name': 'Validate version and all staged assets', 'conclusion': 'success'}]})
        return {'headSha': release.SHA, 'conclusion': 'success', 'jobs': jobs}

    def push_runs_and_ci(self):
        runs = [{'databaseId': n, 'workflowName': w, 'headSha': release.SHA,
                 'headBranch': 'master', 'event': 'push', 'status': 'completed',
                 'conclusion': 'success'} for n, w in [(1, 'ci'), (2, 'release')]]
        ci = {'headSha': release.SHA, 'conclusion': 'success',
              'jobs': [{'name': 'check', 'conclusion': 'success'}]}
        return runs, ci

    def artifact_fetch(self, corrupt=None):
        """A stand-in for one preflight run's six Actions artifacts."""
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        staged = Path(temp.name)
        original = Path.read_bytes
        def read(path):
            return b'branch preflight binary' if str(path).startswith('target/') else original(path)
        with patch.object(release, 'metadata'), patch.object(release, 'selftest'), patch.object(Path, 'read_bytes', read):
            for target in release.TARGETS:
                release.package(target, staged)
        for installer in release.INSTALLERS:
            (staged / installer).write_bytes(release.source_bytes(installer))
        zips, listing = {}, []
        for asset_id, target in enumerate(release.TARGETS, 1):
            name = release.archive_name(target)
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, 'w') as z:
                for member in [name, name + '.sha256']:
                    data = (staged / member).read_bytes()
                    if (name, member) == corrupt:
                        data += b'corrupt'
                    z.writestr(member, data)
            zips[str(asset_id)] = buffer.getvalue()
            listing.append({'id': asset_id, 'name': 'release-' + target, 'expired': False})
        def gh(*args, **kwargs):
            command = ' '.join(args)
            if command.startswith('run list'):
                return json.dumps(self.push_runs_and_ci()[0]).encode()
            if command.startswith('run view'):
                return json.dumps(self.push_runs_and_ci()[1] if args[2] == '1'
                                  else self.release_detail()).encode()
            if 'artifacts?per_page' in command:
                return json.dumps({'artifacts': listing}).encode()
            if command.endswith('/zip'):
                return zips[command.rsplit('/', 2)[1]]
            raise RuntimeError('unexpected gh call: ' + command)
        return gh

    def test_build_evidence_validates_branch_artifact_contents(self):
        # The documented local pre-tag check must still verify what the branch
        # run actually packaged, not merely that six artifacts exist.
        with patch.object(release, 'metadata'), patch.object(release, 'gh', side_effect=self.artifact_fetch()) as gh:
            release.evidence('build')
        self.assertGreater(gh.call_count, 3)

    def test_build_evidence_rejects_a_corrupt_branch_artifact(self):
        target = release.TARGETS[0]
        corrupt = (release.archive_name(target), release.archive_name(target))
        with patch.object(release, 'metadata'), patch.object(release, 'gh', side_effect=self.artifact_fetch(corrupt)):
            with self.assertRaisesRegex(RuntimeError, 'checksum'):
                release.evidence('build')


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
            finalization = api.call_args_list[2]
            self.assertEqual(finalization.args[2]['tag_name'], release.TAG)
            self.assertEqual(finalization.args[2]['target_commitish'], release.SHA)


class PublishedReleaseAudit(unittest.TestCase):
    """A released artifact is verified on its own terms, not against a rebuild."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.directory = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def build(self, target, out, binary):
        original = Path.read_bytes
        def read(path):
            if str(path).startswith('target/'):
                return binary
            return original(path)
        out.mkdir(parents=True, exist_ok=True)
        with patch.object(release, 'metadata'), patch.object(release, 'selftest'), patch.object(Path, 'read_bytes', read):
            release.package(target, out)

    def test_release_built_by_another_runner_audits_clean(self):
        # Two builds of one commit with different binaries stand in for a
        # hosted-runner update: new MSVC PE timestamps, CodeView GUIDs and
        # import hints, same code. The published release stays valid.
        published, rebuilt = self.directory / 'published', self.directory / 'rebuilt'
        for target in release.TARGETS:
            self.build(target, published, b'preflight runner binary')
            self.build(target, rebuilt, b'tag runner binary')
        for installer in release.INSTALLERS:
            (published / installer).write_bytes(release.source_bytes(installer))
        assets = [{'name': p.name, 'id': i} for i, p in enumerate(sorted(published.iterdir()), 1)]
        by_id = {str(a['id']): (published / a['name']).read_bytes() for a in assets}
        state = {'draft': False, 'assets': assets}
        with patch.object(release, 'gh', side_effect=lambda *args: by_id[args[-1].rsplit('/', 1)[-1]]):
            release.audit_release(state)
            # Uploads are still compared with the exact bytes that produced them.
            with self.assertRaisesRegex(RuntimeError, 'Published payload differs'):
                release.compare_remote(state, rebuilt, True)

    def test_draft_audit_rejects_an_inconsistent_archive(self):
        draft = self.directory / 'draft'
        for target in release.TARGETS:
            self.build(target, draft, b'tag runner binary')
        corrupted = release.archive_name(release.TARGETS[0])
        (draft / corrupted).write_bytes((draft / corrupted).read_bytes() + b'corrupt')
        assets = [{'name': p.name, 'id': i} for i, p in enumerate(sorted(draft.iterdir()), 1)]
        by_id = {str(a['id']): (draft / a['name']).read_bytes() for a in assets}
        with patch.object(release, 'gh', side_effect=lambda *args: by_id[args[-1].rsplit('/', 1)[-1]]):
            with self.assertRaisesRegex(RuntimeError, 'checksum'):
                release.audit_release({'draft': True, 'assets': assets})

    def test_conflicting_asset_names_fail_before_any_download(self):
        # Two assets with one name would collide on disk and hide one of them.
        assets = [{'name': 'install.sh', 'id': 1}, {'name': 'install.sh', 'id': 2}]
        with patch.object(release, 'gh') as gh:
            with self.assertRaisesRegex(RuntimeError, 'Conflicting remote assets'):
                release.audit_release({'draft': False, 'assets': assets})
        gh.assert_not_called()

    def test_asset_name_with_a_path_component_fails_before_any_download(self):
        assets = [{'name': '../install.sh', 'id': 1}]
        with patch.object(release, 'gh') as gh:
            with self.assertRaisesRegex(RuntimeError, 'Conflicting remote assets'):
                release.audit_release({'draft': True, 'assets': assets})
        gh.assert_not_called()


if __name__ == '__main__':
    unittest.main()
