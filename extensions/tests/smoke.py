#!/usr/bin/env python3
"""Disposable real-server acceptance: python3 extensions/tests/smoke.py [--binary PATH].

Requires the acknowledged extension review, Swamp, Git, and an unrestricted local
socket environment. All Git changes and approvals are confined to a fresh /tmp repo.
The repository and command transcripts remain available after the session stops.
"""
import argparse
import hashlib
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
parser.add_argument('--source-diff-only', action='store_true')
parser.add_argument('--commit-rebase-only', action='store_true')
parser.add_argument('--rebase-only', action='store_true')
parser.add_argument('--decisions-only', action='store_true')
parser.add_argument('--exit-only', action='store_true')
parser.add_argument('--exit-menu-only', action='store_true')
parser.add_argument('--timing-only', action='store_true', help='Three diagnostic diff-and-return runs on one warm disposable server')
parser.add_argument('--transition-timing-only', action='store_true', help='Measure warm answer, helper dispatch, and helper exit transitions')
parser.add_argument('--history-runs', type=int, default=0, help='Retained fixture runs (three definitions and outputs each)')
parser.add_argument('--swamp-source', help='Matching Swamp source checkout, required for history seeding')
parser.add_argument('--test-output-only', action='store_true')
parser.add_argument('--latency-only', action='store_true')
parser.add_argument('--question-latency-only', action='store_true')
parser.add_argument('--latency-baseline', action='store_true', help='Measure the previous view without requiring live output')
args = parser.parse_args()
source = Path(__file__).resolve().parents[2]
root = Path(tempfile.mkdtemp(prefix='zellij-workflow-acceptance-'))
main, topic = root / 'main', root / 'topic'
main.mkdir()
binary = root / 'zellij'
# Keep the tested executable and plugins stable if another checkout rebuilds during the run.
shutil.copy2(Path(args.binary).resolve(), binary)
builtins = root / 'builtins'
builtins.mkdir()
profile = 'release' if Path(args.binary).resolve().parent.name == 'release' else 'debug'
for plugin in (source / ('target/wasm32-wasip1/' + profile)).glob('*.wasm'):
    shutil.copyfile(plugin, builtins / plugin.name)
session = 'workflow-' + root.name.rsplit('-', 1)[-1]
env = os.environ.copy()
if args.timing_only or args.transition_timing_only:
    env['ZELLIJ_SWAMP_TIMING'] = '1'
    root.chmod(0o700)
swamp_config = Path(env.get('XDG_CONFIG_HOME', str(Path.home() / '.config'))) / 'swamp'
if args.firecracker_only and not swamp_config.exists():
    swamp_config = Path.home() / '.config/swamp'
for name in ('ZELLIJ', 'ZELLIJ_SESSION_NAME', 'ZELLIJ_PANE_ID', 'ZELLIJ_PANE_REF', 'ZELLIJ_CONFIG_FILE', 'ZELLIJ_CONFIG_DIR', 'SWAMP_REPO_DIR', 'SWAMP_SERVER_URL', 'GIT_PAGER', 'PAGER', 'LESS'):
    env.pop(name, None)
