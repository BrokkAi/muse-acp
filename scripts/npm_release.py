#!/usr/bin/env python3
"""Pack verified native release archives for npm, then optionally publish."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

import release

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = 'https://registry.npmjs.org/'


def npm(*args, **kwargs):
    return subprocess.check_output([shutil.which('npm') or 'npm', *args], text=True, **kwargs)


def stage(assets, directory):
    # Validate every archive before staging anything. This also checks the
    # release commit, version, checksums, file modes, LICENSE and NOTICE.
    entries = {target: release.inspect_archive(assets, target) for target in release.TARGETS}
    manifest = json.loads((ROOT / 'npm/package.json').read_text())
    manifest.pop('private')
    manifest['version'] = release.VERSION
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'package.json').write_text(json.dumps(manifest, indent=2) + '\n')
    shutil.copytree(ROOT / 'npm/bin', directory / 'bin')
    (directory / 'bin/muse-acp.cjs').chmod(0o755)
    for name in ['README.md', 'LICENSE', 'NOTICE']:
        (directory / name).write_bytes(release.source_bytes(name))
    for target, files in entries.items():
        native = directory / 'native' / target
        native.mkdir(parents=True)
        binary = 'muse-acp.exe' if 'windows' in target else 'muse-acp'
        for name in [binary, 'release.json']:
            content, mode = files[name]
            (native / name).write_bytes(content)
            (native / name).chmod(mode)
    return manifest


def pack(assets, output):
    release.metadata()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        directory = Path(temp) / 'package'
        manifest = stage(assets, directory)
        result = json.loads(npm('pack', '--json', '--ignore-scripts', '--pack-destination',
                                str(output.resolve()), cwd=directory))[0]
    archive = output / result['filename']
    integrity = 'sha512-' + base64.b64encode(hashlib.sha512(archive.read_bytes()).digest()).decode()
    release.require(integrity == result['integrity'], 'npm tarball integrity mismatch')
    print(f"Packed {manifest['name']}@{manifest['version']}: {archive}")
    return archive, manifest, integrity


def registry_version(name, version):
    url = REGISTRY + urllib.parse.quote(name, safe='@') + '/' + urllib.parse.quote(version, safe='')
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def publish(archive, manifest, integrity):
    existing = registry_version(manifest['name'], manifest['version'])
    if existing is not None:
        release.require(existing['dist']['integrity'] == integrity,
                        'This npm version already exists with different contents; refusing to overwrite or skip it')
        print('Identical npm version already published.')
        return
    dist_tag = 'next' if '-' in manifest['version'] else 'latest'
    subprocess.run([shutil.which('npm') or 'npm', 'publish', str(archive.resolve()),
                    '--access=public', '--ignore-scripts', '--registry=' + REGISTRY,
                    '--tag=' + dist_tag], check=True)
    # A new package/version can take a few seconds to reach registry read replicas.
    published = None
    for attempt in range(6):
        published = registry_version(manifest['name'], manifest['version'])
        if published is not None:
            break
        if attempt < 5:
            time.sleep(5)
    release.require(published is not None and published['dist']['integrity'] == integrity,
                    'Published npm tarball integrity could not be verified')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['pack', 'publish'])
    parser.add_argument('--assets', type=Path, default=Path('dist'))
    parser.add_argument('--output', type=Path, default=Path('npm-dist'))
    args = parser.parse_args()
    if args.command == 'publish' and os.environ.get('GITHUB_ACTIONS') == 'true':
        release.require(os.environ.get('GITHUB_EVENT_NAME') == 'push'
                        and os.environ.get('GITHUB_REF') == 'refs/tags/' + release.TAG,
                        'Automated npm publication requires an explicit matching tag push')
    archive, manifest, integrity = pack(args.assets, args.output)
    if args.command == 'publish':
        publish(archive, manifest, integrity)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, KeyError, ValueError, OSError) as error:
        sys.exit(str(error))
