#!/usr/bin/python3
"""FOR-72 SIMULATED harness replay peer. Not a harness and not live evidence.

Stands in for a Codex, Claude Code or OMP executable and replays a transcript
recorded from the real binary (scripts/harness_transcripts.py). Recorded
harness output is emitted in recorded order; before each recorded client frame
the peer reads the adapter's frame and compares it. Placeholders such as
<<uuid:3>> bind to the live values the adapter chose (request IDs, session IDs)
so later output answers the live adapter. A different frame kind is a hard
failure; other client differences are logged as drift.

Files beside this executable: `.transcript.jsonl` (input), `.state.json`
(bindings and session count across processes), `.errors`, `.drift`, `.done`.
"""
import json
import os
import re
import sys
import uuid

ME = os.path.abspath(sys.argv[0])
ROOT = os.path.dirname(ME)
PLACEHOLDER = re.compile(r'<<[a-z-]+(?::\d+)?>>')
DISCRIMINATORS = ('type', 'method', 'subtype', 'command')


def note(suffix, message):
    with open(ME + suffix, 'a') as out:
        out.write(message + '\n')


def load_state():
    try:
        with open(ME + '.state.json') as file:
            return json.load(file)
    except FileNotFoundError:
        encoded = ''.join(c if c.isascii() and c.isalnum() else '-' for c in ROOT)
        return {'sessions': 0, 'binds': {'<<root>>': ROOT, '<<root-encoded>>': encoded, '<<home>>': '/nonexistent-home',
                                         '<<host>>': 'replay-host', '<<provider>>': 'http://127.0.0.1:9', '<<user>>': 'replay-user', '<<redacted>>': 'redacted'}}


def save_state():
    temporary = ME + '.state.tmp'
    with open(temporary, 'w') as out:
        json.dump(state, out)
    os.replace(temporary, ME + '.state.json')


def fill(value):
    """Substitute bound placeholders; unbound UUID placeholders get fresh values."""
    if isinstance(value, dict):
        return {fill(k): fill(v) for k, v in value.items()}
    if isinstance(value, list):
        return [fill(v) for v in value]
    if isinstance(value, str):
        def one(match):
            name = match.group(0)
            if name not in binds and name.startswith('<<uuid:'):
                binds[name] = str(uuid.uuid4())
            return binds.get(name, name)
        return PLACEHOLDER.sub(one, value)
    return value


def bind(recorded, live, path, drift, hard):
    if isinstance(recorded, dict) and isinstance(live, dict):
        for key in DISCRIMINATORS:
            if key in recorded and isinstance(recorded[key], str) and recorded.get(key) != live.get(key):
                hard.append(f'{path}/{key}: recorded {recorded[key]!r}, adapter sent {live.get(key)!r}')
        for key in sorted(set(recorded) | set(live)):
            if key not in live:
                drift.append(f'{path}/{key}: missing from the adapter frame')
            elif key not in recorded:
                drift.append(f'{path}/{key}: not in the recorded frame')
            else:
                bind(recorded[key], live[key], f'{path}/{key}', drift, hard)
    elif isinstance(recorded, list) and isinstance(live, list):
        if len(recorded) != len(live):
            drift.append(f'{path}: {len(recorded)} recorded items, {len(live)} sent')
        for index, (r, v) in enumerate(zip(recorded, live)):
            bind(r, v, f'{path}/{index}', drift, hard)
    elif isinstance(recorded, str) and isinstance(live, str) and PLACEHOLDER.search(recorded):
        parts, names, position = [], [], 0
        for match in PLACEHOLDER.finditer(recorded):
            parts.append(re.escape(recorded[position:match.start()]))
            name = match.group(0)
            if name == '<<redacted>>':
                parts.append('.*?')
            elif name in binds:
                parts.append(re.escape(binds[name]))
            else:
                parts.append('(.+?)')
                names.append(name)
            position = match.end()
        parts.append(re.escape(recorded[position:]))
        found = re.fullmatch(''.join(parts), live, re.S)
        if not found:
            drift.append(f'{path}: adapter value does not fit {recorded!r}')
            return
        for name, value in zip(names, found.groups()):
            binds.setdefault(name, value)
    elif recorded != live:
        drift.append(f'{path}: recorded value differs')


def emit(frame):
    line = frame['raw'] if set(frame) == {'raw'} else json.dumps(fill(frame), separators=(',', ':'))
    sys.stdout.write(line + '\n')
    sys.stdout.flush()


def fail(message):
    note('.errors', f'session {session}: {message}')
    save_state()
    raise SystemExit(3)


with open(ME + '.transcript.jsonl') as file:
    lines = [json.loads(line) for line in file if line.strip()]
header, entries = lines[0], lines[1:]
state = load_state()
binds = state['binds']
argv = sys.argv[1:]
for recorded in header.get('execs', []):
    if fill(recorded['argv']) == argv:
        sys.stdout.write(recorded['stdout'])
        sys.stdout.flush()
        raise SystemExit(recorded['code'])

state['sessions'] += 1
session = state['sessions']
mine = [e for e in entries if e['session'] == session]
if not mine:
    fail(f'no recorded session for invocation {session}: {argv[:4]}')


def fits(recorded, live):
    """Whether a live client frame can be this recorded frame: the same kinds
    and no conflict with a value bound earlier. Binds nothing."""
    saved = dict(binds)
    drift, hard = [], []
    bind(recorded, live, 'frame', drift, hard)
    binds.clear()
    binds.update(saved)
    return not hard and not any('does not fit' in d for d in drift)


# Independent client frames may arrive in another order than recorded (for
# example a control reply racing a new user input). Match each live frame to
# the first unconsumed recorded client frame of this session that it fits.
consumed = set()
for index, entry in enumerate(mine):
    direction = entry['dir']
    if direction == 'start':
        drift, hard = [], []
        bind(entry['argv'], argv, 'argv', drift, hard)
        for item in drift:
            note('.drift', f'session {session}: {item}')
        save_state()
    elif direction == 'recv':
        emit(entry['frame'])
    elif direction == 'send':
        while index not in consumed:
            raw = sys.stdin.readline()
            if not raw:
                note('.drift', f'session {session}: adapter closed input before a recorded frame')
                save_state()
                raise SystemExit(0)
            try:
                live = json.loads(raw)
            except ValueError:
                fail('adapter sent a frame that is not JSON')
            target = next((j for j in range(index, len(mine)) if mine[j]['dir'] == 'send' and j not in consumed
                           and fits(mine[j]['frame'], live)), index)
            consumed.add(target)
            drift, hard = [], []
            bind(mine[target]['frame'], live, 'frame', drift, hard)
            save_state()
            if hard:
                fail('; '.join(hard))
            for item in drift:
                note('.drift', f'session {session}: {item}')
    elif direction == 'file':
        path = fill(entry['path'])
        os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, 'w') as out:
            out.write(fill(entry['content']))
note('.done', f'session {session} complete')
save_state()
for raw in sys.stdin:
    try:
        kind = [json.loads(raw).get(k) for k in DISCRIMINATORS]
    except ValueError:
        kind = ['invalid']
    note('.drift', f'session {session}: unrecorded frame after the transcript ended: {[k for k in kind if k]}')
