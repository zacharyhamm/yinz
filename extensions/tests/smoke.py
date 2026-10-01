#!/usr/bin/env python3
"""Disposable real-server acceptance: python3 extensions/tests/smoke.py [--binary PATH].

Requires the acknowledged extension review, Swamp, Git, and an unrestricted local
socket environment. All Git changes and approvals are confined to a fresh /tmp repo.
The repository and command transcripts remain available after the session stops.
"""
import argparse
import atexit
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
parser.add_argument('--binary', default='target/debug/yinz')
parser.add_argument('--firecracker-only', action='store_true')
parser.add_argument('--validation-loop-only', action='store_true')
parser.add_argument('--cancellation-only', action='store_true')
parser.add_argument('--recovery-only', action='store_true')
parser.add_argument('--source-diff-only', action='store_true')
parser.add_argument('--commit-rebase-only', action='store_true')
parser.add_argument('--rebase-only', action='store_true')
parser.add_argument('--lock-only', action='store_true', help='Contended checkout lock: visible holder, release, and Ctrl-C')
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
root = Path(tempfile.mkdtemp(prefix='yinz-workflow-acceptance-'))
main, topic = root / 'main', root / 'topic'
main.mkdir()
binary = root / 'yinz'
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
    env['YINZ_SWAMP_TIMING'] = '1'
    root.chmod(0o700)
swamp_config = Path(env.get('XDG_CONFIG_HOME', str(Path.home() / '.config'))) / 'swamp'
if args.firecracker_only and not swamp_config.exists():
    swamp_config = Path.home() / '.config/swamp'
for name in ('YINZ', 'YINZ_SESSION_NAME', 'YINZ_PANE_ID', 'YINZ_PANE_REF', 'YINZ_CONFIG_FILE', 'YINZ_CONFIG_DIR', 'SWAMP_REPO_DIR', 'SWAMP_SERVER_URL', 'GIT_PAGER', 'PAGER', 'LESS'):
    env.pop(name, None)