for name, directory in [('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                        ('XDG_CACHE_HOME', 'cache'), ('XDG_RUNTIME_DIR', 'runtime'),
                        ('ZELLIJ_SOCKET_DIR', 'sock'), ('TMPDIR', 'tmp')]:
    env[name] = str(root / directory)
    (root / directory).mkdir()
env.update(ZELLIJ_CONFIG_DIR=str(root / 'config/zellij'),
           ZELLIJ_CONFIG_FILE=str(root / 'config/zellij/config.kdl'))
# Serve checks the existing Swamp login; keep that host credential store available
# while isolating Zellij's configuration. Never copy credentials into transcripts.
if swamp_config.exists():
    (root / 'config/swamp').symlink_to(swamp_config, target_is_directory=True)
env['SWAMP_SMOKE_ZELLIJ'] = str(binary)
if args.source_diff_only:
    env.update(GIT_PAGER='false', PAGER='false', LESS='-F -E')
real_swamp = shutil.which('swamp')
assert real_swamp, 'Swamp is required'
fixture_swamp = root / 'swamp'
shutil.copyfile(real_swamp, fixture_swamp)
fixture_swamp.chmod(0o755)
real_swamp = str(fixture_swamp)
fixture_bin = root / 'bin'
fixture_bin.mkdir()
wrapper = fixture_bin / 'swamp'
wrapper.write_text('''#!/usr/bin/env python3
import os, subprocess, sys
from pathlib import Path
with (Path(os.environ['SWAMP_SMOKE_ROOT']) / 'swamp-processes.jsonl').open('a') as log:
    log.write(__import__('json').dumps(dict(pid=os.getpid(), ppid=os.getppid(), parent=os.path.basename(os.readlink('/proc/' + str(os.getppid()) + '/exe')), command=sys.argv[1:4])) + '\\n')
os.execv(os.environ['SWAMP_SMOKE_REAL'], [os.environ['SWAMP_SMOKE_REAL']] + sys.argv[1:])
''')
wrapper.chmod(0o755)
# Fork intentionally drops old arguments. Keep each real agent-kind
# command inside this fixture so it cannot contact a model provider.
fixture_claude = fixture_bin / 'claude'
fixture_claude.write_text('#!/usr/bin/python3\nimport os\nos.execv("/usr/bin/python3", ["/usr/bin/python3", ' + repr(str(main / 'extensions/tests/stub_agent.py')) + ', "--exit"])\n')
fixture_claude.chmod(0o755)
fixture_codex = fixture_bin / 'codex'
fixture_codex.write_text(fixture_claude.read_text().replace('import os\n', 'import os, sys, json\nos.environ["SWAMP_SMOKE_KIND"] = "codex"\nwith open(os.path.join(os.environ["SWAMP_SMOKE_ROOT"], "codex-argv.jsonl"), "a") as log: log.write(json.dumps(sys.argv[1:]) + "\\n")\n'))
fixture_codex.chmod(0o755)

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


def until(label, predicate, interval=0.25):
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
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
    default = dict(agentCommand='/usr/bin/python3', agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit'], gate=['/usr/bin/true'])
    default.update(inputs)
    return zellij('workflow', 'run', '@hammz/zellij-agent-exit', '--launch', token, '--no-view', '--input', json.dumps(default))


def answer(q, value, token):
    a = q['attributes']
    return zellij('workflow', 'answer', a['run'], q['model'], a['question'], value, '--submission', token)


def command_pane(token, suffix):
    for item in histories(token):
        operation = bridge(method='lookup', work=token, key=item['runId'] + '-' + suffix)
        if operation and operation.get('outcome') == 'pending' and operation.get('pane') is not None:
            return next((p for p in json.loads(zellij('action', 'list-panes', '--json'))
                         if not p['is_plugin'] and p['id'] == operation['pane']), None)
    return None


def commit_pane(token):
    return command_pane(token, 'commit-and-rebase')


def dismiss_command(token, suffix, visible=False):
    def ready():
        pane = command_pane(token, suffix)
        return pane if pane and (not visible or not pane['is_suppressed']) else None
    pane = until(suffix + ' pane', ready)
    pane_id = 'terminal_' + str(pane['id'])
    until(suffix + ' dismissal prompt', lambda: 'Press Enter to return.' in
          zellij('action', 'dump-screen', '--pane-id', pane_id))
    zellij('action', 'write-chars', '--pane-id', pane_id, '\n')
    if suffix == 'tests':
        until('completed test pane closed', lambda: not any(
            not p['is_plugin'] and p['id'] == pane['id']
            for p in json.loads(zellij('action', 'list-panes', '--json'))))
        # Further input to the old pane cannot restart its capture command.
        run(binary, '--session', session, 'action', 'write-chars', '--pane-id', pane_id, '\n', check=False)
        assert not any(not p['is_plugin'] and p['id'] == pane['id']
                       for p in json.loads(zellij('action', 'list-panes', '--json')))


def dismiss_commit(token):
    dismiss_command(token, 'commit-and-rebase')


def exercise_exit_menus():
    checkout = root / 'exit-topic'
    run('git', 'worktree', 'add', '-b', 'exit-topic', checkout)
    pane = int(zellij('run', '--cwd', str(checkout), '--', 'claude').strip().split('_')[-1])
    until('exit-menu stub ready', lambda: 'STUB READY' in zellij('action', 'dump-screen'))
    zellij('action', 'write-chars', '--pane-id', str(pane), 'exit\n')
    automatic = until('exit-menu automatic run', lambda: swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(pane))['results'])
    token = automatic[0]['inputs']['launch']
    reduced = ['restart', 'fork', 'run-tests', 'leave']
    full = ['restart', 'fork', 'begin-review', 'run-tests', 'leave']
    prompts = {
        tuple(reduced): 'Agent exited. Choose Restart, Fork, Run tests, or Leave.',
        tuple(full): 'Agent exited. Choose Restart, Fork, Begin review, Run tests, or Leave.',
        ('begin-review', 'run-tests', 'leave'): 'Choose Begin review, Run tests, or Leave.',
    }

    def menu(token, answers):
        q = until(token + ': exit menu', lambda: pending(token, 'exit'))
        assert q['attributes']['answers'] == answers, q
        assert q['attributes']['prompt'] == prompts[tuple(answers)], q
        approval = next(a for a in swamp('workflow', 'approvals')['approvals']
                        if a['runId'] == q['attributes']['run'] and a['stepName'] == q['attributes']['step'])
        assert approval['prompt'] == q['attributes']['prompt'], approval
        return q

    def leave(token, q):
        answer(q, 'leave', 'leave-' + q['attributes']['run'])
        until(token + ': completed', lambda: all(r['status'] == 'succeeded' for r in histories(token)))

    def exited_source():
        # Leave closes its source. Each later launch needs a fresh exited pane.
        pane = int(zellij('run', '--cwd', str(checkout), '--', '/usr/bin/true').strip().split('_')[-1])
        until('exit-menu source exited', lambda: any(p['id'] == pane and not p['is_plugin'] and p['exited']
                                                    for p in json.loads(zellij('action', 'list-panes', '--json'))))
        return pane

    first = menu(token, reduced)
    for hidden in ('begin-review',):
        refused = run(binary, '--session', session, 'workflow', 'answer', first['attributes']['run'],
                      first['model'], 'exit', hidden, '--submission', 'hidden-' + hidden, check=False)
        assert refused.returncode != 0 and 'Answer is not permitted' in refused.stdout + refused.stderr
        swamp('report', 'get', '@swamp/method-summary', '--model', first['model'])
        assert menu(token, reduced)['attributes']['state'] == 'pending'
    for changed in (False, True):
        prior = menu(token, reduced)
        answer(prior, 'fork', 'fork-' + prior['attributes']['run'])
        until('exit-menu Fork picker', lambda: any(p.get('plugin_url') == 'zellij:hocus-new-agent' and not p['is_suppressed']
                                                  for p in json.loads(zellij('action', 'list-panes', '--json'))))
        if changed:
            (checkout / 'new-file').write_text('change while choosing Fork\n')
        os.write(ui_fd, b'\x1b')
        fresh = menu(token, full if changed else reduced)
        assert fresh['attributes']['run'] != prior['attributes']['run'] and fresh['model'] != prior['model']
    leave(token, fresh)
    (checkout / 'new-file').unlink()
    run('git', 'commit', '--allow-empty', '-m', 'exit menu target advances')
    with (main / '.git/info/exclude').open('a') as exclude:
        exclude.write('ignored-exit\n')
    (checkout / 'ignored-exit').write_text('ignored\n')
    for name, answers, overrides in (
        ('behind', reduced, {}),
        ('manual', ['begin-review', 'run-tests', 'leave'], {'sourcePane': -1}),
        ('missing-ref', full, {'targetBranch': 'missing-ref'}),
        ('inspection-error', full, {'checkout': str(root)}),
        ('vm-name', full, {'firecracker': 'uninspected-vm'}),
        ('vm-image', full, {'firecrackerImage': 'uninspected-image'}),
    ):
        token = 'exit-menu-' + name
        inputs = dict(checkout=str(checkout), sourcePane=exited_source() if name != 'manual' else -1, kind='claude')
        inputs.update(overrides)
        if name.startswith('vm-'):
            # Test the workflow's VM fallback without a guest. The public launch
            # command correctly refuses to assign a VM to this local fixture pane.
            inputs.update(launch=token, session=session, binary=str(binary), target=str(main))
            bridge(method='swamp_dispatch', work=token, key='launch', directory=str(main),
                   operation='workflow.run', payload=dict(workflowIdOrName='@hammz/zellij-agent-exit', inputs=inputs))
        else:
            launch(token, **inputs)
        q = menu(token, answers)
        assert bridge(method='lookup', work=token, key='worker') is None
        assert bridge(method='lookup', work=token, key=q['attributes']['run'] + '-sync') is None
        leave(token, q)
    run('git', 'commit', '--allow-empty', '-m', 'ahead without content changes', cwd=checkout)
    launch('exit-menu-ahead', checkout=str(checkout), sourcePane=exited_source())
    leave('exit-menu-ahead', menu('exit-menu-ahead', full))
    run('git', 'branch', 'exit-target', cwd=checkout)
    launch('exit-menu-target', checkout=str(checkout), sourcePane=exited_source(), targetBranch='exit-target')
    leave('exit-menu-target', menu('exit-menu-target', reduced))
    print('PASS: exact exit answers and approval prompts; hidden choices refused; Fork cancellation rechecks; behind/ignored/target, manual, errors and VM context covered', flush=True)


ui_pid = ui_fd = None
ui_output = bytearray()
ui_frames = []

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
                    ui_frames.append((len(ui_output), time.monotonic()))
                    if b'\x1b[6n' in data: os.write(ui_fd, b'\x1b[1;1R')
            except OSError:
                pass
    threading.Thread(target=drain, daemon=True).start()


print('Acceptance fixtures:', root, flush=True)
run('git', 'init', '-b', 'main')
run('git', 'config', 'user.name', 'Workflow test')
run('git', 'config', 'user.email', 'workflow@example.invalid')
(main / 'file').write_text('base\n')
(main / 'second').write_text('base\n')
run('git', 'add', 'file', 'second')
run('git', 'commit', '-m', 'base')
base = run('git', 'rev-parse', 'HEAD').stdout.strip()
run('git', 'worktree', 'add', '-b', 'topic', topic)
(topic / 'file').write_text('topic\n')
swamp('repo', 'init', str(main), '--tool', 'none')
run('swamp', 'extension', 'pull', '@swamp/git', '--json')
for directory in ('extensions', 'workflows'):
    run('cp', '-R', source / directory, main)
# Inject the lost acknowledgement at the typed approval boundary, before bundling.
# Only this disposable copy has the fault; production extensions stay unchanged.
question_source = main / 'extensions/models/question.ts'
question_code = question_source.read_text()
anchor = '    // Return at dispatch acknowledgement:'
assert question_code.count(anchor) == 1
question_code = question_code.replace(anchor, f"""    try {{
        await Deno.remove({json.dumps(str(root / 'interrupt-after-approval'))});
        throw new Error('Injected lost approval acknowledgement');
    }} catch (error) {{
        if (!(error instanceof Deno.errors.NotFound)) throw error;
    }}
""" + anchor)
question_source.write_text(question_code)
if args.commit_rebase_only:
    # Keep oversized legacy rows beside new results to test the actual CEL projection.
    git_source = main / 'extensions/models/git_workflow.ts'
    git_code = git_source.read_text()
    marker = '        const handle = await c.writeResource('
    assert git_code.count(marker) == 1
    git_code = git_code.replace(marker, """                    if (output.includes('LARGE HOOK BEGIN')) {
                        const legacy = { ...completed, stdout: output, stderr: output, operation: { ...command, output } };
                        await c.writeResource('rebaseResult', 'rebaseResult-legacy', legacy);
                        await c.writeResource('claim', 'claim-legacy', { key: 'legacy', fingerprint: 'legacy', outcome: 'completed', result: JSON.stringify(legacy) });
                    }
""" + marker)
    git_source.write_text(git_code)
(main / '.zellij').mkdir()
shutil.copy(source / '.zellij/swamp-workflows.kdl', main / '.zellij/swamp-workflows.kdl')
if args.commit_rebase_only:
    binding = main / '.zellij/swamp-workflows.kdl'
    binding.write_text(binding.read_text().replace('agent_exit {', 'agent_exit {\n    commit_agent kind="codex" model="commit-model" effort="medium"'))
(main / '.git/info/exclude').write_text('.swamp*\n.gitignore\nextensions/\nworkflows/\nmodels/\n.zellij/\nAGENTS.md\nCLAUDE.md\n')
for name in ('agent-work', 'review-integration', 'question', 'agent-exit'):
    swamp('workflow', 'validate', '@hammz/zellij-' + name)
    if name in ('agent-exit', 'review-integration'):
        definition = swamp('workflow', 'get', '@hammz/zellij-' + name)
        for job in definition['jobs']:
            for step in job['steps']:
                task = step['task']
                if task.get('methodName') == 'continue_flow':
                    assert task['inputs']['inputs']['rebasePrompt'] == '${{ inputs.rebasePrompt }}'
                    assert task['inputs']['inputs']['commitAgent'] == '${{ inputs.commitAgent }}'
if args.history_runs:
    assert args.swamp_source, '--history-runs requires --swamp-source'
    run('deno', 'run', '-A', '--config', Path(args.swamp_source) / 'deno.json',
        source / 'extensions/tests/seed_history.ts', args.swamp_source, main, args.history_runs)
if args.source_diff_only:
    # Exercise the exact workflow argv with a capturing pager, so assertions cover
    # all output (including lines outside the terminal viewport).
    workflow = swamp('workflow', 'get', '@hammz/zellij-review-integration')
    diff = next(s['task']['inputs'] for j in workflow['jobs'] for s in j['steps'] if s['name'] == 'diff')
    assert diff['cwd'] == '${{ inputs.checkout }}'
    for name in ('agent-exit', 'review-integration'):
        definition = swamp('workflow', 'get', '@hammz/zellij-' + name)
        for job in definition['jobs']:
            for step in job['steps']:
                task = step['task']
                if task.get('methodName') == 'continue_flow':
                    assert task['inputs']['inputs']['checkout'] == '${{ inputs.checkout }}'
    pager_bin = root / 'capture-bin'
    pager_bin.mkdir()
    pager = pager_bin / 'less'
    pager.write_text('#!/bin/bash\ncat -- "${@: -1}"\n')
    pager.chmod(0o755)
    sibling = root / 'sibling'
    run('git', 'worktree', 'add', '-b', 'sibling', sibling)
    viewer_tmp = topic / 'tmp [viewer]'
    viewer_tmp.mkdir()
    viewer_env = dict(env, TMPDIR=str(viewer_tmp), PATH=str(pager_bin) + os.pathsep + env['PATH'])
    argv = [str(topic) if a == '${{ inputs.checkout }}' else a for a in diff['argv']]

    def capture(overrides=None, checkout=topic):
        result = subprocess.run(argv[:-1] + [str(checkout)], cwd=main,
                                env=dict(viewer_env, **(overrides or {})),
                                text=True, capture_output=True, timeout=30)
        assert not list(viewer_tmp.iterdir()), 'Viewer must remove all temporary files'
        return result

    (topic / 'file').write_text('base\n')
    clean = capture()
    assert clean.returncode == 0, clean.stderr
    assert 'No unstaged or untracked changes.' in clean.stdout
    assert 'diff --git' not in clean.stdout
    nested = topic / "nested 'repo\nname"
    run('git', 'init', nested)
    (nested / 'private').write_text('NESTED CONTENT MUST NOT APPEAR\n')
    result = capture()
    assert result.returncode == 0, result.stderr
    assert 'Untracked nested repository:' in result.stdout
    assert "\\nname/" in result.stdout, result.stdout
    assert 'NESTED CONTENT' not in result.stdout
    assert 'No unstaged or untracked changes.' not in result.stdout
    shutil.rmtree(nested)
    for directory, label in ((main, 'MAIN'), (sibling, 'SIBLING'), (topic, 'AGENT')):
        (directory / 'file').write_text(label + ' TRACKED\n')
        (directory / ('-untracked ' + label + '\nfile')).write_text(label + ' UNTRACKED\n')
    (topic / 'second').write_text('STAGED MUST NOT APPEAR\n')
    run('git', 'add', 'second', cwd=topic)

    def repository_state(directory):
        return [run('git', *args, cwd=directory).stdout for args in (
            ('status', '--porcelain=v1'), ('diff',), ('diff', '--cached'), ('rev-parse', 'HEAD'))]

    before = [repository_state(d) for d in (main, sibling, topic)]
    for other in (None, main, sibling):
        overrides = {} if other is None else dict(
            GIT_DIR=run('git', 'rev-parse', '--absolute-git-dir', cwd=other).stdout.strip(),
            GIT_WORK_TREE=str(other), GIT_COMMON_DIR=str(main / '.git'),
            GIT_INDEX_FILE=run('git', 'rev-parse', '--path-format=absolute', '--git-path', 'index', cwd=other).stdout.strip(),
            GIT_OBJECT_DIRECTORY=str(main / '.git/objects'))
        result = capture(overrides)
        assert result.returncode == 0, result.stderr
        assert 'AGENT TRACKED' in result.stdout and 'AGENT UNTRACKED' in result.stdout
        assert not any(label in result.stdout for label in ('MAIN', 'SIBLING', 'STAGED MUST', 'zellij-show-diff.'))
        assert [repository_state(d) for d in (main, sibling, topic)] == before
    missing = capture(checkout=root / 'missing-checkout')
    assert missing.returncode != 0 and 'checkout unavailable' in missing.stderr
    for directory, label in ((main, 'MAIN'), (sibling, 'SIBLING'), (topic, 'AGENT')):
        (directory / 'file').write_text('base\n')
        (directory / ('-untracked ' + label + '\nfile')).unlink()
    (topic / 'second').write_text('base\n')
    run('git', 'reset', '--', 'second', cwd=topic)
    (topic / 'file').write_text('topic\n')
    print('PASS: exact viewer argv isolates checkout and index, quotes nested repositories, excludes its own TMPDIR, and fails for a missing checkout', flush=True)
(root / 'config/zellij').mkdir(exist_ok=True)
(root / 'config/zellij/config.kdl').write_text('show_release_notes false\nshow_startup_tips false\nbuiltin_plugins_dir ' + json.dumps(str(builtins)) + '\nfirecracker_provider ' + json.dumps(str(source / 'target/debug/zellij-firecracker')) + '\n')
observer = root / 'snapshot.ts'
observer.write_text('''import z, {definePlugin} from 'zellij:plugin';
export default definePlugin({
  load() { z.requestPermission(['MessageAndLaunchOtherPlugins','ReadCliPipes']); },
  async pipe(message) {
    if (message.name !== 'snapshot' || typeof message.source !== 'object' || !('Cli' in message.source)) return;
    const cli=message.source.Cli;
    z.host.blockCliPipeInput(cli);
    try {
      const snapshot=z.waitForPipe('agent_snapshot');
      z.command('MessageToPlugin',{plugin_url:'zellij:hocus-agentd',message_name:'agent_subscribe',
        destination_plugin_id:null,plugin_config:{},message_payload:null,message_args:{},new_plugin_args:null,floating_pane_coordinates:null});
      z.host.cliPipeOutput(cli,(await snapshot).payload ?? '');
    } finally {z.host.unblockCliPipeInput(cli);}
  }
});
''')
(root / 'cache/zellij').mkdir(exist_ok=True)
(root / 'cache/zellij/permissions.kdl').write_text(json.dumps(str(observer)) + ' {\n MessageAndLaunchOtherPlugins\n ReadCliPipes\n}\n')
if args.firecracker_only:
    provider_config = source / 'target/firecracker-test/config/zellij/firecracker.json'
    shutil.copy(provider_config, root / 'config/zellij/firecracker.json')
    # Config names the existing immutable image; VM state stays in this fixture.
cold_start_ms = None
if args.timing_only or args.transition_timing_only:
    started = time.monotonic()
    run(binary, 'workflow', 'server', 'restart')
    cold_start_ms = (time.monotonic() - started) * 1000
def decision_tests_smoke():
    for stage in ('exit', 'prepare', 'review', 'merge'):
        token = 'test-return-' + stage
        checkout = root / token
        control = root / (token + '-control')
        control.write_text('pass')
        run('git', 'worktree', 'add', '-b', token, checkout)
        gate = ['python3', '-c', 'from pathlib import Path; mode=Path(' + repr(str(control)) + ').read_text(); '
                'print("test-return live output", flush=True); '
                'Path("test-created").write_text("changed") if mode == "change" else None; '
                'raise SystemExit(1 if mode == "fail" else 0)']
        launch(token, checkout=str(checkout), gate=gate)
        if stage != 'exit': choose(token, 'exit', 'begin-review')
        if stage in ('review', 'merge'): choose(token, 'prepare', 'commit-and-rebase')
        if stage == 'merge': choose(token, 'review', 'merge')
        q = until(stage + ' offers tests', lambda: pending(token, stage))
        assert 'run-tests' in q['attributes']['answers']
        zellij('workflow', 'show', q['attributes']['run'])
        until('test menu displayed', lambda: 'Run tests' in zellij('action', 'dump-screen'))
        head = run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout
        choose(token, stage, 'run-tests', visible=True)
        fresh = until('passing tests return to ' + stage, lambda: pending(token, stage))
        assert fresh['attributes']['run'] != q['attributes']['run']
        control.write_text('fail')
        choose(token, stage, 'run-tests', visible=True)
        failed = until('failed tests offer retry', lambda: pending(token, 'failed-tests'))
        assert failed['attributes']['answers'] == ['run-tests', 'start-fix-agent', 'return-to-menu', 'leave']
        control.write_text('pass')
        choose(token, 'failed-tests', 'run-tests', visible=True)
        until('retry returns to ' + stage, lambda: pending(token, stage))
        control.write_text('fail')
        choose(token, stage, 'run-tests', visible=True)
        choose(token, 'failed-tests', 'return-to-menu')
        until('failed tests return to ' + stage, lambda: pending(token, stage))
        control.write_text('change')
        choose(token, stage, 'run-tests', visible=True)
        destination = 'exit' if stage == 'exit' else 'prepare'
        changed = until('changed tests refresh evidence', lambda: pending(token, destination))
        state = next(h['inputs']['state'] for h in histories(token) if h['runId'] == changed['attributes']['run'])
        assert state['test'] is None and state['review'] is None
        assert run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout == head
        assert (checkout / 'test-created').read_text() == 'changed'
        if stage == 'exit':
            control.write_text('fail')
            choose(token, 'exit', 'run-tests', visible=True)
            until('failed exit tests waiting', lambda: pending(token, 'failed-tests'))
            (checkout / 'edited-while-waiting').write_text('late edit')
            choose(token, 'failed-tests', 'return-to-menu')
            returned = until('return rechecks late edits', lambda: pending(token, 'exit'))
            state = next(h['inputs']['state'] for h in histories(token) if h['runId'] == returned['attributes']['run'])
            assert state['test'] is None and state['review'] is None
        choose(token, destination, 'leave')
        print('PASS: tests from', stage, 'pass/fail/retry/return/change preserve origin and require fresh approval', flush=True)


    token = 'foreground-cleanup'
    checkout = root / token
    run('git', 'worktree', 'add', '-b', token, checkout)
    launch(token, checkout=str(checkout))
    choose(token, 'exit', 'begin-review')
    choose(token, 'prepare', 'commit-and-rebase')
    choose(token, 'review', 'merge')
    q = until('cleanup fixture confirmation', lambda: pending(token, 'merge'))
    zellij('workflow', 'show', q['attributes']['run'])
    until('merge confirmation displayed', lambda: 'Merge exact revision' in zellij('action', 'dump-screen'))
    choose(token, 'merge', 'merge', visible=True)
    choose(token, 'cleanup', 'remove-clean-worktree', visible=True)
    until('guarded removal completed', lambda: not checkout.exists())
    print('PASS: merge and non-force cleanup occupy the foreground and wait for dismissal', flush=True)


def rebase_smoke():
    # Build a real paused rebase and refuse a resolver before approving one.
    (topic / 'file').write_text('source conflict\n')
    run('git', 'commit', '-am', 'source conflict', cwd=topic)
    (topic / 'second').write_text('source second conflict\n')
    run('git', 'commit', '-am', 'second source conflict', cwd=topic)
    (main / 'file').write_text('target conflict\n')
    (main / 'second').write_text('target second conflict\n')
    run('git', 'commit', '-am', 'target conflict')
    launch('conflict-leave')
    choose('conflict-leave', 'exit', 'begin-review')
    choose('conflict-leave', 'prepare', 'commit-and-rebase')
    count = len(agents())
    choose('conflict-leave', 'conflict', 'leave-paused')
    assert len(agents()) == count
    assert run('git', 'diff', '--name-only', '--diff-filter=U', cwd=topic).stdout.strip() == 'file'
    configured_prompt = 'CUSTOM RESOLVER: Resolve and stage conflicts; do not continue or abort the rebase. Exit.'
    config_path = main / '.zellij/swamp-workflows.kdl'
    original_config = config_path.read_text()
    config_path.write_text(re.sub(r'    rebase_prompt .*', '    rebase_prompt ' + json.dumps(configured_prompt), original_config))
    launch('conflict-resolve')
    choose('conflict-resolve', 'exit', 'begin-review')
    choose('conflict-resolve', 'conflict', 'run-tests')
    first_conflict = choose('conflict-resolve', 'conflict', 'resolve-with-agent')
    config_path.write_text(original_config)
    staged = until('explicit staged decision', lambda: pending('conflict-resolve', 'resolved'))
    assert staged['attributes']['answers'] == ['continue-rebase', 'run-tests', 'leave-paused']
    head = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
    choose('conflict-resolve', 'resolved', 'run-tests')
    assert run('git', 'rev-parse', 'HEAD', cwd=topic).stdout == head
    until('tests return to staged decision', lambda: pending('conflict-resolve', 'resolved'))
    assert not pending('conflict-resolve', 'review')
    choose('conflict-resolve', 'resolved', 'continue-rebase')
    next_conflict = choose('conflict-resolve', 'conflict', 'resolve-with-agent')
    choose('conflict-resolve', 'resolved', 'continue-rebase')
    assert first_conflict['attributes']['run'] != next_conflict['attributes']['run']
    choose('conflict-resolve', 'review', 'leave')
    assert not run('git', 'diff', '--name-only', '--diff-filter=U', cwd=topic).stdout.strip()
    assert run('git', 'symbolic-ref', '--short', 'HEAD', cwd=topic).stdout.strip() == 'topic'
    prompts = [json.loads(line) for line in (root / 'resolver-prompts.jsonl').read_text().splitlines()]
    assert prompts.count(configured_prompt) == 2 and prompts[-2:] == [configured_prompt, configured_prompt], prompts
    print('PASS: paused rebase requires an explicit resolver; each continuation requires a choice and tests preserve the paused index', flush=True)

    for mode in ('unchanged', 'completed', 'completed-dirty', 'staged-dirty'):
        checkout = root / ('resolver-' + mode)
        token = 'resolver-' + mode
        run('git', 'worktree', 'add', '-b', token, checkout, base)
        (checkout / 'file').write_text('resolver fixture conflict\n')
        run('git', 'commit', '-am', 'resolver fixture', cwd=checkout)
        paused = run('git', 'rebase', 'main', cwd=checkout, check=False)
        assert paused.returncode != 0
        launch(token, checkout=str(checkout), agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit', '--resolver-' + mode])
        choose(token, 'exit', 'begin-review')
        before = run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout
        q = choose(token, 'conflict', 'resolve-with-agent')
        if mode == 'staged-dirty':
            choose(token, 'resolved', 'continue-rebase')
        destination = {'unchanged': 'conflict', 'completed': 'review', 'completed-dirty': 'prepare', 'staged-dirty': 'prepare'}[mode]
        fresh = until('resolver ' + mode + ' inspected', lambda: pending(token, destination))
        assert fresh['attributes']['run'] != q['attributes']['run']
        if mode == 'unchanged':
            assert run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout == before
            assert run('git', 'diff', '--name-only', '--diff-filter=U', cwd=checkout).stdout.strip() == 'file'
        else:
            assert run('git', 'symbolic-ref', '--short', 'HEAD', cwd=checkout).stdout.strip() == token
        choose(token, destination, 'leave-paused' if mode == 'unchanged' else 'leave')
        print('PASS: resolver', mode, 'returns to', destination, 'without automatic continuation', flush=True)



run(binary, 'attach', '--create-background', session)
try:
    if args.question_latency_only:
        attach_ui()
        until('terminal client has rendered', lambda: len(ui_output) > 1000)
        for startup in json.loads(zellij('action', 'list-panes', '--json')):
            if startup.get('plugin_url')=='zellij:about':
                zellij('action', 'close-pane', '--pane-id', 'plugin_' + str(startup['id']))
        samples=[]
        for index in range(8):
            pane=int(zellij('run', '--cwd', str(topic), '--', 'claude').strip().split('_')[-1])
            until('stub ready', lambda: 'STUB READY' in zellij('action', 'dump-screen'))
            offset=len(ui_output)
            zellij('action', 'write-chars', '--pane-id', str(pane), 'exit\n')
            until('actionable exit question', lambda: b'Begin review' in bytes(ui_output[offset:]), interval=0.005)
            elapsed=time.monotonic()-float((root / ('exit-' + str(pane))).read_text())
            token=swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(pane))['results'][0]['inputs']['launch']
            q=until('exit question data', lambda: pending(token, 'exit'))
            offset=len(ui_output)
            submitted=time.monotonic()
            answer(q, 'begin-review', 'timing-begin-' + str(index))
            until('next actionable question', lambda: b'Show diff' in bytes(ui_output[offset:]), interval=0.005)
            next_question=time.monotonic()-submitted
            samples.append(dict(exit_to_question=elapsed, answer_to_question=next_question))
            print('TIMING SAMPLE:', index, samples[-1], flush=True)
            q=until('preparation question data', lambda: pending(token, 'prepare'))
            answer(q, 'leave', 'timing-leave-' + str(index))
            until('workflow completed', lambda: (lambda rows: rows and all(r['status']=='succeeded' for r in rows))(swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(pane))['results']))
        import statistics
        def summary(key):
            warm=sorted(sample[key] for sample in samples[1:])
            position=(len(warm)-1)*.95
            lower=int(position)
            p95=warm[lower]+(warm[min(lower+1,len(warm)-1)]-warm[lower])*(position-lower)
            return dict(cold=samples[0][key], warm_median=statistics.median(warm), warm_p95=p95)
        measured=dict(samples=samples, exit_to_question=summary('exit_to_question'), answer_to_question=summary('answer_to_question'))
        (root / 'question-latency.json').write_text(json.dumps(measured, indent=2))
        print('QUESTION LATENCY:', measured, flush=True)
        raise SystemExit(0)
    if args.firecracker_only:
        shutil.copy(source / 'extensions/tests/stub_agent.py', topic / 'stub_agent.py')
        vm = bridge(method='agent_spawn', work='vm', key='worker', spec=dict(command='/usr/bin/python3', args=['stub_agent.py', '--exit'], cwd=str(topic), source_directory=str(topic), firecracker='workflow-vm', firecracker_image='zellij-dev', automatic_firecracker_vm=True))
        assert vm['supervisor'].startswith('remote:')
        bridge(method='agent_prompt', work='vm', key='edit', agent='worker', prompt='guest edit')
        until('original guest exits', lambda: bridge(method='observe', work='vm', key='worker').get('processExited'))
        edited = bridge(method='observe', work='vm', key='edit')
        assert edited.get('message'), zellij('action', 'dump-screen', '--pane-id', 'terminal_' + str(vm['pane']))
        guest_commit = edited['message']
        assert not (topic / 'guest.txt').exists()
        vm_name = vm['target']['Firecracker']['vm']
        (main / 'host-only').write_text('host revision\n')
        run('git', 'add', 'host-only')
        run('git', 'commit', '-m', 'advance host target')
        host_target = run('git', 'rev-parse', 'HEAD').stdout.strip()
        launch('original-vm', sourcePane=vm['pane'], firecracker=vm_name,
               agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit', '--inspect-review'])
        q = until('VM exit menu before sync or spawn', lambda: pending('original-vm', 'exit'))
        assert not (topic / 'guest.txt').exists()
        assert bridge(method='lookup', work='original-vm', key='worker') is None
        answer(q, 'begin-review', 'vm-begin')
        prepared = until('original VM synced without worker', lambda: pending('original-vm', 'prepare'))
        assert (topic / 'guest.txt').read_text() == 'guest\n'
        assert bridge(method='lookup', work='original-vm', key='worker') is None
        answer(prepared, 'commit-and-rebase', 'vm-rebase')
        dismiss_commit('original-vm')
        review_menu = until('host rebased review menu', lambda: pending('original-vm', 'review'))
        host_commit = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout.strip()
        assert host_commit != guest_commit
        answer(review_menu, 'start-code-review', 'vm-review')
        confirmation = until('host reviewer confirmation', lambda: pending('original-vm', 'merge'))
        records = swamp('data', 'query', 'modelType == "@hammz/zellij" && name == "review-current"', '--select', '{"attributes":attributes}')['results']
        reviewed = next(r['attributes'] for r in records if r['attributes']['evidence']['source']['commit'] == host_commit)
        inspected = json.loads(reviewed['findings'][0]['message'])
        assert reviewed['resolved']
        assert inspected['checkout'] == str(topic)
        assert inspected['commit'] == reviewed['evidence']['source']['commit']
        assert inspected['contents'] == reviewed['evidence']['source']['contents']
        assert inspected['host_file'] == 'host revision\n'
        helper = bridge(method='lookup', work='original-vm', key=reviewed['reviewer'])
        assert helper['target'] == 'Local'
        answer(confirmation, 'leave', 'vm-review-leave')
        print('PASS: VM reviewer runs locally and inspects rebased host evidence', flush=True)
        # A second explicitly spawned fixture creates guest/host conflicting changes.
        conflict = bridge(method='agent_spawn', work='vm-conflict-fixture', key='worker', spec=dict(command='/usr/bin/python3', args=['stub_agent.py', '--exit'], cwd=str(topic), source_directory=str(topic), firecracker=vm_name, reuse=True))
        bridge(method='agent_prompt', work='vm-conflict-fixture', key='edit', agent='worker', prompt='guest conflict')
        until('conflicting guest exits', lambda: bridge(method='observe', work='vm-conflict-fixture', key='worker').get('processExited'))
        (topic / 'file').write_text('host conflict\n')
        launch('conflict-vm', sourcePane=conflict['pane'], firecracker=vm_name)
        q = until('conflict VM menu', lambda: pending('conflict-vm', 'exit'))
        answer(q, 'begin-review', 'conflict-begin')
        def conflict_failed():
            runs = swamp('workflow', 'history', 'search', '--input', 'launch=conflict-vm')['results']
            return runs if any(r['status'] == 'failed' for r in runs) and not any(r['status'] == 'running' for r in runs) else None
        failed_runs = until('sync conflict blocks preparation', conflict_failed)
        for item in failed_runs:
            if item['status'] == 'failed':
                swamp('report', 'get', '@swamp/workflow-summary', '--workflow', item['workflowName'])
        assert bridge(method='lookup', work='conflict-vm', key='worker') is None
        assert (topic / 'file').read_text() == 'host conflict\n'
        assert run('git', 'rev-parse', 'HEAD').stdout.strip() == host_target
        assert topic.exists()
        print('PASS: original exited VM synced without spawning; sync conflicts preserve checkout and block integration', flush=True)
        raise SystemExit(0)
    if not args.recovery_only:
        attach_ui()
        until('terminal client has rendered', lambda: len(ui_output) > 1000)
        def choose(token, question, value, dismiss=True, visible=False):
            print('CHOICE:', token, question, value, flush=True)
            q = until(token + ': ' + question, lambda: pending(token, question))
            answer(q, value, 'answer-' + q['attributes']['run'])
            suffix = {'commit-and-rebase': 'commit-and-rebase', 'continue-rebase': 'continue-rebase',
                      'run-tests': 'tests', 'remove-clean-worktree': 'cleanup'}.get(value)
            if value == 'merge' and question == 'merge':
                suffix = 'integrate'
            if suffix and dismiss:
                dismiss_command(token, suffix, visible=visible)
            return q

        def prepare(token):
            choose(token, 'exit', 'begin-review')
            choose(token, 'prepare', 'commit-and-rebase')
            return until(token + ': review menu', lambda: pending(token, 'review'))

        def agents():
            return [p for p in json.loads(zellij('action', 'list-panes', '--json')) if p.get('agent')]

        if args.decisions_only:
            decision_tests_smoke()
            raise SystemExit(0)

        if args.rebase_only:
            rebase_smoke()
            raise SystemExit(0)

        if args.transition_timing_only:
            def rendered(label, offset, value):
                until(label, lambda: value in bytes(ui_output[offset:]), interval=0.005)
                end = bytes(ui_output).index(value, offset) + len(value)
                return next(stamp for size, stamp in ui_frames if size >= end)

            samples = []
            for repetition in range(3):
                token = 'transition-' + str(repetition)
                launch(token, gate=['/usr/bin/python3', '-c', 'print("failure-evidence" + " retained failure line" * 12000); raise SystemExit(1)'])
                q = until('exit question', lambda: pending(token, 'exit'))
                offset = len(ui_output)
                zellij('workflow', 'show', q['attributes']['run'])
                rendered('initial exit question rendered', offset, b'Begin review')
                offset = len(ui_output)
                started = time.monotonic()
                os.write(ui_fd, b'\t' * q['attributes']['answers'].index('begin-review') + b'\r')
                question_at = rendered('preparation rendered', offset, b'Show diff')
                answer_to_question = (question_at - started) * 1000
                answer_started = started
                choose(token, 'prepare', 'commit-and-rebase')
                failure_offset = len(ui_output)
                choose(token, 'review', 'run-tests')
                failed = until('saved failure', lambda: pending(token, 'failed-tests'))
                rendered('failure question rendered', failure_offset, b'Start fix agent')
                offset = len(ui_output)
                started = time.monotonic()
                os.write(ui_fd, b'\t' * failed['attributes']['answers'].index('start-fix-agent') + b'\r')
                next_question_at = rendered('helper exit question rendered', offset, b'Show diff')
                fresh = until('fresh preparation question', lambda: pending(token, 'prepare'))
                assert fresh['attributes']['run'] != failed['attributes']['run']
                assert 'failure-evidence' in (topic / 'received-failure-output').read_text()
                prompt_at = float((root / 'prompt-received').read_text())
                helper_exit = float((root / 'helper-exit').read_text())
                samples.append(dict(repetition=repetition + 1, answerToQuestionMs=answer_to_question,
                    answerToPromptMs=(prompt_at - started) * 1000,
                    helperExitToQuestionMs=(next_question_at - helper_exit) * 1000,
                    agent=json.loads((root / 'helper-timing.json').read_text()),
                    clocks=dict(answer=answer_started, question=question_at, fixAnswer=started, prompt=prompt_at, helperExit=helper_exit, nextQuestion=next_question_at),
                    previousRun=failed['attributes']['run'], nextRun=fresh['attributes']['run']))
                for item in histories(token):
                    run_id = item['runId']
                    for kind in ('get', 'logs'):
                        (root / (run_id + '.' + kind + '.json')).write_text(json.dumps(swamp('workflow', 'history', kind, run_id), indent=2))
                (topic / 'received-failure-output').unlink()
                (topic / 'received-failure-prompt').unlink()
                choose(token, 'prepare', 'leave')
                print('TRANSITION:', samples[-1], flush=True)
            for path in Path(env['TMPDIR']).rglob('zellij.log*'):
                shutil.copy(path, root / path.name)
            measured = dict(profile=profile, answerInput='terminal', coldStartMs=cold_start_ms, retainedRuns=args.history_runs, samples=samples,
                wallClockOffset=time.time()-time.monotonic(),
                reportSourceSha256=hashlib.sha256((main / 'extensions/reports/workflow.ts').read_bytes()).hexdigest(),
                binaries={name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in [('zellij', binary), ('swamp', fixture_swamp)]})
            (root / 'transitions.json').write_text(json.dumps(measured, indent=2))
            print('PASS: three fix cycles with saved failure output and fresh exit questions', flush=True)
            raise SystemExit(0)

        if args.exit_menu_only:
            exercise_exit_menus()
            raise SystemExit(0)

        if args.commit_rebase_only:
            hooks = root / 'commit-hooks'
            hooks.mkdir()
            hook = hooks / 'pre-commit'
            release = root / 'release-commit'
            for scenario in ('success', 'clean', 'commit-failed', 'conflict'):
                if scenario == 'success':
                    (main / 'target-only').write_text('advanced target\n')
                    run('git', 'add', 'target-only')
                    run('git', 'commit', '-m', 'advance target')
                    hook.write_text('#!/usr/bin/env python3\nimport time\nfrom pathlib import Path\n'
                                    'print("LIVE COMMIT HOOK", flush=True)\n'
                                    f'while not Path({str(release)!r}).exists(): time.sleep(0.01)\n'
                                    'print("LARGE HOOK BEGIN\\n" + ("😀é𝄞\\n" * 65000) + "LARGE HOOK END", flush=True)\n')
                    hook.chmod(0o700)
                    run('git', 'config', 'core.hooksPath', hooks)
                    original_id = int(zellij('run', '--cwd', str(topic), '--', 'claude').strip().split('_')[-1])
                    until('source ready', lambda: 'STUB READY' in zellij('action', 'dump-screen'))
                    zellij('action', 'write-chars', '--pane-id', str(original_id), 'exit\n')
                    automatic = until('automatic exit run', lambda: swamp('workflow', 'history', 'search',
                                      '--filter', 'inputs.sourcePane == ' + str(original_id))['results'])
                    token = automatic[0]['inputs']['launch']
                else:
                    token = 'foreground-' + scenario
                    if scenario == 'commit-failed':
                        (topic / 'file').write_text('failed commit contents\n')
                        hook.write_text('#!/bin/sh\nprintf "COMMIT REJECTED\\n" >&2\nexit 1\n')
                    elif scenario == 'conflict':
                        run('git', 'config', '--unset', 'core.hooksPath')
                        (main / 'file').write_text('target conflict\n')
                        run('git', 'commit', '-am', 'target conflict')
                    launch(token)
                choose(token, 'exit', 'begin-review')
                q = until('foreground preparation', lambda: pending(token, 'prepare'))
                if scenario != 'success':
                    zellij('workflow', 'show', q['attributes']['run'])
                until('preparation displayed', lambda: 'Show diff' in zellij('action', 'dump-screen'))
                view = next(p for p in json.loads(zellij('action', 'list-panes', '--json'))
                            if p.get('plugin_url') in ('zellij:swamp-workflow', 'zellij:agent-exit')
                            and not p['is_suppressed'] and p['is_focused'])
                head = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
                messages = root / 'commit-messages.jsonl'
                previous_messages = messages.read_text().splitlines() if messages.exists() else []
                answer(q, 'commit-and-rebase', 'foreground-' + scenario)
                pane = until('foreground commit pane', lambda: commit_pane(token))
                pane_id = 'terminal_' + str(pane['id'])
                until('command pane takes focus', lambda: any(p['id'] == pane['id'] and not p['is_plugin']
                      and p['is_focused'] and not p['is_suppressed'] for p in json.loads(zellij('action', 'list-panes', '--json'))))
                for field in ('pane_x', 'pane_y', 'pane_rows', 'pane_columns', 'is_floating'):
                    assert pane[field] == view[field], (field, pane, view)
                screen = lambda: zellij('action', 'dump-screen', '--pane-id', pane_id)
                if scenario == 'success':
                    until('live commit command and hook output', lambda: '$ git commit' in screen() and 'LIVE COMMIT HOOK' in screen())
                    assert 'Press Enter to return.' not in screen()
                    release.touch()
                until('result waits for dismissal', lambda: 'Press Enter to return.' in screen())
                output = screen()
                assert ('No changes to commit.' in output) == (scenario == 'clean'), output
                assert ('$ git rebase ' in output) == (scenario != 'commit-failed'), output
                assert ('finished (exit 0)' in output) == (scenario in ('success', 'clean')), output
                if scenario == 'commit-failed':
                    assert 'COMMIT REJECTED' in output
                if scenario in ('clean', 'commit-failed'):
                    assert run('git', 'rev-parse', 'HEAD', cwd=topic).stdout == head
                generated = messages.read_text().splitlines()
                assert len(generated) == len(previous_messages) + (scenario != 'clean')
                if scenario == 'success':
                    generated_message = json.loads(generated[-1])
                    assert generated_message['checkout'] == str(topic)
                    assert run('git', 'log', '-1', '--format=%B', cwd=topic).stdout.rstrip('\n') == generated_message['message']
                    assert not (topic / 'INJECTED').exists()
                    argv = json.loads((root / 'codex-argv.jsonl').read_text().splitlines()[-1])
                    assert 'commit-model' in argv and any('model_reasoning_effort' in arg and 'medium' in arg for arg in argv), argv
                    retained = swamp('data', 'query', 'name == "commitMessage-current"', '--select', 'attributes')['results']
                    assert any(item['message'] == generated_message['message'] and item['owner']['agent'].endswith('-commit-agent') for item in retained)
                # Persistence is the behavior under test: finished commands must await input.
                time.sleep(1)
                assert commit_pane(token) and not pending(token, 'review') and not pending(token, 'conflict')
                os.write(ui_fd, b'\r')
                until('workflow view restored', lambda: any(p['id'] == view['id'] and p['is_plugin']
                      and not p['is_suppressed'] for p in json.loads(zellij('action', 'list-panes', '--json'))))
                if scenario == 'success':
                    question = until('review after oversized commit output', lambda: pending(token, 'review'))
                    snapshot = bridge(method='workflow_show', directory=str(topic), run=question['attributes']['run'])
                    assert len(json.dumps(snapshot).encode()) < 1_048_576
                    assert any(q['attributes']['state'] == 'pending' for q in snapshot['questions']['results'])
                    entries = snapshot['outputs']['results']
                    result = next(e for e in entries if e['name'] == 'rebaseResult-current')
                    ref = result['attributes']['outputRef']
                    model = swamp('model', 'get', ref['modelId'])['name']
                    exact = 'modelName == ' + json.dumps(model) + ' && name == ' + json.dumps(ref['name']) + ' && version == ' + str(ref['version'])
                    full = swamp('data', 'query', exact, '--select', 'attributes.output')['results'][0]
                    assert len(full.encode()) >= 600_000 and 'LARGE HOOK BEGIN' in full and 'LARGE HOOK END' in full
                    exact = 'modelName == ' + json.dumps(result['model']) + ' && name == "rebaseResult-current" && version == ' + str(result['version'])
                    saved = swamp('data', 'query', exact, '--select', 'attributes')['results'][0]
                    assert len(saved['stdout'].encode()) <= 16384 and not saved['operation'].get('output')
                    assert 'LARGE HOOK BEGIN' in saved['stdout'] and 'LARGE HOOK END' in saved['stdout']
                    claim_name = 'claim-' + saved['operation']['key']
                    claim = swamp('data', 'query', 'modelName == ' + json.dumps(result['model']) + ' && name == ' + json.dumps(claim_name), '--select', 'attributes')['results'][0]
                    assert claim['outcome'] == 'completed'
                    assert json.loads(claim['result'])['stdout'] == saved['stdout']
                    legacy = next(e for e in entries if e['name'] == 'rebaseResult-legacy')['attributes']
                    assert len(legacy['stdout'].encode()) + len(legacy['stderr'].encode()) <= 16384
                    assert not legacy['operation'].get('output')
                    assert next(e for e in entries if e['name'] == 'claim-legacy')['attributes']['outcome'] == 'completed'
                    assert all('result' not in e['attributes'] for e in entries if e['name'].startswith('claim-'))
                    assert next(e for e in entries if e['name'] == ref['name'])['attributes'] == {}
                if scenario == 'commit-failed':
                    def failed_run():
                        return next((r for r in swamp('workflow', 'history', 'search', '--input', 'launch=' + token)['results']
                                     if r['status'] == 'failed'), None)
                    failed = until('commit failure stops workflow', failed_run)
                    swamp('report', 'get', '@swamp/workflow-summary', '--workflow', failed['workflowName'])
                else:
                    choose(token, 'conflict' if scenario == 'conflict' else 'review',
                           'leave-paused' if scenario == 'conflict' else 'leave')
                print('PASS:', scenario, 'shows commands/output in place, waits for Enter, and restores workflow', flush=True)
            run('git', 'rebase', '--abort', cwd=topic)
            (topic / 'commit-description-test').write_text('pending commit\n')
            for scenario in ('malformed', 'cancelled'):
                token = 'commit-message-' + scenario
                flag = '--malformed-commit' if scenario == 'malformed' else '--hold-commit'
                launch(token, commitAgent=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--exit', flag]))
                choose(token, 'exit', 'begin-review')
                q = until('commit message preparation', lambda: pending(token, 'prepare'))
                head = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
                answer(q, 'commit-and-rebase', token)
                commit_runs = lambda: swamp('workflow', 'history', 'search', '--input', 'launch=' + token)['results']
                def commit_operation():
                    for item in commit_runs():
                        operation = bridge(method='lookup', work=token, key=item['runId'] + '-commit-agent-prompt')
                        if operation and operation.get('accepted'):
                            return item, operation
                item, operation = until('commit agent received prompt', commit_operation)
                if scenario == 'cancelled':
                    zellij('workflow', 'cancel', item['runId'])
                    until('commit run cancelled', lambda: all(r['status'] not in ('running', 'suspended') for r in commit_runs()))
                    (root / 'release-commit-agent').touch()
                    until('late commit agent exits', lambda: any(p['id'] == operation['pane'] and p['exited'] for p in agents()))
                else:
                    until('malformed message stops workflow', lambda: any(r['status'] == 'failed' for r in commit_runs()))
                    report = swamp('report', 'get', '@swamp/method-summary', '--model', item['runId'] + '-commit-message')['json']
                    assert report['status'] == 'failed' and report['methodName'] == 'commit_message_result'
                assert bridge(method='lookup', work=token, key=item['runId'] + '-commit-and-rebase') is None
                assert run('git', 'rev-parse', 'HEAD', cwd=topic).stdout == head
                assert (topic / 'commit-description-test').exists()
                print('PASS:', scenario, 'commit message preserves changes and prevents Git dispatch', flush=True)
            raise SystemExit(0)

        def exercise_diff(token, view=None):
            before_diff = run('git', 'status', '--porcelain=v1', cwd=topic).stdout
            before_head = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
            contents = (topic / 'file').read_text()
            base_contents = run('git', 'show', 'HEAD:file', cwd=topic).stdout
            second_contents = (topic / 'second').read_text()
            (topic / 'second').write_text('staged changes stay outside Show diff\n')
            run('git', 'add', 'second', cwd=topic)
            untracked = topic / 'untracked diff\nfixture'
            staged = run('git', 'diff', '--cached', cwd=topic).stdout
            # The viewer must ignore both Git's pager and less's auto-exit options.
            run('git', 'config', 'core.pager', 'false')
            try:
                for shape, addition in [('short', 'foreground diff line\n'),
                                        ('long', ''.join(f'foreground diff line {i:03d}\n' for i in range(200))),
                                        ('empty', '')]:
                    (topic / 'file').write_text(base_contents + addition)
                    if shape == 'empty':
                        if untracked.exists():
                            untracked.unlink()
                    else:
                        untracked.write_text('untracked diff contents\n')
                    for dismiss in ('q', 'Esc'):
                        expected = run('git', 'diff', '--no-ext-diff', cwd=topic).stdout
                        previous = choose(token, 'prepare', 'show-diff')
                        def viewer():
                            return next((p for p in json.loads(zellij('action', 'list-panes', '--json'))
                                         if not p['is_plugin'] and not p['exited']
                                         and (not view or not p['is_suppressed'])
                                         and '--lesskey-src=' in (p.get('terminal_command') or '')), None)
                        terminal = until('persistent diff viewer', viewer)
                        pane = 'terminal_' + str(terminal['id'])
                        def send(data):
                            if view:
                                os.write(ui_fd, data)
                            else:
                                zellij('action', 'write', '--pane-id', pane, *map(str, data))
                        if view:
                            assert not terminal['is_suppressed']
                            for field in ('pane_x', 'pane_y', 'pane_rows', 'pane_columns', 'is_floating'):
                                assert terminal[field] == view[field], (field, terminal, view)
                            panes = json.loads(zellij('action', 'list-panes', '--json'))
                            assert next(p for p in panes if p['is_plugin'] and p['id'] == view['id'])['is_suppressed']
                        def screen():
                            return zellij('action', 'dump-screen', '--pane-id', pane)
                        until('viewer renders ' + shape, lambda: ('No unstaged or untracked changes.' if shape == 'empty'
                              else 'foreground diff line') in screen())
                        until('viewer prompt', lambda: 'q to return' in screen())
                        assert 'staged changes stay outside Show diff' not in screen()
                        if shape == 'long':
                            first = screen()
                            send(b'\x1b[B')
                            until('down arrow scrolls', lambda: screen() != first)
                            send(b'\x1b[A')
                            until('up arrow scrolls back', lambda: screen() == first)
                            send(b'/foreground diff line 190\r')
                            until('search finds later line', lambda: 'foreground diff line 199' in screen())
                        if shape != 'empty':
                            send(b'/untracked diff contents\r')
                            until('untracked contents render', lambda: 'untracked diff contents' in screen())
                        # This is a persistence assertion: short/empty output must not auto-exit.
                        time.sleep(1)
                        assert viewer(), (shape, dismiss)
                        # Suppressed viewers need explicit input too; encode Esc as Zellij does.
                        send(b'q' if dismiss == 'q' else (b'\x1b' if view else b'\x1b[27u'))
                        fresh = until('pager exit returns to preparation', lambda: pending(token, 'prepare'))
                        assert fresh['attributes']['run'] != previous['attributes']['run']
                        if view:
                            until('preparation menu renders again', lambda: 'Show diff' in zellij('action', 'dump-screen'))
                            restored = next(p for p in json.loads(zellij('action', 'list-panes', '--json'))
                                            if p['is_plugin'] and p['id'] == view['id'])
                            assert not restored['is_suppressed']
                        assert (topic / 'file').read_text() == base_contents + addition
                        assert run('git', 'diff', '--no-ext-diff', cwd=topic).stdout == expected
                        assert run('git', 'diff', '--cached', cwd=topic).stdout == staged
                        if shape == 'empty':
                            assert not untracked.exists()
                        else:
                            assert untracked.read_text() == 'untracked diff contents\n'
                        assert run('git', 'rev-parse', 'HEAD', cwd=topic).stdout == before_head
                        assert not list(Path(env['TMPDIR']).glob('zellij-show-diff.*'))
                        print('PASS:', token, shape, dismiss, 'persists and restores preparation without mutation', flush=True)
            finally:
                (topic / 'file').write_text(contents)
                (topic / 'second').write_text(second_contents)
                run('git', 'reset', '--', 'second', cwd=topic)
                if untracked.exists():
                    untracked.unlink()
                run('git', 'config', '--unset', 'core.pager')
            assert run('git', 'status', '--porcelain=v1', cwd=topic).stdout == before_diff

        if args.timing_only:
            launch('timing')
            choose('timing', 'exit', 'begin-review')
            until('warm preparation', lambda: pending('timing', 'prepare'))
            measurements = []
            for repetition in range(3):
                started = time.monotonic()
                previous = choose('timing', 'prepare', 'show-diff')
                def viewer():
                    return next((p for p in json.loads(zellij('action', 'list-panes', '--json'))
                                 if not p['is_plugin'] and not p['exited']
                                 and '--lesskey-src=' in (p.get('terminal_command') or '')), None)
                diff = until('diagnostic viewer', viewer)
                pane = 'terminal_' + str(diff['id'])
                until('diagnostic viewer ready', lambda: 'q to return' in zellij('action', 'dump-screen', '--pane-id', pane))
                dismissed = time.monotonic()
                zellij('action', 'write-chars', '--pane-id', pane, 'q')
                fresh = until('diff returns to preparation', lambda: pending('timing', 'prepare'))
                assert fresh['attributes']['run'] != previous['attributes']['run']
                elapsed = (time.monotonic() - started) * 1000
                runs = histories('timing')
                for item in runs:
                    run_id = item['runId']
                    (root / (run_id + '.history.json')).write_text(json.dumps(swamp('workflow', 'history', 'get', run_id), indent=2))
                    logs = swamp('workflow', 'history', 'logs', run_id)
                    (root / (run_id + '.logs.json')).write_text(json.dumps(logs, indent=2))
                    timing_lines = [line for line in logs['lines'] if 'swamp_timing ' in line]
                    assert timing_lines, 'Missing TypeScript timings for ' + run_id
                    for line in timing_lines:
                        record = json.loads(line.split('swamp_timing ', 1)[1])
                        assert record['layer'] in ('typescript', 'swamp') and record['elapsedMs'] >= 0
                measurements.append(dict(repetition=repetition + 1, elapsedMs=elapsed,
                    untilDismissMs=(dismissed - started) * 1000, previousRun=previous['attributes']['run'], nextRun=fresh['attributes']['run']))
                for path in Path(env['TMPDIR']).rglob('zellij.log*'):
                    shutil.copy(path, root / ('repetition-' + str(repetition + 1) + '-' + path.name))
                print('TIMING:', measurements[-1], flush=True)
            (root / 'timing.json').write_text(json.dumps(dict(coldStartMs=cold_start_ms, repetitions=measurements), indent=2))
            choose('timing', 'prepare', 'leave')
            print('PASS: three warm diff-and-return repetitions; cold startup recorded separately', flush=True)
            raise SystemExit(0)

        if args.source_diff_only or args.exit_only:
            if not args.exit_only:
                launch('unattended-diff')
                choose('unattended-diff', 'exit', 'begin-review')
                exercise_diff('unattended-diff')
                choose('unattended-diff', 'prepare', 'leave')
                launch('previews')
                choose('previews', 'exit', 'begin-review')
                preparation = until('manual preparation', lambda: pending('previews', 'prepare'))
                zellij('workflow', 'show', preparation['attributes']['run'])
                until('manual preparation menu renders', lambda: 'Show diff' in zellij('action', 'dump-screen'))
                view = next(p for p in json.loads(zellij('action', 'list-panes', '--json'))
                            if p.get('plugin_url') == 'zellij:swamp-workflow' and not p['is_suppressed'])
                exercise_diff('previews', view)
                choose('previews', 'prepare', 'leave')
            original_id = int(zellij('run', '--cwd', str(topic), '--', 'claude').strip().split('_')[-1])
            original = until('tiled source is ready for input', lambda: next((p for p in json.loads(zellij('action', 'list-panes', '--json')) if not p['is_plugin'] and p['id'] == original_id and p.get('pane_cwd') == str(topic)), None))
            assert not original['is_floating'] and not original['is_suppressed']
            until('stub has enabled raw input', lambda: 'STUB READY' in zellij('action', 'dump-screen'))
            zellij('action', 'write-chars', '--pane-id', str(original_id), 'exit\n')
            original = until('automatic exit hook suppresses source', lambda: next((p for p in agents() if p['id'] == original_id and p['exited'] and p['is_suppressed']), None))
            automatic = until('automatic exit run', lambda: swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(original_id))['results'])
            token = automatic[0]['inputs']['launch']
            snapshot = json.loads(zellij('pipe', '--name', 'snapshot', '--plugin', 'file:' + str(observer), '--', '{}'))
            assert original_id not in [p['pane_id'] for p in snapshot['panes']]
            menu = choose(token, 'exit', 'fork')
            assert menu['attributes']['answers'] == ['restart', 'fork', 'begin-review', 'run-tests', 'leave']
            until('hidden source resolved by Fork', lambda: 'forking claude' in zellij('action', 'dump-screen'))
            os.write(ui_fd, b'\x1b')
            until('Fork cancellation returns to exit question', lambda: pending(token, 'exit'))
            before_ids = {p['id'] for p in agents()}
            choose(token, 'exit', 'fork')
            until('Fork picker reopened', lambda: 'forking claude' in zellij('action', 'dump-screen'))
            os.write(ui_fd, b'\t\r')
            forked = until('Fork reuses the source and takes focus', lambda: next((p for p in agents() if p['id'] == original_id and not p['exited'] and not p['is_suppressed'] and p['is_focused']), None))
            assert forked['agent']['kind'] == 'codex'
            assert forked['agent']['cwd'] == str(topic)
            assert {p['id'] for p in agents()} == before_ids
            for field in ('tab_id', 'pane_x', 'pane_y', 'pane_rows', 'pane_columns', 'is_floating'):
                assert forked[field] == original[field], field
            print('PASS: automatic tiled exit suppresses source; cancelled Fork returns; confirmed Fork reuses source kind/cwd/layout and takes focus', flush=True)
            until('fork has enabled raw input', lambda: 'STUB READY' in zellij('action', 'dump-screen'))
            zellij('action', 'write-chars', '--pane-id', str(original_id), 'exit\n')
            until('fork exit hook suppresses source', lambda: next((p for p in agents() if p['id'] == original_id and p['exited'] and p['is_suppressed']), None))
            automatic = until('fork exit run', lambda: [r for r in swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(original_id))['results'] if r['inputs']['launch'] != token])
            token = automatic[0]['inputs']['launch']
            if args.exit_only:
                choose(token, 'exit', 'leave')
                print('PASS: the fork exits to a fresh menu in the original pane', flush=True)
                raise SystemExit(0)
            choose(token, 'exit', 'begin-review')
            until('automatic preparation menu', lambda: pending(token, 'prepare'))
            view = next(p for p in json.loads(zellij('action', 'list-panes', '--json'))
                        if p.get('plugin_url') == 'zellij:agent-exit' and not p['is_suppressed'])
            exercise_diff(token, view)
            choose(token, 'prepare', 'leave')
            print('PASS: pager replaces the automatic exit view at its original location and restores it', flush=True)
            raise SystemExit(0)

        if not args.test_output_only and not args.latency_only:
            launch('success')
            initial = until('initial exit menu', lambda: pending('success', 'exit'))
            assert initial['attributes']['answers'] == ['begin-review', 'run-tests', 'leave']
            assert initial['attributes']['prompt'] == 'Choose Begin review, Run tests, or Leave.'
            assert bridge(method='lookup', work='success', key='worker') is None
            assert not agents(), 'Exit must not start an agent'
            assert run('git', 'rev-parse', 'HEAD').stdout.strip() == base
            before = len(histories('success'))
            launch('success')
            assert len(histories('success')) == before, 'Duplicate launch dispatched a second run'
            choose('success', 'exit', 'begin-review')
            exercise_diff('success')
            choose('success', 'prepare', 'commit-and-rebase')
            choose('success', 'review', 'merge')
            q = until('informed skipped-check confirmation', lambda: pending('success', 'merge'))
            assert 'Tests: SKIPPED' in q['attributes']['prompt'] and 'Review: SKIPPED' in q['attributes']['prompt']
            assert all(p['exited'] for p in agents()), 'Only the completed commit agent is expected before optional review'
            zellij('workflow', 'show', q['attributes']['run'])
            until('confirmation rendered', lambda: b'Merge exact revision' in re.sub(rb'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]', b'', bytes(ui_output)))
            # Changed target invalidates this exact approval and returns to preparation.
            run('git', 'commit', '--allow-empty', '-m', 'target changed after inspection')
            target = run('git', 'rev-parse', 'HEAD').stdout.strip()
            choose('success', 'merge', 'merge')
            fresh = until('stale approval returns to preparation', lambda: pending('success', 'prepare'))
            assert fresh['attributes']['run'] != q['attributes']['run']
            assert run('git', 'rev-parse', 'HEAD').stdout.strip() == target
            choose('success', 'prepare', 'commit-and-rebase')
            choose('success', 'review', 'run-tests')
            choose('success', 'review', 'start-code-review')
            reviewed = until('unchanged reviewer confirmation', lambda: pending('success', 'merge'))
            assert 'Tests: PASSED' in reviewed['attributes']['prompt']
            assert 'Review: PASSED' in reviewed['attributes']['prompt']
            choose('success', 'merge', 'merge')
            choose('success', 'cleanup', 'keep')
            until('successful completion', lambda: all(r['status'] == 'succeeded' for r in histories('success')))
            assert topic.exists()
            assert run('git', 'rev-parse', 'HEAD').stdout == run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
            print('PASS: no unsolicited worker; diff is read-only; stale approval re-prepares; optional passing checks and merge/keep succeed', flush=True)

        release = root / 'release-live-command'
        def failure_block(name):
            return f'FAIL [ 0.01s] fixture {name}\n--- STDOUT: fixture {name} ---\n{name} failure-evidence\n'
        live_gate = ("import sys,time; from pathlib import Path; "
                     "print('LIVE BEFORE RELEASE 😀', flush=True); "
                     "print('LIVE STDERR', file=sys.stderr, flush=True); "
                     "release=Path(" + repr(str(release)) + "); "
                     "exec('while not release.exists(): time.sleep(0.05)'); "
                     "print(" + repr(failure_block('first')) + "); "
                     "sys.stdout.write(('𝄞' * 63 + '\\n') * 4800); "
                     "print(" + repr(failure_block('middle')) + "); "
                     "sys.stdout.write(('𝄞' * 63 + '\\n') * 4800); "
                     "print(" + repr(failure_block('last')) + "); sys.exit(1)")
        launch('failed-checks', gate=['/usr/bin/python3', '-c', live_gate])
        def check_output_previews(question, nested=False):
            snapshot = bridge(method='workflow_show', directory=str(topic), run=question['attributes']['run'])
            assert len(json.dumps(snapshot).encode()) < 1_048_576
            assert any(q['attributes'].get('question') == question['attributes']['question'] and
                       q['attributes']['state'] == 'pending' for q in snapshot['questions']['results'])
            seen = set()
            for entry in snapshot['outputs']['results']:
                attrs = entry['attributes']
                path = 'attributes.test.output' if attrs.get('test') else 'attributes.output'
                value = attrs.get('test') or attrs
                if 'output' not in value or (not value.get('previewTruncated') and not value.get('outputRef')):
                    continue
                exact = 'modelName == ' + json.dumps(entry['model']) + ' && name == ' + json.dumps(entry['name']) + ' && version == ' + str(entry['version'])
                saved = swamp('data', 'query', exact, '--select', path)['results'][0]
                if value.get('outputRef'):
                    ref = value['outputRef']
                    model_name = swamp('model', 'get', ref['modelId'])['name']
                    exact = 'modelType == ' + json.dumps(ref['modelType']) + ' && modelName == ' + json.dumps(model_name) + ' && name == ' + json.dumps(ref['name']) + ' && version == ' + str(ref['version'])
                    saved = swamp('data', 'query', exact, '--select', 'attributes.output')['results'][0]
                    assert saved.startswith(value['output'][:1024])
                else:
                    assert saved.startswith(value['output'])
                assert len(saved.encode()) > 2 * 1024 * 1024 and len(value['output']) <= 16384
                assert not any(0xD800 <= ord(c) <= 0xDFFF for c in value['output'])
                assert value['exitCode'] == 1
                seen.add(entry['name'].split('-')[0])
            assert 'test' in seen, seen
            if nested:
                assert 'checks' in seen, seen
            ids = [h['id'] for h in [snapshot['history']] + snapshot['children']]
            requests = swamp('data', 'query', 'specName == "operation" && tags.workflowRunId in ' + json.dumps(ids), '--select', 'attributes.request')['results']
            requests = [json.loads(r) for r in requests]
            assert all(r['method'] != 'agent_spawn' or r['key'].endswith('-commit-agent')
                       for r in requests), 'Failed tests must not automatically start a repair agent'
            entry = next(e for e in snapshot['outputs']['results'] if e['attributes'].get('summaryLength', 0))
            predicate = 'modelName == ' + json.dumps(entry['model']) + ' && name == ' + json.dumps(entry['name']) + ' && version == ' + str(entry['version'])
            saved = swamp('data', 'query', predicate, '--select', 'attributes')['results'][0]
            assert saved['outputComplete'] and not saved['captureError']
            assert saved['summaryLength'] == len(saved['summary'].encode('utf-16-le')) // 2
            assert saved['failedTests'] == ['fixture first', 'fixture last', 'fixture middle']
            for name in ('first', 'middle', 'last'):
                assert name + ' failure-evidence' in saved['summary'] and name + ' failure-evidence' in saved['output']
            offset, chunks = 0, []
            while offset < entry['attributes']['summaryLength']:
                chunk = bridge(method='workflow_report', directory=str(main), model=entry['model'], name=entry['name'], version=entry['version'], offset=offset)
                assert len(json.dumps(chunk).encode()) < 1_048_576 and chunk['next'] > offset
                chunks.append(chunk['text'])
                offset = chunk['next']
            assert ''.join(chunks) == saved['summary'], 'Chunked view lost report content'
            return saved['summary']


        review_question = prepare('failed-checks')
        # Earlier scenarios may leave completed views open. Start this external-close check with one view.
        for pane in json.loads(zellij('action', 'list-panes', '--json')):
            if pane.get('plugin_url') == 'zellij:swamp-workflow':
                zellij('action', 'close-pane', '--pane-id', 'plugin_' + str(pane['id']))
        frame_start = len(ui_output)
        zellij('workflow', 'show', review_question['attributes']['run'])
        until('review buttons rendered', lambda: b'Run tests' in bytes(ui_output[frame_start:]))
        clicked = time.monotonic()
        if args.test_output_only:
            # Report scenarios approve through the bridge; latency checks cover button feedback.
            choose('failed-checks', 'review', 'run-tests', dismiss=False)
            feedback = None
        else:
            frame_start = len(ui_output)
            os.write(ui_fd, b'\r')
            # A fast acknowledgement can coalesce the transient submitting frame.
            until('immediate submission feedback', lambda: any(text in bytes(ui_output[frame_start:])
                  for text in (b'Submitting answer', b'Answer recorded')), interval=0.005)
            feedback = time.monotonic() - clicked
            until('answer recorded', lambda: b'Answer recorded' in bytes(ui_output[frame_start:]), interval=0.005)
        submission = time.monotonic() - clicked
        if not args.latency_baseline:
            # Output from another stream may replace this frame before a CLI snapshot arrives.
            until('live output before command exit', lambda: b'LIVE BEFORE RELEASE' in bytes(ui_output))
        assert not release.exists()
        for pane in json.loads(zellij('action', 'list-panes', '--json')):
            if pane.get('plugin_url') == 'zellij:swamp-workflow':
                zellij('action', 'close-pane', '--pane-id', 'plugin_' + str(pane['id']))
        until('view externally closed', lambda: not any(p.get('plugin_url') == 'zellij:swamp-workflow'
              for p in json.loads(zellij('action', 'list-panes', '--json'))))
        frame_start = len(ui_output)
        zellij('workflow', 'show', review_question['attributes']['run'])
        until('view reattached', lambda: b'Cancel run' in bytes(ui_output[frame_start:]))
        assert not release.exists(), 'Closing the pane externally must not release or stop execution'
        released = time.monotonic()
        release.touch()
        dismiss_command('failed-checks', 'tests')
        until('next question rendered', lambda: 'Start fix agent' in zellij('action', 'dump-screen'), interval=0.005)
        until('failure summary displayed before execution details', lambda: 'Test report: FAILED' in zellij('action', 'dump-screen'))
        timings = dict(click_to_feedback_seconds=feedback, submission_seconds=submission,
                       release_to_next_question_seconds=time.monotonic()-released,
                       child_decisions=len(histories('failed-checks')))
        (root / 'latency.json').write_text(json.dumps(timings, indent=2))
        print('LATENCY:', timings, flush=True)
        if args.latency_only:
            raise SystemExit(0)
        for pane in json.loads(zellij('action', 'list-panes', '--json')):
            if pane.get('plugin_url') == 'zellij:swamp-workflow':
                zellij('action', 'close-pane', '--pane-id', 'plugin_' + str(pane['id']))
        failed = until('failed-test choices', lambda: pending('failed-checks', 'failed-tests'))
        assert failed['attributes']['answers'] == ['run-tests', 'start-fix-agent', 'return-to-menu', 'leave']
        complete_summary = check_output_previews(failed)
        frame_start = len(ui_output)
        zellij('workflow', 'show', failed['attributes']['run'])
        until('completed report reattached', lambda: 'Test report: FAILED' in zellij('action', 'dump-screen'))
        choose('failed-checks', 'failed-tests', 'return-to-menu')
        choose('failed-checks', 'review', 'merge')
        informed = until('failed-check merge confirmation', lambda: pending('failed-checks', 'merge'))
        assert 'Tests: FAILED' in informed['attributes']['prompt']
        check_output_previews(informed, nested=True)
        def confirmation_report():
            report = swamp('report', 'get', '@hammz/workflow', '--workflow', '@hammz/zellij-review-integration')['json']
            return report if report.get('run') == informed['attributes']['run'] else None
        # Reports are published after execution, not while approval is suspended.
        # Keep cleanup pending until the exact confirmation report is inspected.
        choose('failed-checks', 'merge', 'merge')
        until('cleanup decision pending', lambda: pending('failed-checks', 'cleanup'))
        report = until('confirmation report saved', confirmation_report)
        assert not report.get('error'), report
        assert complete_summary in report['testReports']
        choose('failed-checks', 'cleanup', 'keep')
        print('PASS: verbose failed-test previews and merge confirmation; full logs retained; no automatic repair', flush=True)
        for token, gate in [('fix', ['/bin/sh', '-c', 'printf failure-evidence; exit 1']),
                            ('large-fix', ['/usr/bin/python3', '-c', live_gate])]:
            launch(token, gate=gate)
            prepare(token)
            choose(token, 'review', 'run-tests')
            choose(token, 'failed-tests', 'start-fix-agent')
            until('fix exit returns to preparation for its edits', lambda: pending(token, 'prepare'))
            choose(token, 'prepare', 'leave')
            assert topic.exists()
            received = (topic / 'received-failure-output').read_text()
            prompt = (topic / 'received-failure-prompt').read_text()
            assert 'failure-evidence' in received and len(prompt.encode()) <= 262144
            if token == 'large-fix':
                for name in ('first', 'middle', 'last'):
                    assert name + ' failure-evidence' in received
                assert 'swamp data query ' in prompt and 'version == 1' in prompt
            else:
                assert 'Test report: FAILED' in prompt
            (topic / 'received-failure-output').unlink()
            (topic / 'received-failure-prompt').unlink()
        print('PASS: approved fix receives failure output and exits to a fresh menu; Leave preserves checkout', flush=True)
        if args.test_output_only:
            launch('passing-report')
            passing_review = prepare('passing-report')
            zellij('workflow', 'show', passing_review['attributes']['run'])
            until('passing gate workflow visible', lambda: 'Run tests' in zellij('action', 'dump-screen'))
            choose('passing-report', 'review', 'run-tests')
            q = until('passing report menu', lambda: pending('passing-report', 'review'))
            frame_start = len(ui_output)
            zellij('workflow', 'show', q['attributes']['run'])
            until('success summary displayed', lambda: 'Test report: PASSED' in zellij('action', 'dump-screen'))
            choose('passing-report', 'review', 'leave')
            print('PASS: success report displayed', flush=True)
            raise SystemExit(0)

        launch('review-changes', agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit', '--edit-review'])
        prepare('review-changes')
        choose('review-changes', 'review', 'start-code-review')
        choose('review-changes', 'exit', 'leave')
        assert (topic / 'reviewer-change').read_text() == 'reviewer edit\n'
        (topic / 'reviewer-change').unlink()
        print('PASS: reviewer untracked changes return to the initial menu', flush=True)

        original = next(p for p in agents() if p.get('exited') and p['agent']['kind'] == 'claude')
        launch('fork-cancel', sourcePane=original['id'], kind='claude')
        first_menu = choose('fork-cancel', 'exit', 'fork')
        assert first_menu['attributes']['answers'] == ['restart', 'fork', 'run-tests', 'leave']
        assert first_menu['attributes']['prompt'] == 'Agent exited. Choose Restart, Fork, Run tests, or Leave.'
        until('existing fork picker', lambda: any(p.get('plugin_url') == 'zellij:hocus-new-agent' and not p['is_suppressed'] for p in json.loads(zellij('action', 'list-panes', '--json'))))
        zellij('action', 'start-or-reload-plugin', 'zellij:swamp-bridge')
        until('picker dispatch becomes uncertain after reload', lambda: bridge(method='observe', work='fork-cancel', key=first_menu['attributes']['run'] + '-pane-choice')['outcome'] == 'unknown')
        os.write(ui_fd, b'\x1b')
        cancelled = until('cancelled fork returns to fresh exit menu', lambda: pending('fork-cancel', 'exit'))
        assert cancelled['attributes']['run'] != first_menu['attributes']['run']
        choose('fork-cancel', 'exit', 'leave')
        until('Leave closes the exited source', lambda: not any(p['id'] == original['id'] for p in agents()))
        original = next(p for p in agents() if p.get('exited') and p['agent']['kind'] == 'claude')
        launch('restart-choice', sourcePane=original['id'], kind='claude')
        choose('restart-choice', 'exit', 'restart')
        until('restart uses original pane', lambda: any(p['id'] == original['id'] and not p['exited'] for p in agents()))
        print('PASS: fork cancellation returns to a fresh menu; restart reuses original pane', flush=True)

        exercise_exit_menus()
        decision_tests_smoke()

        launch('unresolved-review', agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit', '--findings'])
        prepare('unresolved-review')
        choose('unresolved-review', 'review', 'start-code-review')
        unresolved = until('unresolved review confirmation', lambda: pending('unresolved-review', 'merge'))
        assert 'Review: FAILED / unresolved findings' in unresolved['attributes']['prompt']
        choose('unresolved-review', 'merge', 'merge')
        choose('unresolved-review', 'cleanup', 'keep')
        print('PASS: informed merge accepts unresolved review findings', flush=True)

        rebase_smoke()

        zellij('workflow', 'run', '@hammz/zellij-question', '--launch', 'recovery', '--no-view', '--input', json.dumps(dict(kind='text', prompt='Instructions?', answers=[])))
        question = until('text question', lambda: pending('recovery', 'question'))
        (root / 'interrupt-after-approval').touch()
        failed = run(binary, '--session', session, 'workflow', 'answer', question['attributes']['run'], question['model'], 'question', 'Recorded instructions', '--submission', 'recorded-once', check=False)
        assert failed.returncode
        swamp('report', 'get', '@swamp/method-summary', '--model', question['model'])
        bridge(method='swamp_dispatch', operation='model.method.run', work='recovery', key='recover', directory=str(main), payload=dict(modelIdOrName=question['model'], methodName='recover', inputs=dict(run=question['attributes']['run'], question='question')))
        until('recorded answer recovery', lambda: all(r['status'] == 'succeeded' for r in histories('recovery')))
        duplicate = run(binary, '--session', session, 'workflow', 'answer', question['attributes']['run'], question['model'], 'question', 'Recorded instructions', '--submission', 'recorded-once', check=False)
        assert duplicate.returncode
        swamp('report', 'get', '@swamp/method-summary', '--model', question['model'])
        print('PASS: interrupted approval recovers once; duplicate answer refused', flush=True)

    owned = bridge(method='agent_spawn', work='restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--wait'], cwd=str(topic), source_directory=str(topic)))
    bridge(method='agent_prompt', work='restart', key='pending', agent='worker', prompt='Wait for recovery')
    zellij('action', 'start-or-reload-plugin', 'zellij:swamp-bridge')
    observed = until('dispatcher restart', lambda: (lambda o: o if o['outcome'] == 'unknown' else None)(bridge(method='observe', work='restart', key='pending')))
    reconciled = bridge(method='reconcile', work='restart', key='reconcile', operation='worker')
    assert reconciled['supervisor'] == owned['supervisor'] and reconciled['pane'] == owned['pane']
    assert observed['outcome'] == 'unknown'
    assert bridge(method='agent_spawn', work='restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--wait'], cwd=str(topic), source_directory=str(topic)))['pane'] == owned['pane']
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
    held_workflow=swamp('workflow', 'create', 'server-survival')
    held_release=root / 'release-server-survival'
    held_started=root / 'started-server-survival'
    Path(held_workflow['path']).write_text(json.dumps(dict(id=held_workflow['id'], name=held_workflow['name'], version=1,
        jobs=[dict(name='main', steps=[dict(name='held', task=dict(type='model_method', modelType='command/shell', modelName='server-survival', methodName='execute',
            inputs=dict(run='touch ' + str(held_started) + '; while [ ! -f ' + str(held_release) + ' ]; do sleep 0.05; done', workingDir=str(main), timeout=120000)))])])))
    held=bridge(method='swamp_dispatch', operation='workflow.run', work='server-survival', key='held', directory=str(main), payload=dict(workflowIdOrName=held_workflow['name'], noSupersede=True))
    until('Swamp execution held before server crash', held_started.exists)
    swamp_pid=json.loads(run(binary, 'workflow', 'server', 'status').stdout)['pid']
    os.kill(server_pids[0], signal.SIGKILL)
    until('supervisor survives server crash', lambda: any(h['host_id'] == survivor['supervisor'] and not h['attached'] for h in json.loads(zellij('pty-host', 'list', '--json'))))
    run(binary, 'attach', '--create-background', session)
    until('restored session accepts commands', lambda: json.loads(zellij('action', 'list-tabs', '--json')))
    assert json.loads(run(binary, 'workflow', 'server', 'status').stdout)['pid']==swamp_pid
    assert not held_release.exists()
    assert swamp('workflow', 'history', 'get', held['run'])['status']=='running'
    assert bridge(method='observe', work='server-survival', key='held')['outcome']=='pending'
    held_release.touch()
    until('surviving Swamp execution completes', lambda: bridge(method='observe', work='server-survival', key='held')['outcome']=='completed')
    assert len(swamp('workflow', 'history', 'search', '--workflow', held_workflow['name'])['results'])==1
    print('PASS: Zellij server crash leaves active Swamp execution alive; reattachment completes without duplication', flush=True)
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
    (root / 'release-live-command').touch()
    (root / 'release-server-survival').touch()
    # Runs outlive the session; stop the fixture service separately once idle.
    run(binary, 'workflow', 'server', 'stop', check=False)
    run(binary, 'kill-session', session, check=False)
    if ui_pid: os.waitpid(ui_pid, 0)
    if ui_fd is not None: os.close(ui_fd)
    transcript.close()
    launches=[json.loads(line) for line in (root / 'swamp-processes.jsonl').read_text().splitlines()]
    operations=[entry for entry in launches if entry['parent'] in ('zellij', 'swamp') and entry['command'][0] in ('workflow', 'model', 'data')]
    assert not operations, operations
    print('PASS: zero per-operation Swamp CLI launches from Zellij or Swamp', flush=True)
    print('Retained evidence:', root, flush=True)
