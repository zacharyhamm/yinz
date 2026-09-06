#!/usr/bin/env python3
"""Deterministic, interactive hook source for disposable workflow smoke tests."""
import json
import os
import subprocess
import sys
import termios
import tty

remote = 'ZELLIJ_REMOTE_PANE_TOKEN' in os.environ
binary = 'zellij' if remote else os.environ['SWAMP_SMOKE_ZELLIJ']
settings = termios.tcgetattr(sys.stdin)
tty.setraw(sys.stdin)
print('\x1b[?2004h', end='', flush=True)


def report(hook, **fields):
    payload = dict(hook_event_name=hook, session_id='stub-' + os.environ.get('ZELLIJ_REMOTE_PANE_TOKEN', os.environ.get('ZELLIJ_PANE_ID', '')), cwd=os.getcwd(), **fields)
    command = [binary, '--pty-broker', 'report', 'claude'] if remote else [binary, 'action', 'agent-report', '--kind', 'claude', '--hook-json']
    subprocess.run(command, input=json.dumps(payload).encode(), check=True)


try:
    report('SessionStart')
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
        # A reviewer responds with the required structured findings. The worker leaves
        # the fixture's intended revision unchanged, so the Git stages do the commit.
        if '--wait' in sys.argv:
            continue
        message = 'unstructured findings' if '--malformed' in sys.argv and prompt.startswith('Review ') else '{"findings":[]}'
        report('Stop', last_assistant_message=message)
        if '--exit' in sys.argv:
            break
finally:
    termios.tcsetattr(sys.stdin, termios.TCSANOW, settings)