for name, directory in [('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                        ('XDG_CACHE_HOME', 'cache'), ('XDG_RUNTIME_DIR', 'runtime'),
                        ('YINZ_SOCKET_DIR', 'sock'), ('TMPDIR', 'tmp')]:
    env[name] = str(root / directory)
    (root / directory).mkdir()
env.update(YINZ_CONFIG_DIR=str(root / 'config/yinz'),
           YINZ_CONFIG_FILE=str(root / 'config/yinz/config.kdl'))
# Serve checks the existing Swamp login; keep that host credential store available
# while isolating Yinz's configuration. Never copy credentials into transcripts.
if swamp_config.exists():
    (root / 'config/swamp').symlink_to(swamp_config, target_is_directory=True)
env['SWAMP_SMOKE_YINZ'] = str(binary)
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
fixture_claude.write_text('#!/usr/bin/python3\nimport os, sys\nos.execv("/usr/bin/python3", ["/usr/bin/python3", ' + repr(str(main / 'extensions/tests/stub_agent.py')) + ', "--exit"] + sys.argv[1:])\n')
fixture_claude.chmod(0o755)
fixture_codex = fixture_bin / 'codex'
fixture_codex.write_text(fixture_claude.read_text().replace('import os, sys\n', 'import os, sys, json\nos.environ["SWAMP_SMOKE_KIND"] = "codex"\nwith open(os.path.join(os.environ["SWAMP_SMOKE_ROOT"], "codex-argv.jsonl"), "a") as log: log.write(json.dumps(sys.argv[1:]) + "\\n")\n'))
fixture_codex.chmod(0o755)

env.update(PATH=str(fixture_bin) + os.pathsep + env['PATH'],
           SWAMP_SMOKE_REAL=real_swamp, SWAMP_SMOKE_ROOT=str(root))
(root / 'environment.json').write_text(json.dumps({k: env[k] for k in ('XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_RUNTIME_DIR','YINZ_SOCKET_DIR','SWAMP_SMOKE_YINZ')}, indent=2))
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


def yinz(*argv):
    return run(binary, '--session', session, *argv, cwd=topic if topic.exists() else main).stdout


def bridge(**request):
    result = json.loads(yinz('pipe', '--name', 'swamp', '--plugin', 'yinz:swamp-bridge', '--', json.dumps(request)))
    if 'error' in result:
        raise RuntimeError(result['error'])
    return result['result']


def fixture_processes():
    """Every live process that runs this fixture's binary or names its root on its command line.

    Supervisors are `setsid`'d and outlive the server on purpose, the managed Swamp
    server is its own session, and stub agents fork sleepers: none of them are
    reachable by parent pid, so the fixture root is the only thing they share."""
    found = []
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid(): continue
        try:
            argv = (proc / 'cmdline').read_bytes()
            exe = os.readlink(str(proc / 'exe'))
        except OSError:
            continue
        if exe.startswith(str(root) + os.sep) or str(root).encode() in argv:
            found.append((int(proc.name), argv.replace(b'\0', b' ').decode(errors='replace').strip()))
    return found


def signal_until_gone(pids, grace):
    """SIGTERM every pid (its whole process group when it leads one), then SIGKILL survivors."""
    def deliver(pid, sig):
        try:
            target = -pid if os.getpgid(pid) == pid else pid
        except OSError:
            return
        try:
            os.kill(target, sig)
        except OSError:
            pass
    for pid in pids: deliver(pid, signal.SIGTERM)
    deadline = time.monotonic() + grace
    while time.monotonic() < deadline and any(Path('/proc/' + str(pid)).exists() for pid in pids):
        time.sleep(0.1)
    for pid in pids:
        if Path('/proc/' + str(pid)).exists(): deliver(pid, signal.SIGKILL)


teardown_done = False
ui_pid = ui_fd = None


def teardown():
    """Leave no fixture process behind, whatever state the run reached.

    Each step stands alone: the server may be gone, the Swamp service may refuse to
    stop while an execution is held. `kill-session` closes attached panes and sweeps
    detached supervisors left by earlier incarnations of this session. Verify that
    shutdown before fallback cleanup can hide any leaked processes."""
    global teardown_done
    if teardown_done: return
    teardown_done = True
    steps = []
    def step(label, action):
        try:
            action()
        except Exception as error:
            steps.append((label, error))
            transcript.write('teardown {}: {!r}\n'.format(label, error))
            transcript.flush()
    def release_files():
        try:
            (root / 'hold-review').unlink()
        except FileNotFoundError:
            pass
        (root / 'release-live-command').touch()
        (root / 'release-server-survival').touch()
    def reap_ui():
        global ui_pid, ui_fd
        if ui_pid:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and os.waitpid(ui_pid, os.WNOHANG) == (0, 0):
                time.sleep(0.1)
            if Path('/proc/' + str(ui_pid)).exists():
                try:
                    os.kill(ui_pid, signal.SIGKILL)
                    os.waitpid(ui_pid, 0)
                except OSError:
                    pass
            ui_pid = 0
        if ui_fd is not None:
            os.close(ui_fd)
            ui_fd = None
    clean = False
    try:
        step('release', release_files)
        step('kill-session', lambda: run(binary, 'kill-session', session))
        step('workflow server stop', lambda: run(binary, 'workflow', 'server', 'stop'))
        # The shared SSH bootstrap master expires after ControlPersist=60 seconds.
        deadline = time.monotonic() + (70 if args.firecracker_only else 10)
        while True:
            survivors = fixture_processes()
            supervisors = json.loads(run(binary, 'pty-host', 'list', '--json').stdout)
            if not isinstance(supervisors, list):
                raise ValueError('supervisor listing must be a JSON array')
            if not survivors and not supervisors or time.monotonic() >= deadline:
                break
            time.sleep(0.1)
        # Capture the verification result before any forced cleanup.
        clean = not (steps or survivors or supervisors)
        if not clean:
            print('FAIL: normal shutdown left processes or reported errors:', flush=True)
            for pid, argv in survivors: print('  {} {}'.format(pid, argv[:160]), flush=True)
            for host in supervisors: print('  supervisor {}'.format(host), flush=True)
    except Exception as error:
        steps.append(('verification', error))
        transcript.write('teardown verification: {!r}\n'.format(error))
        transcript.flush()
    finally:
        step('pty-host kill-all', lambda: run(binary, 'pty-host', 'kill-all'))
        step('client', reap_ui)
        step('sweep', lambda: signal_until_gone([pid for pid, _ in fixture_processes()], 5))
    if steps: print('teardown steps failed:', steps, flush=True)
    if clean: print('PASS: normal shutdown left no fixture processes', flush=True)
    return clean


atexit.register(teardown)


def teardown_on_signal(signum, frame):
    teardown()
    signal.signal(signum, signal.SIG_DFL)
    os.kill(os.getpid(), signum)


for signum in (signal.SIGTERM, signal.SIGHUP):
    signal.signal(signum, teardown_on_signal)


def attached_pane(resource):
    host = next((h for h in json.loads(yinz('pty-host', 'list', '--json')) if h['host_id'] == resource['supervisor']), None)
    return host and host['attached'] and any(p['id'] == host['terminal_id'] and not p['is_plugin'] for p in json.loads(yinz('action', 'list-panes', '--json')))


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
    return yinz('workflow', 'run', '@hammz/yinz-agent-exit', '--launch', token, '--no-view', '--input', json.dumps(default))


def answer(q, value, token):
    a = q['attributes']
    return yinz('workflow', 'answer', a['run'], q['model'], a['question'], value, '--submission', token)


def command_pane(token, suffix):
    for item in histories(token):
        operation = bridge(method='lookup', work=token, key=item['runId'] + '-' + suffix)
        if operation and operation.get('outcome') == 'pending' and operation.get('pane') is not None:
            return next((p for p in json.loads(yinz('action', 'list-panes', '--json'))
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
          yinz('action', 'dump-screen', '--pane-id', pane_id))
    yinz('action', 'write-chars', '--pane-id', pane_id, '\n')
    if suffix == 'tests':
        until('completed test pane closed', lambda: not any(
            not p['is_plugin'] and p['id'] == pane['id']
            for p in json.loads(yinz('action', 'list-panes', '--json'))))
        # Further input to the old pane cannot restart its capture command.
        run(binary, '--session', session, 'action', 'write-chars', '--pane-id', pane_id, '\n', check=False)
        assert not any(not p['is_plugin'] and p['id'] == pane['id']
                       for p in json.loads(yinz('action', 'list-panes', '--json')))


def dismiss_commit(token):
    dismiss_command(token, 'commit-and-rebase')


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
run('swamp', 'extension', 'pull', '@swamp/git@2026.08.28.1', '--json')
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
(main / '.yinz').mkdir()
shutil.copy(source / '.yinz/swamp-workflows.kdl', main / '.yinz/swamp-workflows.kdl')
if args.commit_rebase_only:
    binding = main / '.yinz/swamp-workflows.kdl'
    binding.write_text(binding.read_text().replace('agent_exit {', 'agent_exit {\n    commit_agent kind="codex" model="commit-model" effort="medium"'))
(main / '.git/info/exclude').write_text('.swamp*\n.gitignore\nextensions/\nworkflows/\nmodels/\n.yinz/\nAGENTS.md\nCLAUDE.md\n')
for name in ('agent-work', 'review-integration', 'question', 'agent-exit'):
    swamp('workflow', 'validate', '@hammz/yinz-' + name)
    if name in ('agent-exit', 'review-integration'):
        definition = swamp('workflow', 'get', '@hammz/yinz-' + name)
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
    workflow = swamp('workflow', 'get', '@hammz/yinz-review-integration')
    diff = next(s['task']['inputs'] for j in workflow['jobs'] for s in j['steps'] if s['name'] == 'diff')
    assert diff['cwd'] == '${{ inputs.checkout }}'
    for name in ('agent-exit', 'review-integration'):
        definition = swamp('workflow', 'get', '@hammz/yinz-' + name)
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
    argv = [str(topic) if a == '${{ inputs.checkout }}' else base if 'data.latest' in a else a for a in diff['argv']]

    def capture(overrides=None, checkout=topic):
        result = subprocess.run(argv[:-2] + [str(checkout), base], cwd=main,
                                env=dict(viewer_env, **(overrides or {})),
                                text=True, capture_output=True, timeout=30)
        assert not list(viewer_tmp.iterdir()), 'Viewer must remove all temporary files'
        return result

    (topic / 'file').write_text('base\n')
    clean = capture()
    assert clean.returncode == 0, clean.stderr
    assert 'Committed branch changes:' in clean.stdout and 'Staged changes:' in clean.stdout
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
    (topic / 'second').write_text('AGENT STAGED\n')
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
        assert all(label in result.stdout for label in ('AGENT TRACKED', 'AGENT UNTRACKED', 'AGENT STAGED'))
        assert not any(label in result.stdout for label in ('MAIN', 'SIBLING', 'yinz-show-diff.'))
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
(root / 'config/yinz').mkdir(exist_ok=True)
(root / 'config/yinz/config.kdl').write_text('builtin_plugins_dir ' + json.dumps(str(builtins)) + '\nfirecracker_provider ' + json.dumps(str(source / 'target/debug/yinz-firecracker')) + '\n')
observer = root / 'snapshot.ts'
observer.write_text('''import z, {definePlugin} from 'yinz:plugin';
export default definePlugin({
  load() { z.requestPermission(['MessageAndLaunchOtherPlugins','ReadCliPipes']); },
  async pipe(message) {
    if (message.name !== 'snapshot' || typeof message.source !== 'object' || !('Cli' in message.source)) return;
    const cli=message.source.Cli;
    z.host.blockCliPipeInput(cli);
    try {
      const snapshot=z.waitForPipe('agent_snapshot');
      z.command('MessageToPlugin',{plugin_url:'yinz:hocus-agentd',message_name:'agent_subscribe',
        destination_plugin_id:null,plugin_config:{},message_payload:null,message_args:{},new_plugin_args:null,floating_pane_coordinates:null});
      z.host.cliPipeOutput(cli,(await snapshot).payload ?? '');
    } finally {z.host.unblockCliPipeInput(cli);}
  }
});
''')
(root / 'cache/yinz').mkdir(exist_ok=True)
(root / 'cache/yinz/permissions.kdl').write_text(json.dumps(str(observer)) + ' {\n MessageAndLaunchOtherPlugins\n ReadCliPipes\n}\n')
if args.firecracker_only:
    provider_config = source / 'target/firecracker-test/config/yinz/firecracker.json'
    shutil.copy(provider_config, root / 'config/yinz/firecracker.json')
    # Config names the existing immutable image; VM state stays in this fixture.
cold_start_ms = None
if args.timing_only or args.transition_timing_only:
    started = time.monotonic()
    run(binary, 'workflow', 'server', 'restart')
    cold_start_ms = (time.monotonic() - started) * 1000
def validation_loop_smoke():
    def choose(token, value, question='prepare', dismiss=True):
        print('CHOICE:', token, question, value, flush=True)
        q = until(token + ': ' + question, lambda: pending(token, question))
        answer(q, value, 'answer-' + q['attributes']['run'])
        suffix = {'commit-and-rebase': 'commit-and-rebase', 'continue-rebase': 'continue-rebase',
                  'run-tests': 'tests', 'retry-tests': 'tests', 'remove-clean-worktree': 'cleanup'}.get(value)
        if value == 'merge' and question == 'merge': suffix = 'integrate'
        if suffix and dismiss: dismiss_command(token, suffix)
        if value in ('leave', 'leave-paused', 'keep'):
            def finished():
                runs = histories(token)
                return runs and all(item['status'] == 'succeeded' for item in runs)
            until(token + ': finished', finished)
        return q

    def menu(token):
        return until(token + ': menu', lambda: pending(token, 'prepare'))

    def check_state(token):
        q = menu(token)
        records = swamp('data', 'query', 'specName == "checks" && modelName == ' + json.dumps(q['attributes']['run'] + '-checks'), '--select', 'attributes')['results']
        assert len(records) == 1
        return records[0]

    if not (args.cancellation_only or args.rebase_only):
        token = 'validation-loop'
        # Review and viewer cover committed, staged, unstaged and untracked contents together.
        (topic / 'committed').write_text('COMMITTED REVIEW SCOPE\n')
        run('git', 'add', 'committed', cwd=topic)
        run('git', 'commit', '-m', 'committed branch change', cwd=topic)
        (topic / 'second').write_text('STAGED REVIEW SCOPE\n')
        run('git', 'add', 'second', cwd=topic)
        (topic / 'second').write_text('UNSTAGED REVIEW SCOPE\n')
        (topic / 'untracked file').write_text('UNTRACKED REVIEW SCOPE\n')
        spec = lambda *flags: dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--exit', *flags])
        roles = dict(reviewAgent=spec('--findings-until-fixed'), reviewFixAgent=spec('--repair-review'), testFixAgent=spec('--repair-tests'))
        gate = ['/usr/bin/python3', '-c', 'from pathlib import Path; import sys; '
                'print("test failure-evidence BEGIN\\n" + "diagnostic\\n" * 18000 + "failure-evidence END", flush=True); '
                'sys.exit(0 if Path("gate-fixed").exists() else 1)']
        launch(token, gate=gate, **roles)
        first = menu(token)
        assert first['attributes']['answers'] == ['show-diff', 'review', 'commit-and-rebase', 'run-tests', 'merge', 'leave']
        before = run('git', 'status', '--porcelain=v1', cwd=topic).stdout
        choose(token, 'show-diff', dismiss=False)
        pane = until('diff pane', lambda: command_pane(token, 'diff'))
        pane_id = 'terminal_' + str(pane['id'])
        yinz('action', 'write-chars', '--pane-id', pane_id, 'G')
        until('diff full scope', lambda: 'UNTRACKED REVIEW SCOPE' in yinz('action', 'dump-screen', '--full', '--pane-id', pane_id))
        yinz('action', 'write-chars', '--pane-id', pane_id, 'q')
        menu(token)
        assert run('git', 'status', '--porcelain=v1', cwd=topic).stdout == before
        choose(token, 'review')
        state = check_state(token)
        assert state['review']['valid'] and not state['review']['resolved']
        assert not state['evidence']['source']['clean']
        choose(token, 'fix-review-with-agent')
        assert check_state(token)['review']['outcome'] == 'stale'
        assert 'Unresolved fixture finding' in (topic / 'review-fix-evidence').read_text()
        choose(token, 'review')
        assert check_state(token)['review']['resolved']
        hook_dir = root / 'failing-hooks'
        hook_dir.mkdir()
        hook = hook_dir / 'pre-commit'
        hook.write_text('#!/bin/sh\necho COMMIT REFUSED BY FIXTURE\nexit 1\n')
        hook.chmod(0o755)
        run('git', 'config', 'core.hooksPath', hook_dir)
        before_commit = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
        choose(token, 'commit-and-rebase')
        assert not check_state(token)['evidence']['source']['rebasing']
        assert run('git', 'rev-parse', 'HEAD', cwd=topic).stdout == before_commit
        run('git', 'config', '--unset', 'core.hooksPath')
        choose(token, 'commit-and-rebase')
        assert check_state(token)['review']['outcome'] == 'stale'
        assert not run('git', 'status', '--porcelain=v1', cwd=topic).stdout
        assert 'Keep literal $(touch INJECTED) text.' in run('git', 'log', '-1', '--format=%B', cwd=topic).stdout
        choose(token, 'run-tests')
        assert not check_state(token)['test']['passed']
        choose(token, 'fix-tests-with-agent')
        assert check_state(token)['test']['outcome'] == 'stale'
        evidence = (topic / 'received-failure-output').read_text()
        assert 'failure-evidence BEGIN' in evidence and 'failure-evidence END' in evidence
        choose(token, 'commit-and-rebase')
        menu(token)
        choose(token, 'run-tests')
        assert check_state(token)['test']['passed']
        # A test pass alone is insufficient; no confirmation is offered.
        choose(token, 'merge')
        assert check_state(token)['review']['outcome'] == 'stale' and pending(token, 'merge') is None
        choose(token, 'review')
        state = check_state(token)
        assert state['review']['resolved'] and state['test']['passed']
        for key, spec_value in roles.items():
            assert all(r['inputs'][key] == spec_value for r in histories(token))
        choose(token, 'merge')
        q = until('exact revision approval', lambda: pending(token, 'merge'))
        assert state['evidence']['source']['commit'] in q['attributes']['prompt'] and state['evidence']['target']['commit'] in q['attributes']['prompt']
        run('git', 'commit', '--allow-empty', '-m', 'target moved after approval')
        moved = run('git', 'rev-parse', 'HEAD').stdout
        choose(token, 'merge', 'merge')
        menu(token)
        assert run('git', 'rev-parse', 'HEAD').stdout == moved
        choose(token, 'commit-and-rebase')
        menu(token)
        choose(token, 'review')
        menu(token)
        choose(token, 'run-tests')
        menu(token)
        choose(token, 'merge')
        choose(token, 'merge', 'merge')
        until('separate cleanup choice', lambda: pending(token, 'cleanup'))
        assert run('git', 'rev-parse', 'HEAD').stdout == run('git', 'rev-parse', 'HEAD', cwd=topic).stdout
        cleanup_question = pending(token, 'cleanup')
        (topic / 'late-cleanup').write_text('preserve dirty worktree\n')
        choose(token, 'remove-clean-worktree', 'cleanup')
        retried = until('refused cleanup returns to its guarded choice', lambda: pending(token, 'cleanup'))
        assert retried['attributes']['run'] != cleanup_question['attributes']['run']
        assert (topic / 'late-cleanup').read_text() == 'preserve dirty worktree\n'
        (topic / 'late-cleanup').unlink()
        choose(token, 'keep', 'cleanup')
        print('PASS: dirty review, repairs, manual retries, complete reports, commit messages, exact merge evidence, and role continuations', flush=True)

        for mode in ('malformed-review', 'malformed-commit'):
            checkout = root / mode
            run('git', 'worktree', 'add', '-b', mode, checkout)
            (checkout / 'dirty').write_text('uncommitted\n')
            role = 'reviewAgent' if mode.endswith('review') else 'commitAgent'
            launch(mode, checkout=str(checkout), **{role: spec('--malformed' if mode.endswith('review') else '--malformed-commit')})
            choose(mode, 'review' if mode.endswith('review') else 'commit-and-rebase', dismiss=False)
            state = check_state(mode)
            if mode.endswith('review'): assert not state['review']['valid']
            assert run('git', 'status', '--porcelain=v1', cwd=checkout).stdout == '?? dirty\n'
            choose(mode, 'leave')
        print('PASS: malformed helpers return to the menu without Git mutation', flush=True)

    if not args.rebase_only:
        for interrupt in ('cancel-step', 'ctrl-c'):
            token = 'cancel-' + interrupt
            checkout = root / token
            run('git', 'worktree', 'add', '-b', token, checkout)
            retry = root / (token + '-retry')
            pids = root / (token + '-pids')
            program = ('import os,signal,subprocess,time; from pathlib import Path; '
                       'retry=Path(' + repr(str(retry)) + '); '
                       'exec("if retry.exists():\\n print(\\"RETRY PASSED\\", flush=True)\\n raise SystemExit(0)"); '
                       + ('signal.signal(signal.SIGINT, signal.SIG_IGN); ' if interrupt == 'cancel-step' else '') +
                       'child=subprocess.Popen(["/usr/bin/python3","-c","import time; time.sleep(600)"]); '
                       'Path(' + repr(str(pids)) + ').write_text(str(os.getpid())+" "+str(child.pid)); '
                       'print("PARTIAL CANCELLATION EVIDENCE", flush=True); time.sleep(600)')
            launch(token, checkout=str(checkout), gate=['/usr/bin/python3', '-c', program])
            q = menu(token)
            yinz('workflow', 'show', q['attributes']['run'])
            choose(token, 'run-tests', dismiss=False)
            pane = until('long foreground command', lambda: command_pane(token, 'tests'))
            pane_id = 'terminal_' + str(pane['id'])
            until('command and child ready', lambda: pids.exists())
            until('retained partial output', lambda: 'PARTIAL CANCELLATION EVIDENCE' in yinz('action', 'dump-screen', '--pane-id', pane_id))
            operation = next(bridge(method='lookup', work=token, key=r['runId'] + '-tests') for r in histories(token) if r['inputs'].get('phase') == 'tests')
            started = time.monotonic()
            if interrupt == 'cancel-step':
                bridge(method='cancel_step', work=token, operation=operation['key'])
                bridge(method='cancel_step', work=token, operation=operation['key'])
            else:
                yinz('action', 'write', '--pane-id', pane_id, '3')
                # Graceful interruption retains the command pane until dismissal.
                dismiss_command(token, 'tests')
            until('cancelled output retained', lambda: pending(token, 'test-output'))
            if interrupt == 'cancel-step': assert time.monotonic() - started >= 5
            stopped = bridge(method='observe', work=token, key=operation['key'])
            assert stopped['outcome'] == 'cancelled' and stopped['terminationVerified']
            def dead(pid):
                stat = Path('/proc/' + str(pid) + '/stat')
                return not stat.exists() or stat.read_text().split(') ')[1].startswith('Z')
            until('owned command and child terminated', lambda: all(dead(pid) for pid in map(int, pids.read_text().split())))
            choose(token, 'return-to-menu', 'test-output')
            assert check_state(token)['test']['outcome'] == 'cancelled'
            retry.touch()
            choose(token, 'run-tests')
            assert check_state(token)['test']['passed']
            choose(token, 'leave')
        print('PASS: real command and child cancellation, ignored INT escalation, Ctrl-C, retained output, and explicit successful retry', flush=True)

    if not args.cancellation_only:
        checkout = root / 'repeated-conflicts'
        run('git', 'worktree', 'add', '-b', 'repeated-conflicts', checkout)
        for content in ('branch one\n', 'branch two\n'):
            (checkout / 'file').write_text(content)
            run('git', 'add', 'file', cwd=checkout)
            run('git', 'commit', '-m', content.strip(), cwd=checkout)
        (main / 'file').write_text('target conflicts\n')
        run('git', 'add', 'file')
        run('git', 'commit', '-m', 'target conflicts')
        token = 'repeated-conflicts'
        launch(token, checkout=str(checkout))
        choose(token, 'commit-and-rebase')
        for _ in range(2):
            assert check_state(token)['evidence']['source']['rebasing']
            choose(token, 'resolve-with-agent')
            state = check_state(token)
            assert state['evidence']['source']['rebasing'] and not state['evidence']['source']['conflicts']
            choose(token, 'continue-rebase')
        assert not check_state(token)['evidence']['source']['rebasing']
        choose(token, 'leave')
        print('PASS: repeated conflicts require explicit resolver and continuation choices', flush=True)


def foreground_smoke():
    for automatic in (False, True):
        if automatic:
            pane = int(yinz('run', '--cwd', str(topic), '--', 'claude').strip().split('_')[-1])
            until('original agent ready', lambda: 'STUB READY' in yinz('action', 'dump-screen', '--pane-id', 'terminal_' + str(pane)))
            yinz('action', 'write-chars', '--pane-id', 'terminal_' + str(pane), 'exit\n')
            runs = until('automatic exit launch', lambda: swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(pane))['results'])
            token = runs[0]['inputs']['launch']
        else:
            token = 'manual-foreground'
            launch(token)
        q = until('foreground menu', lambda: pending(token, 'prepare'))
        if not automatic: yinz('workflow', 'show', q['attributes']['run'])
        view = until('decision view visible', lambda: next((p for p in json.loads(yinz('action', 'list-panes', '--json'))
                     if p.get('plugin_url') == ('yinz:agent-exit' if automatic else 'yinz:swamp-workflow') and not p['is_suppressed']), None))
        hold = root / 'hold-review'
        hold.touch()
        answer(q, 'review', 'foreground-review-' + q['attributes']['run'])
        helper = until('review helper foreground', lambda: (lambda p: p if p and not p['is_suppressed'] else None)(command_pane(token, 'helper')))
        assert (helper['pane_x'], helper['pane_y']) == (view['pane_x'], view['pane_y'])
        hold.unlink()
        reviewed = until('review returned to menu', lambda: pending(token, 'prepare'))
        assert reviewed['attributes']['run'] != q['attributes']['run']
        until('decision view restored', lambda: any(p['id'] == view['id'] and p['is_plugin'] and not p['is_suppressed'] for p in json.loads(yinz('action', 'list-panes', '--json'))))
        if automatic:
            answer(reviewed, 'fork', 'foreground-fork')
            until('fork picker', lambda: any(p.get('plugin_url') == 'yinz:hocus-new-agent' and not p['is_suppressed'] for p in json.loads(yinz('action', 'list-panes', '--json'))))
            os.write(ui_fd, b'\x1b')
            fresh = until('cancelled fork returned to menu', lambda: pending(token, 'prepare'))
            assert fresh['attributes']['run'] != reviewed['attributes']['run']
            reviewed = fresh
        answer(reviewed, 'leave', 'foreground-leave-' + reviewed['attributes']['run'])
        until('foreground flow finished', lambda: all(r['status'] == 'succeeded' for r in histories(token)))
    print('PASS: manual and exit-hook helpers occupy the decision location, restore it on exit, and cancelled Fork returns to the menu', flush=True)


def lock_smoke():
    # Another workflow's Git worker (here, this process) holds the target checkout lock
    # under a session that no longer exists, exactly as a stranded pane would.
    lock_path = main / '.git/yinz-workflow.lock'
    def hold(session_name):
        holder = lock_path.open('a+')
        fcntl.flock(holder, fcntl.LOCK_EX)
        holder.seek(0)
        holder.truncate()
        holder.write(json.dumps(dict(pid=os.getpid(), session=session_name, work='held-elsewhere',
                                     key='held-elsewhere-commit-and-rebase', since='2026-01-01T00:00:00.000Z')))
        holder.flush()
        return holder
    def release(holder):
        fcntl.flock(holder, fcntl.LOCK_UN)
        holder.close()
    def menu(token):
        return until(token + ': menu', lambda: pending(token, 'prepare'))
    for variant in ('release', 'cancel'):
        token = 'lock-' + variant
        checkout = root / token
        run('git', 'worktree', 'add', '-b', token, checkout)
        (checkout / 'dirty').write_text('uncommitted\n')
        launch(token, checkout=str(checkout))
        q = menu(token)
        holder = hold('vanished-session')
        try:
            answer(q, 'commit-and-rebase', 'answer-' + q['attributes']['run'])
            pane = until('waiting commit pane', lambda: commit_pane(token))
            pane_id = 'terminal_' + str(pane['id'])
            screen = lambda: yinz('action', 'dump-screen', '--pane-id', pane_id)
            until('lock holder named on screen', lambda: all(text in screen() for text in (
                'Waiting for checkout lock', 'vanished-session', 'no longer exists', 'Ctrl-C')))
            assert '$ git' not in screen()
            head = run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout
            if variant == 'release':
                release(holder)
                dismiss_commit(token)
                menu(token)
                assert run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout != head
                assert not run('git', 'status', '--porcelain=v1', cwd=checkout).stdout
            else:
                yinz('action', 'write', '--pane-id', pane_id, '3')
                q = until('cancelled commit returns its saved output', lambda: pending(token, 'commit-output'))
                release(holder)
                answer(q, 'return-to-menu', 'answer-' + q['attributes']['run'])
                menu(token)
                assert run('git', 'rev-parse', 'HEAD', cwd=checkout).stdout == head
                assert run('git', 'status', '--porcelain=v1', cwd=checkout).stdout == '?? dirty\n'
        finally:
            # A failed assertion must not hold the target checkout lock until exit.
            if not holder.closed: release(holder)
        q = menu(token)
        answer(q, 'leave', 'answer-' + q['attributes']['run'])
    print('PASS: a contended checkout lock names its stranded holder on screen; release proceeds and Ctrl-C cancels without mutation', flush=True)


run(binary, 'attach', '--create-background', session)
try:
    if args.exit_only or args.exit_menu_only:
        attach_ui()
        until('terminal client has rendered', lambda: len(ui_output) > 1000)
        foreground_smoke()
        raise SystemExit(0)
    if args.validation_loop_only or args.cancellation_only or args.rebase_only:
        attach_ui()
        until('terminal client has rendered', lambda: len(ui_output) > 1000)
        validation_loop_smoke()
        raise SystemExit(0)
    if args.lock_only:
        attach_ui()
        until('terminal client has rendered', lambda: len(ui_output) > 1000)
        lock_smoke()
        raise SystemExit(0)
    if args.question_latency_only:
        attach_ui()
        until('terminal client has rendered', lambda: len(ui_output) > 1000)
        samples=[]
        for index in range(8):
            pane=int(yinz('run', '--cwd', str(topic), '--', 'claude').strip().split('_')[-1])
            until('stub ready', lambda: 'STUB READY' in yinz('action', 'dump-screen'))
            offset=len(ui_output)
            yinz('action', 'write-chars', '--pane-id', str(pane), 'exit\n')
            until('actionable exit question', lambda: b'Show diff' in bytes(ui_output[offset:]), interval=0.005)
            elapsed=time.monotonic()-float((root / ('exit-' + str(pane))).read_text())
            token=swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(pane))['results'][0]['inputs']['launch']
            q=until('exit question data', lambda: pending(token, 'prepare'))
            offset=len(ui_output)
            submitted=time.monotonic()
            answer(q, 'review', 'timing-review-' + str(index))
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
        vm = bridge(method='agent_spawn', work='vm', key='worker', spec=dict(command='/usr/bin/python3', args=['stub_agent.py', '--exit'], cwd=str(topic), source_directory=str(topic), firecracker='workflow-vm', firecracker_image='yinz-dev', automatic_firecracker_vm=True))
        assert vm['supervisor'].startswith('remote:')
        bridge(method='agent_prompt', work='vm', key='edit', agent='worker', prompt='guest edit')
        until('original guest exits', lambda: bridge(method='observe', work='vm', key='worker').get('processExited'))
        edited = bridge(method='observe', work='vm', key='edit')
        assert edited.get('message'), yinz('action', 'dump-screen', '--pane-id', 'terminal_' + str(vm['pane']))
        guest_commit = edited['message']
        assert not (topic / 'guest.txt').exists()
        vm_name = vm['target']['Firecracker']['vm']
        (main / 'host-only').write_text('host revision\n')
        run('git', 'add', 'host-only')
        run('git', 'commit', '-m', 'advance host target')
        host_target = run('git', 'rev-parse', 'HEAD').stdout.strip()
        launch('original-vm', sourcePane=vm['pane'], firecracker=vm_name,
               agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit', '--inspect-review'])
        prepared = until('original VM synced without worker', lambda: pending('original-vm', 'prepare'))
        assert (topic / 'guest.txt').read_text() == 'guest\n'
        assert bridge(method='lookup', work='original-vm', key='worker') is None
        answer(prepared, 'commit-and-rebase', 'vm-rebase')
        dismiss_commit('original-vm')
        review_menu = until('host rebased review menu', lambda: pending('original-vm', 'prepare'))
        host_commit = run('git', 'rev-parse', 'HEAD', cwd=topic).stdout.strip()
        assert host_commit != guest_commit
        answer(review_menu, 'review', 'vm-review')
        confirmation = until('host reviewer confirmation', lambda: pending('original-vm', 'prepare'))
        records = swamp('data', 'query', 'modelType == "@hammz/yinz" && name == "review-current"', '--select', '{"attributes":attributes}')['results']
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
        def conflict_failed():
            runs = swamp('workflow', 'history', 'search', '--input', 'launch=conflict-vm')['results']
            # Recoverable step errors suspend the workflow; a pending question alone is not a sync failure.
            blocked = any(r['status'] in ('failed', 'suspended')
                          and r['workflowName'] == '@hammz/yinz-review-integration'
                          and r.get('failedStep') == 'sync'
                          and r.get('failureReason') == 'Original VM has unresolved changes or sync conflicts'
                          for r in runs)
            return runs if blocked and not any(r['status'] == 'running' for r in runs) else None
        failed_runs = until('sync conflict blocks preparation', conflict_failed)
        for item in failed_runs:
            if item['status'] in ('failed', 'suspended'):
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
        validation_loop_smoke()

        foreground_smoke()

        lock_smoke()

        yinz('workflow', 'run', '@hammz/yinz-question', '--launch', 'recovery', '--no-view', '--input', json.dumps(dict(kind='text', prompt='Instructions?', answers=[])))
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
    yinz('action', 'start-or-reload-plugin', 'yinz:swamp-bridge')
    observed = until('dispatcher restart', lambda: (lambda o: o if o['outcome'] == 'unknown' else None)(bridge(method='observe', work='restart', key='pending')))
    reconciled = bridge(method='reconcile', work='restart', key='reconcile', operation='worker')
    assert reconciled['supervisor'] == owned['supervisor'] and reconciled['pane'] == owned['pane']
    assert observed['outcome'] == 'unknown'
    assert bridge(method='agent_spawn', work='restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py'), '--wait'], cwd=str(topic), source_directory=str(topic)))['pane'] == owned['pane']
    # Crash only this fixture's server. Supervisors must keep the owned processes.
    survivor = bridge(method='agent_spawn', work='server-restart', key='worker', spec=dict(command='/usr/bin/python3', args=[str(main / 'extensions/tests/stub_agent.py')], cwd=str(topic), source_directory=str(topic)))
    bridge(method='agent_prompt', work='server-restart', key='before', agent='worker', prompt='Before restart')
    until('completion before server restart', lambda: bridge(method='observe', work='server-restart', key='before')['outcome'] == 'completed')
    server_pids = [pid for pid, argv in fixture_processes() if argv.split(' ')[:2] == [str(binary), '--server']]
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
    until('supervisor survives server crash', lambda: any(h['host_id'] == survivor['supervisor'] and not h['attached'] for h in json.loads(yinz('pty-host', 'list', '--json'))))
    run(binary, 'attach', '--create-background', session)
    until('restored session accepts commands', lambda: json.loads(yinz('action', 'list-tabs', '--json')))
    assert json.loads(run(binary, 'workflow', 'server', 'status').stdout)['pid']==swamp_pid
    assert not held_release.exists()
    assert swamp('workflow', 'history', 'get', held['run'])['status']=='running'
    assert bridge(method='observe', work='server-survival', key='held')['outcome']=='pending'
    held_release.touch()
    until('surviving Swamp execution completes', lambda: bridge(method='observe', work='server-survival', key='held')['outcome']=='completed')
    assert len(swamp('workflow', 'history', 'search', '--workflow', held_workflow['name'])['results'])==1
    print('PASS: Yinz server crash leaves active Swamp execution alive; reattachment completes without duplication', flush=True)
    for resource in (survivor, owned):
        adopted = yinz('pty-host', 'adopt', resource['supervisor'], '--session', session)
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
    assert not [p for p in json.loads(yinz('action', 'list-panes', '--json')) if p.get('plugin_url') == 'yinz:agent-exit']
    print('PASS: server crash/adoption retains ownership, resumes verified work, and delivers managed process exit', flush=True)
finally:
    clean = teardown()
    transcript.close()
    print('Retained evidence:', root, flush=True)
    launches=[json.loads(line) for line in (root / 'swamp-processes.jsonl').read_text().splitlines()]
    operations=[entry for entry in launches if entry['parent'] in ('yinz', 'swamp') and entry['command'][0] in ('workflow', 'model', 'data')]
    assert not operations, operations
    print('PASS: zero per-operation Swamp CLI launches from Yinz or Swamp', flush=True)
    assert clean, 'the fixture must leave no processes behind'
