#!/usr/bin/env python3
"""Disposable real-server acceptance: python3 extensions/tests/smoke.py [--binary PATH].

Requires the acknowledged extension review, Swamp, Git, and an unrestricted local
socket environment. All Git changes and approvals are confined to a fresh /tmp repo.
The repository and command transcripts remain available after the session stops.
"""
import argparse
import fcntl
import pty
import re
import struct
import termios
import threading
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default='target/debug/zellij')
parser.add_argument('--firecracker-only', action='store_true')
parser.add_argument('--recovery-only', action='store_true')
args = parser.parse_args()
source = Path(__file__).resolve().parents[2]
root = Path(tempfile.mkdtemp(prefix='zellij-workflow-acceptance-'))
main, topic = root / 'main', root / 'topic'
main.mkdir()
binary = root / 'zellij'
# Keep the tested executable stable if another checkout rebuilds during the run.
os.link(Path(args.binary).resolve(), binary)
session = 'workflow-acceptance'
env = os.environ.copy()
for name in ('ZELLIJ', 'ZELLIJ_SESSION_NAME', 'ZELLIJ_PANE_ID', 'ZELLIJ_PANE_REF', 'ZELLIJ_CONFIG_FILE', 'ZELLIJ_CONFIG_DIR', 'SWAMP_REPO_DIR', 'SWAMP_SERVER_URL'):
    env.pop(name, None)
for name, directory in [('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                        ('XDG_CACHE_HOME', 'cache'), ('XDG_RUNTIME_DIR', 'runtime'),
                        ('ZELLIJ_SOCKET_DIR', 'sock')]:
    env[name] = str(root / directory)
    (root / directory).mkdir()
env['SWAMP_SMOKE_ZELLIJ'] = str(binary)
real_swamp = shutil.which('swamp')
assert real_swamp, 'Swamp is required'
fixture_bin = root / 'bin'
fixture_bin.mkdir()
wrapper = fixture_bin / 'swamp'
wrapper.write_text('''#!/usr/bin/env python3
import os, subprocess, sys
from pathlib import Path
marker = Path(os.environ['SWAMP_SMOKE_ROOT']) / 'interrupt-after-approval'
if sys.argv[1:3] == ['workflow', 'approve'] and marker.exists():
    subprocess.run([os.environ['SWAMP_SMOKE_REAL']] + sys.argv[1:], check=True)
    marker.unlink()
    sys.exit(91)  # the approval landed, but answer delivery was interrupted
os.execv(os.environ['SWAMP_SMOKE_REAL'], [os.environ['SWAMP_SMOKE_REAL']] + sys.argv[1:])
''')
wrapper.chmod(0o755)
env.update(PATH=str(fixture_bin) + os.pathsep + env['PATH'],
           SWAMP_SMOKE_REAL=real_swamp, SWAMP_SMOKE_ROOT=str(root))
(root / 'environment.json').write_text(json.dumps({k: env[k] for k in ('XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_RUNTIME_DIR','ZELLIJ_SOCKET_DIR','SWAMP_SMOKE_ZELLIJ')}, indent=2))
transcript = (root / 'commands.log').open('w')


def run(*argv, cwd=main, check=True):
    result = subprocess.run([str(x) for x in argv], cwd=cwd, env=env,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=300 if args.firecracker_only else 120)
    transcript.write(json.dumps(list(map(str, argv))) + '\n' + result.stdout + result.stderr + '\n')
    transcript.flush()
    if check and result.returncode:
        raise RuntimeError('{}: {}\n{}'.format(argv, result.stdout, result.stderr))
    return result


def swamp(*argv):
    return json.loads(run('swamp', *argv, '--json').stdout)


def zellij(*argv):
    return run(binary, '--session', session, *argv, cwd=topic if topic.exists() else main).stdout


def bridge(**request):
    result = json.loads(zellij('pipe', '--name', 'swamp', '--plugin', 'zellij:swamp-bridge', '--', json.dumps(request)))
    if 'error' in result:
        raise RuntimeError(result['error'])
    return result['result']


def attached_pane(resource):
    host = next((h for h in json.loads(zellij('pty-host', 'list', '--json')) if h['host_id'] == resource['supervisor']), None)
    return host and host['attached'] and any(p['id'] == host['terminal_id'] and not p['is_plugin'] for p in json.loads(zellij('action', 'list-panes', '--json')))


def until(label, predicate):
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.25)
    raise AssertionError('Timed out: ' + label + '; inspect ' + str(root))


