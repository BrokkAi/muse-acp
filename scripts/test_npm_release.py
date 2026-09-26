import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

import npm_release
import release


class NpmPackaging(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.assets = self.root / 'assets'

    def build_archives(self):
        original = Path.read_bytes
        def read(path):
            if str(path).startswith('target/'):
                return b'test native binary'
            return original(path)
        with patch.object(release, 'metadata'), patch.object(release, 'selftest'), patch.object(Path, 'read_bytes', read):
            for target in release.TARGETS:
                release.package(target, self.assets)

    def test_stages_all_verified_platforms_and_cargo_version_without_install_scripts(self):
        self.build_archives()
        output = self.root / 'package'
        manifest = npm_release.stage(self.assets, output)
        self.assertEqual(manifest['version'], release.VERSION)
        self.assertEqual(manifest['name'], '@brokkai/muse-acp')
        self.assertNotIn('private', manifest)
        self.assertNotIn('scripts', manifest)
        self.assertEqual(json.loads((output / 'package.json').read_text()), manifest)
        for target in release.TARGETS:
            binary = 'muse-acp.exe' if 'windows' in target else 'muse-acp'
            native = output / 'native' / target
            self.assertEqual((native / binary).read_bytes(), b'test native binary')
            self.assertEqual(json.loads((native / 'release.json').read_text())['commit'], release.SHA)
        self.assertEqual((output / 'LICENSE').read_bytes(), release.source_bytes('LICENSE'))

    def test_missing_or_corrupt_platform_prevents_packaging(self):
        self.build_archives()
        archive = self.assets / release.archive_name(release.TARGETS[-1])
        archive.write_bytes(archive.read_bytes() + b'corrupt')
        output = self.root / 'package'
        with self.assertRaisesRegex(RuntimeError, 'checksum'):
            npm_release.stage(self.assets, output)
        self.assertFalse(output.exists())
        archive.unlink()
        with self.assertRaises(FileNotFoundError):
            npm_release.stage(self.assets, output)

    def test_publish_retries_only_skip_an_identical_tarball(self):
        manifest = {'name': '@brokkai/muse-acp', 'version': '0.5.0'}
        with patch.object(npm_release, 'registry_version', return_value={'dist': {'integrity': 'same'}}), \
             patch.object(npm_release.subprocess, 'run') as run:
            npm_release.publish(Path('package.tgz'), manifest, 'same')
            run.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, 'different contents'):
                npm_release.publish(Path('package.tgz'), manifest, 'different')
            run.assert_not_called()

    def test_registry_errors_are_not_treated_as_a_new_package(self):
        for code in [401, 403, 429, 500]:
            with self.subTest(code=code), patch.object(npm_release.urllib.request, 'urlopen',
                    side_effect=urllib.error.HTTPError('url', code, 'error', {}, None)):
                with self.assertRaises(urllib.error.HTTPError):
                    npm_release.registry_version('@brokkai/muse-acp', '0.5.0')
        with patch.object(npm_release.urllib.request, 'urlopen',
                side_effect=urllib.error.HTTPError('url', 404, 'missing', {}, None)):
            self.assertIsNone(npm_release.registry_version('@brokkai/muse-acp', '0.5.0'))


if __name__ == '__main__':
    unittest.main()
