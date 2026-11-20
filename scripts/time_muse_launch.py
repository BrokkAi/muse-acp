#!/usr/bin/env python3
"""Record where one `muse serve` launch spends its time.

Temporary diagnostic for the nightly macOS job, where each live test takes
about 38 seconds against about one second on Linux. It runs the MSP client
crate's live test once with `MUSE_CLI` pointed at a pass-through proxy and
prints:

- how long a bare `muse --version` takes;
- a timeline of the MSP conversation, with method names and ids but never
  payloads, and the longest silences in it;
- the Muse process tree while the test waits and, on macOS, a `sample` of the
  Muse process.

Run it the way the nightly job does (macOS or Linux):

    MUSE_ACP_LOOPBACK=1 MUSE_CLI=/path/to/muse python3 scripts/time_muse_launch.py

It reports what it saw and exits 0 whether or not the test passed.
"""

import glob
import json
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

TEST = ['cargo', 'test', '--locked', '-p', 'muse-msp-client', '--test', 'live_muse']
TEST_LIMIT = 240          # seconds before the test is killed
SNAPSHOTS = [5, 15, 25, 35, 50, 80, 120]
SAMPLES = [10, 28]        # seconds into the test; macOS only
SILENCE = 1.0             # a gap this long is called out in the timeline


def say(text=''):
    print(text, flush=True)


def heading(title):
    say()
    say('=== ' + title + ' ===')


# --- proxy mode: stand in for `muse`, forward every byte, log the timing ---

def describe(kind, line):
    """What a logged line was, without its payload."""
    if kind == 'stderr':
        return line.decode('utf-8', 'replace').rstrip()[:200]
    try:
        frame = json.loads(line)
    except ValueError:
        return 'non-JSON line, %d bytes' % len(line)
    if not isinstance(frame, dict):
        return 'JSON %s, %d bytes' % (type(frame).__name__, len(line))
    ident = ' id=%s' % frame['id'] if 'id' in frame else ''
    if 'method' in frame:
        return '%s%s' % (frame['method'], ident)
    if 'error' in frame:
        error = frame['error'] if isinstance(frame['error'], dict) else {}
        return 'error%s code=%s' % (ident, error.get('code'))
    if 'result' in frame:
        return 'result' + ident
    return 'frame with keys %s' % sorted(frame)[:6]


def proxy(real, log_dir, args):
    log = open(os.path.join(log_dir, 'launch-%d.jsonl' % os.getpid()), 'a')
    lock = threading.Lock()

    def record(kind, what=None):
        with lock:
            log.write(json.dumps({'t': time.time(), 'kind': kind, 'what': what}) + '\n')
            log.flush()

    child = subprocess.Popen([real] + args, stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    record('launch', {'pid': child.pid, 'argv': [os.path.basename(real)] + args})

    def forward(signum, _frame):
        record('signal', signum)
        try:
            child.send_signal(signum)
        except OSError:
            pass

    signal.signal(signal.SIGTERM, forward)
    signal.signal(signal.SIGINT, forward)

    def pump(source, sink, kind, close):
        pending = b''
        while True:
            try:
                chunk = os.read(source, 65536)
            except OSError:
                break
            if not chunk:
                break
            # Forward first, so logging can never hold a frame back.
            view = memoryview(chunk)
            try:
                while view:
                    view = view[os.write(sink, view):]
            except OSError:
                break
            pending += chunk
            lines = pending.split(b'\n')
            pending = lines.pop()
            for line in lines:
                if line.strip():
                    record(kind, describe(kind, line))
        if pending.strip():
            record(kind, describe(kind, pending))
        record(kind + ' closed')
        if close is not None:
            try:
                close()
            except OSError:
                pass

    # Reading our stdin can block after the host is gone, hence a daemon.
    threading.Thread(target=pump, daemon=True,
                     args=(0, child.stdin.fileno(), 'to host', child.stdin.close)).start()
    readers = [threading.Thread(target=pump, args=(child.stdout.fileno(), 1, 'from host', None)),
               threading.Thread(target=pump, args=(child.stderr.fileno(), 2, 'stderr', None))]
    for reader in readers:
        reader.start()
    for reader in readers:
        reader.join()
    status = child.wait()
    record('exit', status)
    # A host killed by a signal reports a negative status.
    return status if status >= 0 else 128 - status


# --- driver mode ---

def run_quiet(command, limit=30):
    """Output of a short command, or the reason there is none."""
    try:
        done = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              timeout=limit, universal_newlines=True)
        return done.stdout.rstrip()
    except (OSError, subprocess.SubprocessError) as error:
        return '(%s: %s)' % (command[0], error)


