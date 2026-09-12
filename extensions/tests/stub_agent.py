#!/usr/bin/env python3
"""Deterministic, interactive hook source for disposable workflow smoke tests."""
import json
import shlex
import hashlib
import os
import subprocess
import sys
import termios
import tty
import time

remote = 'ZELLIJ_REMOTE_PANE_TOKEN' in os.environ
binary = 'zellij' if remote else os.environ['SWAMP_SMOKE_ZELLIJ']
settings = termios.tcgetattr(sys.stdin)
tty.setraw(sys.stdin)
print('\x1b[?2004h', end='', flush=True)


def report(hook, **fields):
    payload = dict(hook_event_name=hook, session_id='stub-' + os.environ.get('ZELLIJ_REMOTE_PANE_TOKEN', os.environ.get('ZELLIJ_PANE_ID', '')), cwd=os.getcwd(), **fields)
    kind = os.environ.get('SWAMP_SMOKE_KIND', 'claude')
    command = [binary, '--pty-broker', 'report', kind] if remote else [binary, 'action', 'agent-report', '--kind', kind, '--hook-json']
    subprocess.run(command, input=json.dumps(payload).encode(), check=True)


try:
    report('SessionStart')
    print('STUB READY', flush=True)
    buffer = b''
    while True:
        chunk = os.read(sys.stdin.fileno(), 4096)
        if not chunk:
            break
        buffer += chunk
        if b'\x1b[201~' in buffer:
            before, buffer = buffer.split(b'\x1b[201~', 1)
            prompt = before.split(b'\x1b[200~', 1)[-1].decode()
        elif b'\x1b[200~' not in buffer and (b'\r' in buffer or b'\n' in buffer):
            before, buffer = buffer.replace(b'\r', b'\n').split(b'\n', 1)
            prompt = before.decode().strip()
        else:
            continue
        if not prompt: continue
        report('UserPromptSubmit', prompt=prompt)
        if prompt == 'guest edit':
            with open('guest.txt', 'w') as output: output.write('guest\n')
        if prompt == 'guest conflict':
            with open('file', 'w') as output: output.write('guest conflict\n')
        if prompt.startswith('Resolve only the current rebase conflicts'):
            conflicts = subprocess.check_output(['git', 'diff', '--name-only', '--diff-filter=U', '-z']).decode().split('\0')
            for path in filter(None, conflicts):
                with open(path, 'w') as output: output.write('resolved ' + path + '\n')
                subprocess.run(['git', 'add', '--', path], check=True)
        if prompt.startswith('Fix the failed tests'):
            retrieval = next((line for line in prompt.splitlines() if line.startswith('swamp data query ')), None)
            evidence = prompt
            if retrieval:
                result = json.loads(subprocess.check_output(shlex.split(retrieval)))
                assert len(result['results']) == 1
                evidence = result['results'][0]['summary']
            assert 'failure-evidence' in evidence and 'Test report: FAILED' in evidence
            with open('received-failure-output', 'w') as output: output.write(evidence)
            with open('received-failure-prompt', 'w') as output: output.write(prompt)
        if '--edit-review' in sys.argv and prompt.startswith('Review '):
            with open('reviewer-change', 'w') as output: output.write('reviewer edit\n')
        # A reviewer responds with the required structured findings. The worker leaves
        # the fixture's intended revision unchanged, so the Git stages do the commit.
        if '--wait' in sys.argv:
            continue
        message = 'unstructured findings' if '--malformed' in sys.argv and prompt.startswith('Review ') else '{"findings":[]}'
        if prompt == 'guest edit':
            # The mirror preserves host ownership; trust only this disposable guest checkout.
            message = subprocess.check_output(['git', '-c', 'safe.directory=' + os.getcwd(), 'rev-parse', 'HEAD']).decode().strip()
        if '--inspect-review' in sys.argv and prompt.startswith('Review '):
            assert not remote
            assert not subprocess.check_output(['git', 'status', '--porcelain'])
            inspected = dict(checkout=os.getcwd(), commit=subprocess.check_output(['git', 'rev-parse', 'HEAD']).decode().strip(),
                             contents=hashlib.sha256(b'["","",[]]').hexdigest(),
                             host_file=open('host-only').read())
            message = json.dumps(dict(findings=[dict(severity='info', file='host-only', message=json.dumps(inspected))]))
        if '--findings' in sys.argv and prompt.startswith('Review '):
            message = '{"findings":[{"severity":"warning","file":"file","message":"Unresolved fixture finding"}]}'
        report('Stop', last_assistant_message=message)
        if '--exit' in sys.argv:
            if not remote:
                with open(os.path.join(os.environ['SWAMP_SMOKE_ROOT'], 'exit-' + os.environ['ZELLIJ_PANE_ID']), 'w') as stamp:
                    stamp.write(str(time.monotonic()))
            break
finally:
    termios.tcsetattr(sys.stdin, termios.TCSANOW, settings)