def histories(launch):
    runs = swamp('workflow', 'history', 'search', '--input', 'launch=' + launch)['results']
    for item in runs:
        if item['status'] == 'failed':
            run('swamp', 'report', 'get', '@swamp/workflow-summary', '--workflow', item['workflowName'], '--json', check=False)
            raise AssertionError('Workflow failed: ' + item['runId'])
    return runs


def pending(launch, question):
    for item in histories(launch):
        if item['status'] != 'suspended':
            continue
        records = swamp('data', 'query', 'modelType == "@hammz/question" && attributes.run == ' + json.dumps(item['runId']),
                        '--select', '{"model":modelName,"version":version,"attributes":attributes}')['results']
        latest = {}
        for record in records:
            name = record['attributes']['question']
            if name not in latest or record['version'] > latest[name]['version']:
                latest[name] = record
        q = latest.get(question)
        if q and q['attributes']['state'] == 'pending':
            return q
    return None


def launch(token, **inputs):
    default = dict(agentCommand='/usr/bin/python3', agentArgs=[str(main / 'extensions/tests/stub_agent.py')], gate=['/usr/bin/true'])
    default.update(inputs)
    return zellij('workflow', 'run', '@hammz/zellij-agent-exit', '--launch', token, '--no-view', '--input', json.dumps(default))


def answer(q, value, token):
    a = q['attributes']
    return zellij('workflow', 'answer', a['run'], q['model'], a['question'], value, '--submission', token)


ui_pid = ui_fd = None
ui_output = bytearray()

def attach_ui():
    global ui_pid, ui_fd
    ui_pid, ui_fd = pty.fork()
    if ui_pid == 0:
        os.chdir(str(topic))
        os.execve(str(binary), [str(binary), 'attach', session], dict(env, TERM='xterm-256color'))
    fcntl.ioctl(ui_fd, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 140, 0, 0))
    def drain():
        with (root / 'terminal.bin').open('wb') as output:
            try:
                while True:
                    data = os.read(ui_fd, 65536)
                    if not data: break
                    output.write(data)
                    output.flush()
                    ui_output.extend(data)
                    if b'\x1b[6n' in data: os.write(ui_fd, b'\x1b[1;1R')
            except OSError:
                pass
    threading.Thread(target=drain, daemon=True).start()


print('Acceptance fixtures:', root, flush=True)
run('git', 'init', '-b', 'main')
run('git', 'config', 'user.name', 'Workflow test')
run('git', 'config', 'user.email', 'workflow@example.invalid')
(main / 'file').write_text('base\n')
run('git', 'add', 'file')
run('git', 'commit', '-m', 'base')
base = run('git', 'rev-parse', 'HEAD').stdout.strip()
run('git', 'worktree', 'add', '-b', 'topic', topic)
(topic / 'file').write_text('topic\n')
swamp('repo', 'init', str(main), '--tool', 'none')
run('swamp', 'extension', 'pull', '@swamp/git', '--json')
for directory in ('extensions', 'workflows'):
    run('cp', '-R', source / directory, main)
(main / '.zellij').mkdir()
shutil.copy(source / '.zellij/.swamp-workflows.kdl', main / '.zellij/.swamp-workflows.kdl')
(main / '.git/info/exclude').write_text('.swamp*\n.gitignore\nextensions/\nworkflows/\nmodels/\n.zellij/\nAGENTS.md\nCLAUDE.md\n')
for name in ('agent-work', 'review-integration', 'question', 'agent-exit'):
    swamp('workflow', 'validate', '@hammz/zellij-' + name)
(root / 'config/zellij').mkdir(exist_ok=True)
(root / 'config/zellij/config.kdl').write_text('show_release_notes false\nshow_startup_tips false\nfirecracker_provider ' + json.dumps(str(source / 'target/debug/zellij-firecracker')) + '\n')
if args.firecracker_only:
    provider_config = source / 'target/firecracker-test/config/zellij/firecracker.json'
    shutil.copy(provider_config, root / 'config/zellij/firecracker.json')
    # Config names the existing immutable image; VM state stays in this fixture.
