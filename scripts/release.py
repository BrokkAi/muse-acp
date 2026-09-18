#!/usr/bin/env python3
"""Release packaging, read-only evidence checks, and explicit publication."""
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

REPO = 'BrokkAi/muse-acp'
TARGETS = ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
           'x86_64-apple-darwin', 'aarch64-apple-darwin', 'x86_64-pc-windows-msvc']
VERSION = tomllib.loads(Path('Cargo.toml').read_text())['package']['version']
TAG = os.environ.get('RELEASE_TAG', 'v' + VERSION)
SHA = os.environ.get('RELEASE_COMMIT') or subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def gh(*args, data=None):
    command = ['gh', *args]
    if data is not None:
        command += ['--input', '-']
    return subprocess.check_output(command, input=json.dumps(data).encode() if data is not None else None)


def api(path, method='GET', data=None):
    return json.loads(gh('api', '--hostname', 'github.com', '-X', method,
                         f'repos/{REPO}/{path}', data=data) or b'null')


def optional(path):
    # Only a definite HTTP 404 is absence. Authentication/network errors fail.
    p = subprocess.run(['gh', 'api', '--hostname', 'github.com', f'repos/{REPO}/{path}'], capture_output=True)
    if p.returncode:
        require(b'(HTTP 404)' in p.stderr, 'GitHub lookup failed: ' + p.stderr.decode())
        return None
    return json.loads(p.stdout)