def launches(log_dir):
    """Each proxied launch as its list of records, oldest launch first."""
    found = []
    for path in glob.glob(os.path.join(log_dir, 'launch-*.jsonl')):
        with open(path) as handle:
            records = []
            for line in handle:
                try:
                    records.append(json.loads(line))
                except ValueError:
                    pass  # a line still being written
        if records:
            found.append(records)
    return sorted(found, key=lambda records: records[0]['t'])


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def muse_pids(log_dir):
    pids = []
    for records in launches(log_dir):
        if records[0]['kind'] == 'launch' and not any(r['kind'] == 'exit' for r in records):
            pid = records[0]['what']['pid']
            if alive(pid):
                pids.append(pid)
    return pids


def snapshot(root, log_dir, at):
    """Print the processes under the test and under Muse."""
    rows = {}
    for line in run_quiet(['ps', '-axo', 'pid=,ppid=,etime=,pcpu=,command=']).splitlines():
        parts = line.split(None, 4)
        if len(parts) == 5 and parts[0].isdigit() and parts[1].isdigit():
            rows[int(parts[0])] = (int(parts[1]), parts[2], parts[3], parts[4])
    wanted = set()
    frontier = [root] + muse_pids(log_dir)
    while frontier:
        pid = frontier.pop()
        if pid in wanted:
            continue
        wanted.add(pid)
        frontier.extend(child for child, row in rows.items() if row[0] == pid)
    say('--- processes at +%ds (pid ppid elapsed %%cpu command) ---' % at)
    for pid in sorted(wanted & set(rows)):
        ppid, elapsed, cpu, command = rows[pid]
        say('  %6d %6d %9s %5s  %s' % (pid, ppid, elapsed, cpu, command[:160]))


def sample(log_dir, at):
    """macOS: print where the Muse process is stopped."""
    pids = muse_pids(log_dir)
    if not shutil.which('sample') or not pids:
        return
    for pid in pids[:1]:
        out = os.path.join(log_dir, 'sample-%d-%d.txt' % (at, pid))
        command = ['sample', str(pid), '2', '-mayDie', '-file', out]
        result = run_quiet(command, limit=40)
        if not os.path.exists(out):
            result += '\n' + run_quiet(['sudo', '-n'] + command, limit=40)
        say('--- sample of muse pid %d at +%ds ---' % (pid, at))
        if os.path.exists(out):
            with open(out, errors='replace') as handle:
                for number, line in enumerate(handle):
                    if number >= 170:
                        say('  ... (truncated)')
                        break
                    say('  ' + line.rstrip())
        else:
            say('  no sample: ' + result.replace('\n', ' | '))


def timeline(records):
    start = records[0]['t']
    argv = records[0]['what'].get('argv') if records[0]['kind'] == 'launch' else None
    say('launch: %s' % ' '.join(argv or ['?']))
    gaps = []
    for before, after in zip(records, records[1:]):
        gaps.append((after['t'] - before['t'], before, after))
    previous = start
    for record in records:
        if record['t'] - previous >= SILENCE:
            say('            ... %.1fs of silence ...' % (record['t'] - previous))
        what = '' if record['what'] is None else record['what']
        if record['kind'] == 'launch':
            what = 'pid %s' % record['what'].get('pid')
        say('  +%8.3fs  %-16s %s' % (record['t'] - start, record['kind'], what))
        previous = record['t']
    say('  total: %.3fs from launch to the last record' % (records[-1]['t'] - start))
    say('  longest silences:')
    for length, before, after in sorted(gaps, key=lambda gap: -gap[0])[:5]:
        say('    %7.3fs  after [%s %s]  before [%s %s]' % (
            length, before['kind'], '' if before['kind'] == 'launch' else before['what'],
            after['kind'], after['what']))