run(binary, 'attach', '--create-background', session)
try:
    if args.firecracker_only:
        shutil.copy(source / 'extensions/tests/stub_agent.py', topic / 'stub_agent.py')
        vm = bridge(method='agent_spawn', work='vm', key='worker', spec=dict(command='/usr/bin/python3', args=['stub_agent.py', '--exit'], cwd=str(topic), source_directory=str(topic), firecracker='workflow-vm', firecracker_image='zellij-dev', automatic_firecracker_vm=True))
        assert vm['supervisor'].startswith('remote:')
        bridge(method='agent_prompt', work='vm', key='edit', agent='worker', prompt='guest edit')
        until('original guest exits', lambda: bridge(method='observe', work='vm', key='worker').get('processExited'))
        assert not (topic / 'guest.txt').exists()
        assert not bridge(method='vm_inspect', work='vm', key='inspect', agent='worker')['clean']
        target_vm = vm['target']['Firecracker']
        vm_name = target_vm.get('vm_name') or target_vm['vm']
        vm_inputs = dict(agentCommand='/usr/bin/python3', agentArgs=['stub_agent.py'], firecracker=vm_name, gate=['/usr/bin/true'])
        zellij('workflow', 'run', '@hammz/zellij-agent-work', '--launch', 'reuse-vm', '--no-view', '--input', json.dumps(vm_inputs))
        until('original VM synchronized by workflow', lambda: histories('reuse-vm') and all(r['status'] == 'succeeded' for r in histories('reuse-vm')))
        reused = bridge(method='observe', work='reuse-vm', key='worker')
        reused_vm = reused['target']['Firecracker']
        assert (reused_vm.get('vm_name') or reused_vm['vm']) == vm_name
        assert (topic / 'guest.txt').read_text() == 'guest\n'
        (topic / 'file').write_text('host conflict\n')
        launch('conflict-vm', **vm_inputs, task='guest conflict')
        def conflict_failed():
            runs = swamp('workflow', 'history', 'search', '--input', 'launch=conflict-vm')['results']
            return runs if runs and any(r['status'] == 'failed' for r in runs) and not any(r['status'] == 'running' for r in runs) else None
        failed_runs = until('conflict blocks workflow', conflict_failed)
        for item in failed_runs:
            if item['status'] == 'failed':
                swamp('report', 'get', '@swamp/workflow-summary', '--workflow', item['workflowName'])
            assert item['workflowName'] != '@hammz/zellij-review-integration'
        failed = bridge(method='observe', work='conflict-vm', key='worker')
        failed_vm = failed['target']['Firecracker']
        assert (failed_vm.get('vm_name') or failed_vm['vm']) == vm_name
        assert (topic / 'file').read_text() == 'host conflict\n'
        assert run('git', 'rev-parse', 'HEAD').stdout.strip() == base
        assert topic.exists()
        assert not bridge(method='vm_inspect', work='conflict-vm', key='inspect', agent='worker')['clean']
        print('PASS: exited VM reused with guest-only changes; conflicting sync blocks review, integration and cleanup', flush=True)
        raise SystemExit(0)
    if not args.recovery_only:
        launch('success')
        q = until('unattended merge question', lambda: pending('success', 'decision'))
        assert run('git', 'rev-parse', 'HEAD').stdout.strip() == base
        print('PASS: unattended execution suspends before integration', flush=True)
        before = len(histories('success'))
        launch('success')
        assert len(histories('success')) == before, 'Duplicate launch dispatched a second run'
        attach_ui()
        for _ in range(2):
            render_start = len(ui_output)
            zellij('workflow', 'show', q['attributes']['run'])
            panes = until('workflow view', lambda: [p for p in json.loads(zellij('action', 'list-panes', '--json')) if p.get('plugin_url') == 'zellij:swamp-workflow'])
            pane = 'plugin_' + str(panes[0]['id'])
            until('pending question rendered', lambda: b'Choose merge' in re.sub(rb'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]', b'', bytes(ui_output[render_start:])))
            zellij('action', 'close-pane', '--pane-id', pane)
            assert pending('success', 'decision')
        zellij('workflow', 'run', '@hammz/zellij-question', '--launch', 'view-isolation', '--no-view', '--input', json.dumps(dict(kind='text', prompt='Separate view?', answers=[])))
        second = until('second view question', lambda: pending('view-isolation', 'question'))
        views = []
        for question in (q, second):
            before_ids = {p['id'] for p in json.loads(zellij('action', 'list-panes', '--json')) if p['is_plugin']}
            # open_view parses exactly one JSON acknowledgement; duplicate replies fail this call.
            zellij('workflow', 'show', question['attributes']['run'])
            panes = json.loads(zellij('action', 'list-panes', '--json'))
            new = [p for p in panes if p['is_plugin'] and p['id'] not in before_ids and p.get('plugin_url') == 'zellij:swamp-workflow']
            assert len(new) == 1
            views.append(('plugin_' + str(new[0]['id']), question['attributes']['run']))
        def assert_attachment(pane, run_id):
            start = len(ui_output)
            zellij('action', 'focus-pane-id', pane)
            until('view retains its own run', lambda: ('Run ' + run_id + ':').encode() in bytes(ui_output[start:]))
        for pane, run_id in views:
            assert_attachment(pane, run_id)
        before = {(p['id'], p['is_plugin']) for p in json.loads(zellij('action', 'list-panes', '--json'))}
        for destination in ('plugin_4294967295', 'plugin_bad'):
            failed = run(binary, '--session', session, 'pipe', '--name', 'show', '--plugin', destination, '--', '{}', check=False)
            assert failed.returncode and 'destination' in failed.stderr.lower(), failed
        assert {(p['id'], p['is_plugin']) for p in json.loads(zellij('action', 'list-panes', '--json'))} == before
        for pane, run_id in views:
            assert_attachment(pane, run_id)
            zellij('action', 'close-pane', '--pane-id', pane)
        zellij('workflow', 'cancel', second['attributes']['run'])
        print('PASS: two views retain distinct runs; missing and malformed destinations fail without launch or broadcast', flush=True)
        # Detach the actual terminal client; a CLI action has its own client identity.
        os.kill(ui_pid, signal.SIGHUP)
        until('terminal client detached', lambda: os.waitpid(ui_pid, os.WNOHANG)[0])
        ui_pid = None
        print('PASS: interactive view renders its question; detach/reopen preserves the pending run and answer', flush=True)
        # Move the target after review; approval must start another review, not integrate.
        run('git', 'commit', '--allow-empty', '-m', 'target changed after review')
        target = run('git', 'rev-parse', 'HEAD').stdout.strip()
        answer(q, 'merge', 'stale-merge')
        fresh = until('fresh review after stale evidence', lambda: pending('success', 'decision'))
        assert fresh['attributes']['run'] != q['attributes']['run']
        assert run('git', 'rev-parse', 'HEAD').stdout.strip() == target
        answer(fresh, 'merge', 'fresh-merge')
        cleanup = until('separate cleanup approval', lambda: pending('success', 'cleanup'))
        assert run('git', 'rev-parse', 'HEAD').stdout == run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
        answer(cleanup, 'keep', 'keep-fixture')
        until('successful review completion', lambda: all(r['status'] == 'succeeded' for r in histories('success')))
        report = swamp('report', 'get', '@hammz/workflow', '--workflow', '@hammz/zellij-review-integration')['json']
        assert {'review-current', 'test-current', 'integration-current'} <= {r['name'] for r in report['resources']}
        print('PASS: stale evidence re-enters review; approved integration and keep succeed', flush=True)

        launch('repairs', gate=['/usr/bin/false'], agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--malformed'])
        instructions = until('instructions after both repairs', lambda: pending('repairs', 'repair-limit'))
        history = swamp('workflow', 'history', 'get', instructions['attributes']['run'])
        steps = {s['name']: s['status'] for job in history['jobs'] for s in job['steps']}
        assert steps['round-1-repair'] == 'succeeded' and steps['round-2-repair'] == 'succeeded'
        assert run('git', 'rev-parse', 'HEAD').stdout == run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
        print('PASS: malformed findings and failed tests exhaust exactly two repairs, then suspend', flush=True)

        zellij('workflow', 'run', '@hammz/zellij-question', '--launch', 'recovery', '--no-view', '--input', json.dumps(dict(kind='text', prompt='Instructions?', answers=[])))
        question = until('text question', lambda: pending('recovery', 'question'))
        (root / 'interrupt-after-approval').touch()
        failed = run(binary, '--session', session, 'workflow', 'answer', question['attributes']['run'], question['model'], 'question', 'Recorded instructions', '--submission', 'recorded-once', check=False)
        assert failed.returncode, 'The injected interruption did not occur'
        swamp('report', 'get', '@swamp/method-summary', '--model', question['model'])
        swamp('model', 'method', 'run', question['model'], 'recover', '--input', json.dumps(dict(run=question['attributes']['run'], question='question')))
        until('recorded answer recovery', lambda: all(r['status'] == 'succeeded' for r in histories('recovery')))
        duplicate = run(binary, '--session', session, 'workflow', 'answer', question['attributes']['run'], question['model'], 'question', 'Recorded instructions', '--submission', 'recorded-once', check=False)
        assert duplicate.returncode
        swamp('report', 'get', '@swamp/method-summary', '--model', question['model'])
        print('PASS: interruption after recording/approval recovers once; duplicate answer refused', flush=True)

    owned = bridge(method='agent_spawn', work='restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--wait'], cwd=str(topic), source_directory=str(topic)))
    bridge(method='agent_prompt', work='restart', key='pending', agent='worker', prompt='Wait for recovery')
    zellij('action', 'start-or-reload-plugin', 'zellij:swamp-bridge')
    observed = until('dispatcher restart', lambda: (lambda o: o if o['outcome'] == 'unknown' else None)(bridge(method='observe', work='restart', key='pending')))
    reconciled = bridge(method='reconcile', work='restart', key='reconcile', operation='worker')
    assert reconciled['supervisor'] == owned['supervisor'] and reconciled['pane'] == owned['pane']
    assert observed['outcome'] == 'unknown'
    assert bridge(method='agent_spawn', work='restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--wait'], cwd=str(topic), source_directory=str(topic)))['pane'] == owned['pane']
    if not args.recovery_only:
        zellij('workflow', 'show', instructions['attributes']['run'])
        print('PASS: dispatcher restart preserves ownership and uncertain prompt; no repeated spawn', flush=True)
        zellij('workflow', 'cancel', instructions['attributes']['run'])
        until('explicit cancellation', lambda: swamp('workflow', 'history', 'get', instructions['attributes']['run'])['status'] == 'cancelled')
        assert topic.exists()
        print('PASS: explicit cancellation preserves resources', flush=True)
    # Crash only this fixture's server. Supervisors must keep the owned processes.
    survivor = bridge(method='agent_spawn', work='server-restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py')], cwd=str(topic), source_directory=str(topic)))
    bridge(method='agent_prompt', work='server-restart', key='before', agent='worker', prompt='Before restart')
    until('completion before server restart', lambda: bridge(method='observe', work='server-restart', key='before')['outcome'] == 'completed')
    server_pids = []
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit(): continue
        try:
            argv = (proc / 'cmdline').read_bytes().split(b'\0')
            if os.path.samefile(str(proc / 'exe'), str(binary)) and b'--server' in argv and any(str(root).encode() in arg for arg in argv):
                server_pids.append(int(proc.name))
        except OSError:
            pass
    assert len(server_pids) == 1, 'Identify exactly the fixture server before interruption'
    os.kill(server_pids[0], signal.SIGKILL)
    until('supervisor survives server crash', lambda: any(h['host_id'] == survivor['supervisor'] and not h['attached'] for h in json.loads(zellij('pty-host', 'list', '--json'))))
    run(binary, 'attach', '--create-background', session)
    until('restored session accepts commands', lambda: json.loads(zellij('action', 'list-tabs', '--json')))
    for resource in (survivor, owned):
        adopted = zellij('pty-host', 'adopt', resource['supervisor'], '--session', session)
        assert 'Adopted pane' in adopted, 'Adoption must receive the server acknowledgement'
        until('adopted pane is present', lambda: attached_pane(resource))
    recovered = bridge(method='reconcile', work='server-restart', key='reconcile', operation='worker')
    assert recovered['supervisor'] == survivor['supervisor'] and recovered['process'] == survivor['process']
    bridge(method='reconcile', work='restart', key='reconcile-again', operation='worker')
    assert bridge(method='observe', work='restart', key='pending')['outcome'] == 'unknown'
    bridge(method='agent_prompt', work='server-restart', key='after', agent='worker', prompt='After restart')
    until('completion after server restart', lambda: bridge(method='observe', work='server-restart', key='after')['outcome'] == 'completed')
    os.kill(recovered['process'], signal.SIGTERM)
    until('adopted managed helper exit', lambda: bridge(method='observe', work='server-restart', key='worker').get('processExited'))
    assert not [p for p in json.loads(zellij('action', 'list-panes', '--json')) if p.get('plugin_url') == 'zellij:agent-exit']
    print('PASS: server crash/adoption retains ownership, resumes verified work, and delivers managed process exit', flush=True)
finally:
    run(binary, 'kill-session', session, check=False)
    if ui_pid: os.waitpid(ui_pid, 0)
    if ui_fd is not None: os.close(ui_fd)
    transcript.close()
    print('Retained evidence:', root, flush=True)