def metadata():
    require(TAG == 'v' + VERSION, 'Tag does not match Cargo version')
    require(subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == SHA, 'Wrong checkout commit')
    require(not subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no']), 'Tracked inputs are dirty')


def archive_name(target):
    return f'muse-acp-{TAG}-{target}' + ('.zip' if 'windows' in target else '.tar.gz')


def expected_names():
    return {'install.sh'} | {n for t in TARGETS for n in (archive_name(t), archive_name(t) + '.sha256')}


def source_bytes(name):
    # Use committed bytes, independent of Windows checkout newline conversion.
    return subprocess.check_output(['git', 'show', 'HEAD:' + name])


def digest(data):
    return hashlib.sha256(data).hexdigest()


def selftest(binary_path):
    subprocess.run([str(binary_path), '--selftest'], check=True)


def package(target, out):
    metadata()
    require(target in TARGETS, 'Unknown target')
    out.mkdir(parents=True, exist_ok=True)
    binary = 'muse-acp.exe' if 'windows' in target else 'muse-acp'
    binary_path = Path('target') / target / 'release' / binary
    selftest(binary_path)
    entries = {binary: (binary_path.read_bytes(), 0o755)}
    entries.update({n: (source_bytes(n), 0o644) for n in ['README.md', 'LICENSE', 'NOTICE']})
    manifest = {'commit': SHA, 'version': VERSION, 'target': target,
                'files': {n: {'sha256': digest(b), 'mode': m} for n, (b, m) in entries.items()}}
    entries['release.json'] = (json.dumps(manifest, sort_keys=True).encode(), 0o644)
    name = archive_name(target)
    path = out / name
    if name.endswith('.zip'):
        with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
            for n, (b, mode) in entries.items():
                info = zipfile.ZipInfo(n)
                info.create_system = 3
                info.external_attr = (stat.S_IFREG | mode) << 16
                z.writestr(info, b)
    else:
        with tarfile.open(path, 'w:gz') as tar:
            for n, (b, mode) in entries.items():
                info = tarfile.TarInfo(f'muse-acp-{TAG}-{target}/{n}')
                info.size, info.mode, info.mtime = len(b), mode, 0
                tar.addfile(info, io.BytesIO(b))
    (out / (name + '.sha256')).write_text(f'{digest(path.read_bytes())}  {name}\n')
    inspect_archive(out, target)


def inspect_archive(directory, target):
    name = archive_name(target)
    data = (directory / name).read_bytes()
    require((directory / (name + '.sha256')).read_text().split() == [digest(data), name], 'Archive checksum mismatch')
    entries = {}
    if name.endswith('.zip'):
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            for info in z.infolist():
                require('/' not in info.filename and info.filename not in entries, 'Invalid zip member')
                entries[info.filename] = (z.read(info), (info.external_attr >> 16) & 0o777)
    else:
        prefix = f'muse-acp-{TAG}-{target}/'
        with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as tar:
            for info in tar:
                require(info.isfile() and info.name.startswith(prefix), 'Invalid tar member')
                n = info.name[len(prefix):]
                require('/' not in n and n not in entries, 'Invalid tar path')
                entries[n] = (tar.extractfile(info).read(), info.mode)
    binary = 'muse-acp.exe' if 'windows' in target else 'muse-acp'
    require(set(entries) == {binary, 'README.md', 'LICENSE', 'NOTICE', 'release.json'}, 'Incomplete archive')
    manifest = json.loads(entries['release.json'][0])
    require((manifest['commit'], manifest['version'], manifest['target']) == (SHA, VERSION, target), 'Wrong release metadata')
    require(set(manifest['files']) == set(entries) - {'release.json'}, 'Incomplete manifest')
    for n, spec in manifest['files'].items():
        b, mode = entries[n]
        require(digest(b) == spec['sha256'] and mode == spec['mode'], 'Payload or permission mismatch')
        require(mode == (0o755 if n == binary else 0o644), 'Unexpected permissions')
        if n != binary:
            require(b == source_bytes(n), 'Documentation differs from release commit')
    return entries


def validate(directory):
    require({p.name for p in directory.iterdir()} == expected_names(), 'Missing or unexpected release assets')
    require((directory / 'install.sh').read_bytes() == source_bytes('install.sh'), 'Installer differs')
    return {t: inspect_archive(directory, t) for t in TARGETS}


def tag_check():
    ref = optional('git/ref/tags/' + TAG)
    if ref:
        obj = ref['object']
        while obj['type'] == 'tag':
            obj = api('git/tags/' + obj['sha'])['object']
        require(obj['type'] == 'commit' and obj['sha'] == SHA, 'Existing tag points at another commit')


def find_release(tag):
    release = optional('releases/tags/' + tag)
    if release is not None:
        return release
    # GitHub's tag endpoint can hide drafts; the authenticated list includes them.
    matches = []
    page = 1
    while True:
        releases = api(f'releases?per_page=100&page={page}')
        matches.extend(r for r in releases if r['tag_name'] == tag)
        if len(releases) < 100:
            break
        page += 1
    require(len(matches) <= 1, 'Multiple releases use the proposed tag')
    return api('releases/' + str(matches[0]['id'])) if matches else None


def release_state():
    tag_check()
    release = find_release(TAG)
    if release:
        if not release['draft']:
            require(optional('git/ref/tags/' + TAG) is not None, 'Published release is missing its tag')
        require(release['target_commitish'] == SHA or optional('git/ref/tags/' + TAG), 'Draft target mismatch')
    return release


def compare_remote(release, staged, complete, resume=False):
    assets = release['assets']
    names = [a['name'] for a in assets]
    require(len(names) == len(set(names)) and set(names) <= expected_names(), 'Conflicting remote assets')
    if complete:
        require(set(names) == expected_names(), 'Incomplete published release')
    with tempfile.TemporaryDirectory() as d:
        directory = Path(d)
        for a in assets:
            data = gh('api', '--hostname', 'github.com', '-H', 'Accept: application/octet-stream',
                      f'repos/{REPO}/releases/assets/{a["id"]}')
            (directory / a['name']).write_bytes(data)
        for target in TARGETS:
            name = archive_name(target)
            if name in names:
                if name + '.sha256' not in names:
                    raw = (directory / name).read_bytes()
                    (directory / (name + '.sha256')).write_text(f'{digest(raw)}  {name}\n')
                    require(inspect_archive(directory, target) == inspect_archive(staged, target), 'Partial archive payload conflict')
                    if resume:
                        # Keep the immutable uploaded archive and add its own checksum.
                        (staged / name).write_bytes(raw)
                        (staged / (name + '.sha256')).write_bytes((directory / (name + '.sha256')).read_bytes())
                else:
                    require(inspect_archive(directory, target) == inspect_archive(staged, target), 'Published payload differs from staged build')
            elif name + '.sha256' in names:
                require((directory / (name + '.sha256')).read_bytes() == (staged / (name + '.sha256')).read_bytes(), 'Orphan checksum conflict')
        if 'install.sh' in names:
            require((directory / 'install.sh').read_bytes() == source_bytes('install.sh'), 'Published installer conflict')


def authorization():
    require(os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('GITHUB_JOB') == 'publisher', 'Authorization must run in the actual Actions publisher job')
    require(os.environ.get('GITHUB_REPOSITORY') == REPO and os.environ.get('GITHUB_SHA') == SHA, 'Wrong Actions context')
    # A disposable draft exercises contents:write with this job's short-lived token.
    # Drafts do not create refs; assert that invariant before and after deletion.
    probe = f'preflight-{os.environ["GITHUB_RUN_ID"]}-{os.environ["GITHUB_RUN_ATTEMPT"]}'
    require(optional('git/ref/tags/' + probe) is None, 'Probe tag already exists')
    existing = find_release(probe)
    if existing:
        require(existing['draft'] and existing['target_commitish'] == SHA, 'Probe conflict')
        api('releases/' + str(existing['id']), 'DELETE')
    release = api('releases', 'POST', {'tag_name': probe, 'target_commitish': SHA, 'name': probe, 'draft': True})
    try:
        require(release['draft'], 'Probe must stay a draft')
        api('releases/' + str(release['id']), 'PATCH', {'body': 'Disposable non-publishing permissions check.'})
    finally:
        api('releases/' + str(release['id']), 'DELETE')
    require(optional('git/ref/tags/' + probe) is None, 'Unexpected probe tag')
    print('Publisher contents:write verified by draft create/update/delete; no tag or assets created.')


def evidence(kind):
    metadata()
    runs = json.loads(gh('run', 'list', '--repo', 'github.com/' + REPO, '--commit', SHA, '--limit', '100',
                        '--json', 'databaseId,workflowName,headSha,headBranch,event,status,conclusion'))
    for workflow in ['ci', 'release']:
        event = 'workflow_dispatch' if kind == 'authorization' and workflow == 'release' else 'push'
        candidates = [r for r in runs if r['workflowName'] == workflow and r['headSha'] == SHA and r['event'] == event and not r['headBranch'].startswith('v')]
        require(candidates, 'Missing exact-commit ' + workflow + ' ' + event + ' run')
        run = max(candidates, key=lambda r: r['databaseId'])
        require(run['status'] == 'completed' and run['conclusion'] == 'success', 'Latest ' + workflow + ' run did not succeed')
        detail = json.loads(gh('run', 'view', str(run['databaseId']), '--repo', 'github.com/' + REPO, '--json', 'headSha,jobs,conclusion'))
        require(detail['headSha'] == SHA and detail['conclusion'] == 'success', 'Wrong/failed run')
        require(all(j['conclusion'] == 'success' for j in detail['jobs']), 'Incomplete jobs')
        if workflow == 'release':
            require({j['name'] for j in detail['jobs']} == {'publisher'} | {f'build ({t})' for t in TARGETS}, 'Missing release jobs')
            publisher = next(j for j in detail['jobs'] if j['name'] == 'publisher')
            for name in ['Check publisher authorization', 'Validate version and all staged assets']:
                require(any(s['name'] == name and s['conclusion'] == 'success' for s in publisher['steps']), 'Missing publisher evidence')
            artifacts = api(f'actions/runs/{run["databaseId"]}/artifacts?per_page=100')['artifacts']
            require({a['name'] for a in artifacts if not a['expired']} == {'release-' + t for t in TARGETS}, 'Missing build artifacts')
            with tempfile.TemporaryDirectory() as d:
                for a in artifacts:
                    raw = gh('api', '--hostname', 'github.com', f'repos/{REPO}/actions/artifacts/{a["id"]}/zip')
                    with zipfile.ZipFile(io.BytesIO(raw)) as z:
                        for n in z.namelist():
                            require('/' not in n and n in expected_names(), 'Invalid Actions artifact')
                            Path(d, n).write_bytes(z.read(n))
                Path(d, 'install.sh').write_bytes(source_bytes('install.sh'))
                staged = Path(d)
                validate(staged)
                if kind == 'publication-inputs':
                    require(validate(staged) == validate(Path('dist')), 'Tag build payload differs from successful preflight; refusing uploads')
                if kind in ['version', 'published']:
                    release = release_state()
                    if kind == 'published':
                        require(release and not release['draft'], 'Release is not published')
                    if release:
                        compare_remote(release, staged, complete=not release['draft'])
            print(f'{kind}: exact commit {SHA}, release run {run["databaseId"]}, publisher and all platform artifacts verified')


def publish(staged):
    metadata()
    require(os.environ.get('GITHUB_REF') == 'refs/tags/' + TAG, 'Publication requires explicit matching tag push')
    validate(staged)
    release = release_state()
    if release and not release['draft']:
        compare_remote(release, staged, True)
        return
    authorization()  # Fresh credential check immediately before uploads.
    if not release:
        release = api('releases', 'POST', {'tag_name': TAG, 'target_commitish': SHA, 'name': TAG, 'draft': True, 'generate_release_notes': True})
    compare_remote(release, staged, False, resume=True)
    existing = {a['name'] for a in release['assets']}
    # Never clobber. Existing archive/checksum pairs are verified as unpacked content.
    for name in sorted(expected_names() - existing):
        gh('release', 'upload', TAG, str(staged / name), '--repo', 'github.com/' + REPO)
        uploaded = api('releases/' + str(release['id']))
        asset = next(a for a in uploaded['assets'] if a['name'] == name)
        raw = gh('api', '--hostname', 'github.com', '-H', 'Accept: application/octet-stream', f'repos/{REPO}/releases/assets/{asset["id"]}')
        require(raw == (staged / name).read_bytes(), 'Upload integrity mismatch')
    compare_remote(api('releases/' + str(release['id'])), staged, True)
    api('releases/' + str(release['id']), 'PATCH', {'draft': False, 'make_latest': 'true'})
    compare_remote(api('releases/' + str(release['id'])), staged, True)


if __name__ == '__main__':
    try:
        mode = sys.argv[1]
        if mode == 'package':
            package(sys.argv[2], Path('dist'))
        elif mode == 'authorize':
            metadata()
            release = release_state()
            if release and not release['draft']:
                validate(Path('dist'))
                compare_remote(release, Path('dist'), True)
            else:
                authorization()
        elif mode == 'staged':
            metadata()
            validate(Path('dist'))
            if os.environ.get('GITHUB_EVENT_NAME') == 'workflow_dispatch' or (
                os.environ.get('GITHUB_EVENT_NAME') == 'push'
                and os.environ.get('GITHUB_REF', '').startswith('refs/tags/')
            ):
                evidence('publication-inputs')
            release = release_state()
            if release:
                compare_remote(release, Path('dist'), not release['draft'])
        elif mode == 'publish':
            publish(Path('dist'))
        elif mode in ['build', 'authorization', 'version', 'published']:
            evidence(mode)
        else:
            raise RuntimeError('Unknown release mode')
    except (RuntimeError, subprocess.CalledProcessError, KeyError, ValueError, OSError) as error:
        sys.exit(str(error))