def drive():
    muse = os.environ.get('MUSE_CLI') or shutil.which('muse')
    if not muse or not os.path.isfile(muse):
        say('MUSE_CLI does not name a Muse binary; nothing to time.')
        return 1
    muse = os.path.abspath(muse)

    heading('Host')
    say(run_quiet(['uname', '-a']))
    if sys.platform == 'darwin':
        say(run_quiet(['sw_vers']))
        say(run_quiet(['sysctl', '-n', 'machdep.cpu.brand_string', 'hw.ncpu', 'hw.memsize']))
        say('codesign: ' + run_quiet(['codesign', '-dv', '--verbose=2', muse]).replace('\n', ' | '))
        say('xattr: ' + (run_quiet(['xattr', '-l', muse]) or '(none)'))
        say('spctl: ' + run_quiet(['spctl', '--assess', '--type', 'execute', '-vv', muse]).replace('\n', ' | '))
    say('muse: %s (%d bytes)' % (muse, os.path.getsize(muse)))

    heading('Bare launch: muse --version, three times')
    for attempt in (1, 2, 3):
        began = time.time()
        output = run_quiet([muse, '--version'], limit=120)
        say('  run %d: %.3fs  %s' % (attempt, time.time() - began, output.replace('\n', ' | ')[:120]))

    heading('Build the test')
    began = time.time()
    built = subprocess.run(TEST + ['--no-run'])
    say('  cargo exited %d after %.1fs' % (built.returncode, time.time() - began))
    if built.returncode != 0:
        return 0

    work = tempfile.mkdtemp(prefix='muse-launch-', dir=os.environ.get('RUNNER_TEMP'))
    # The test clears the environment before launching Muse, so the wrapper
    # carries absolute paths instead of reading variables.
    wrapper = os.path.join(work, 'muse')
    with open(wrapper, 'w') as handle:
        handle.write('#!/bin/sh\nexec %s %s --proxy %s %s -- "$@"\n' % (
            shlex.quote(sys.executable), shlex.quote(os.path.abspath(__file__)),
            shlex.quote(muse), shlex.quote(work)))
    os.chmod(wrapper, 0o755)

    heading('One live test through the proxy')
    began = time.time()
    test = subprocess.Popen(TEST + ['--', '--nocapture'], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, universal_newlines=True,
                            env=dict(os.environ, MUSE_CLI=wrapper, MUSE_ACP_LOOPBACK='1'))

    def echo():
        for line in test.stdout:
            say('  [+%6.1fs] %s' % (time.time() - began, line.rstrip()))

    reader = threading.Thread(target=echo)
    reader.start()
    todo = sorted([(at, 'snapshot') for at in SNAPSHOTS] + [(at, 'sample') for at in SAMPLES])
    while test.poll() is None:
        elapsed = time.time() - began
        if elapsed > TEST_LIMIT:
            say('  the test is still running after %ds; killing it' % TEST_LIMIT)
            test.kill()
            break
        if todo and elapsed >= todo[0][0]:
            at, action = todo.pop(0)
            if action == 'snapshot':
                snapshot(test.pid, work, at)
            elif sys.platform == 'darwin':
                sample(work, at)
        time.sleep(0.25)
    test.wait()
    reader.join()
    say('  the test exited %d after %.1fs' % (test.returncode, time.time() - began))

    found = launches(work)
    heading('MSP timeline (%d launch%s)' % (len(found), '' if len(found) == 1 else 'es'))
    if not found:
        say('  the proxy was never started: the test did not launch Muse')
    for records in found:
        timeline(records)
        say()
    shutil.rmtree(work, ignore_errors=True)
    return 0


def main(argv):
    if len(argv) >= 4 and argv[0] == '--proxy' and argv[3] == '--':
        return proxy(argv[1], argv[2], argv[4:])
    if argv:
        say(__doc__)
        return 2
    return drive()


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
