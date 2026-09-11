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
parser.add_argument('--source-diff-only', action='store_true')
parser.add_argument('--timing-only', action='store_true', help='Three diagnostic diff-and-return runs on one warm disposable server')
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
# Keep the tested executable stable if another checkout rebuilds during the run.
os.link(Path(args.binary).resolve(), binary)
session = 'workflow-' + root.name.rsplit('-', 1)[-1]
env = os.environ.copy()
if args.timing_only:
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
# Serve checks the existing Swamp login; keep that host credential store available
# while isolating Zellij's configuration. Never copy credentials into transcripts.
if swamp_config.exists():
    (root / 'config/swamp').symlink_to(swamp_config, target_is_directory=True)
env['SWAMP_SMOKE_ZELLIJ'] = str(binary)
if args.source_diff_only:
    env.update(GIT_PAGER='false', PAGER='false', LESS='-F -E')
real_swamp = shutil.which('swamp')
assert real_swamp, 'Swamp is required'
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
# The duplicate action intentionally drops old arguments. Keep its real agent-kind
# command inside this fixture so it cannot contact a model provider.
fixture_claude = fixture_bin / 'claude'
fixture_claude.write_text('#!/usr/bin/python3\nimport os\nos.execv("/usr/bin/python3", ["/usr/bin/python3", ' + repr(str(main / 'extensions/tests/stub_agent.py')) + ', "--exit"])\n')
fixture_claude.chmod(0o755)

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
(main / '.zellij').mkdir()
shutil.copy(source / '.zellij/.swamp-workflows.kdl', main / '.zellij/.swamp-workflows.kdl')
(main / '.git/info/exclude').write_text('.swamp*\n.gitignore\nextensions/\nworkflows/\nmodels/\n.zellij/\nAGENTS.md\nCLAUDE.md\n')
for name in ('agent-work', 'review-integration', 'question', 'agent-exit'):
    swamp('workflow', 'validate', '@hammz/zellij-' + name)
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
(root / 'config/zellij/config.kdl').write_text('show_release_notes false\nshow_startup_tips false\nfirecracker_provider ' + json.dumps(str(source / 'target/debug/zellij-firecracker')) + '\n')
observer = root / 'snapshot.lua'
observer.write_text('''local z=require('zellij')
return {
  load=function() z.request_permission{'message_and_launch_other_plugins','read_cli_pipes'} end,
  pipe=function(message)
    if message.name~='snapshot' then return end
    local cli=message.source.Cli
    z.block_cli_pipe_input(cli)
    z.task(function()
      local snapshot=z.async.pipe('agent_snapshot')
      z.message_to_plugin{plugin_url='zellij:hocus-agentd',message_name='agent_subscribe'}
      z.cli_pipe_output(cli,z.await(snapshot).payload)
      z.unblock_cli_pipe_input(cli)
    end)
  end,
}
''')
(root / 'cache/zellij').mkdir(exist_ok=True)
(root / 'cache/zellij/permissions.kdl').write_text(json.dumps(str(observer)) + ' {\n MessageAndLaunchOtherPlugins\n ReadCliPipes\n}\n')
if args.firecracker_only:
    provider_config = source / 'target/firecracker-test/config/zellij/firecracker.json'
    shutil.copy(provider_config, root / 'config/zellij/firecracker.json')
    # Config names the existing immutable image; VM state stays in this fixture.
cold_start_ms = None
if args.timing_only:
    started = time.monotonic()
    run(binary, 'workflow', 'server', 'restart')
    cold_start_ms = (time.monotonic() - started) * 1000
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
        def choose(token, question, value):
            print('CHOICE:', token, question, value, flush=True)
            q = until(token + ': ' + question, lambda: pending(token, question))
            answer(q, value, 'answer-' + q['attributes']['run'])
            return q

        def prepare(token):
            choose(token, 'exit', 'begin-review')
            choose(token, 'prepare', 'commit-and-rebase')
            return until(token + ': review menu', lambda: pending(token, 'review'))

        def agents():
            return [p for p in json.loads(zellij('action', 'list-panes', '--json')) if p.get('agent')]

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
                        assert record['layer'] == 'typescript' and record['elapsedMs'] >= 0
                measurements.append(dict(repetition=repetition + 1, elapsedMs=elapsed,
                    untilDismissMs=(dismissed - started) * 1000, previousRun=previous['attributes']['run'], nextRun=fresh['attributes']['run']))
                for path in Path(env['TMPDIR']).rglob('zellij.log*'):
                    shutil.copy(path, root / ('repetition-' + str(repetition + 1) + '-' + path.name))
                print('TIMING:', measurements[-1], flush=True)
            (root / 'timing.json').write_text(json.dumps(dict(coldStartMs=cold_start_ms, repetitions=measurements), indent=2))
            choose('timing', 'prepare', 'leave')
            print('PASS: three warm diff-and-return repetitions; cold startup recorded separately', flush=True)
            raise SystemExit(0)

        if args.source_diff_only:
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
            choose(token, 'exit', 'fork')
            until('hidden source resolved by Fork', lambda: 'forking claude' in zellij('action', 'dump-screen'))
            os.write(ui_fd, b'\x1b')
            until('Fork cancellation returns to exit question', lambda: pending(token, 'exit'))
            before_ids = {p['id'] for p in agents()}
            choose(token, 'exit', 'duplicate')
            duplicated = until('Duplicate launches hidden source kind', lambda: next((p for p in agents() if p['id'] not in before_ids), None))
            assert duplicated['agent']['kind'] == 'claude'
            assert duplicated['agent']['cwd'] == str(topic)
            print('PASS: automatic tiled exit suppresses source; snapshot excludes it; Fork resolves it; cancelled Fork returns; Duplicate preserves kind/cwd', flush=True)
            until('duplicate has enabled raw input', lambda: 'STUB READY' in zellij('action', 'dump-screen'))
            zellij('action', 'write-chars', '--pane-id', str(duplicated['id']), 'exit\n')
            until('duplicate exit hook suppresses source', lambda: next((p for p in agents() if p['id'] == duplicated['id'] and p['exited'] and p['is_suppressed']), None))
            automatic = until('duplicate exit run', lambda: swamp('workflow', 'history', 'search', '--filter', 'inputs.sourcePane == ' + str(duplicated['id']))['results'])
            token = automatic[0]['inputs']['launch']
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
            assert initial['attributes']['answers'] == ['begin-review', 'leave']
            assert initial['attributes']['prompt'] == 'Choose Begin review or Leave.'
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
            assert not agents(), 'Preparation and merge confirmation must not start agents'
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
        live_gate = ("import sys,time; from pathlib import Path; "
                     "print('LIVE BEFORE RELEASE 😀', flush=True); "
                     "print('LIVE STDERR', file=sys.stderr, flush=True); "
                     "release=Path(" + repr(str(release)) + "); "
                     "exec('while not release.exists(): time.sleep(0.05)'); "
                     "sys.stdout.write(('𝄞' * 63 + '\\n') * 9600); sys.exit(1)")
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
            assert {'operation', 'test'} <= seen, seen
            if nested:
                assert 'checks' in seen, seen
            ids = [h['id'] for h in [snapshot['history']] + snapshot['children']]
            requests = swamp('data', 'query', 'specName == "operation" && tags.workflowRunId in ' + json.dumps(ids), '--select', 'attributes.request')['results']
            assert all(json.loads(r)['method'] != 'agent_spawn' for r in requests), 'Failed tests must not automatically start a repair agent'


        review_question = prepare('failed-checks')
        # Earlier scenarios may leave completed views open. Start this detach check with one view.
        for pane in json.loads(zellij('action', 'list-panes', '--json')):
            if pane.get('plugin_url') == 'zellij:swamp-workflow':
                zellij('action', 'close-pane', '--pane-id', 'plugin_' + str(pane['id']))
        frame_start = len(ui_output)
        zellij('workflow', 'show', review_question['attributes']['run'])
        until('review buttons rendered', lambda: b'Run tests' in bytes(ui_output[frame_start:]))
        # Select the actual button; output must arrive while the gate is held open.
        index = review_question['attributes']['answers'].index('run-tests')
        os.write(ui_fd, b'\t' * index)
        clicked = time.monotonic()
        frame_start = len(ui_output)
        os.write(ui_fd, b'\r')
        until('immediate submission feedback', lambda: b'Submitting answer' in bytes(ui_output[frame_start:]), interval=0.005)
        feedback = time.monotonic() - clicked
        until('answer recorded', lambda: b'Answer recorded' in bytes(ui_output[frame_start:]), interval=0.005)
        submission = time.monotonic() - clicked
        if not args.latency_baseline:
            # Output from another stream may replace this frame before a CLI snapshot arrives.
            until('live output before command exit', lambda: b'LIVE BEFORE RELEASE' in bytes(ui_output))
        assert not release.exists()
        os.write(ui_fd, b'\x1b')
        until('view detached', lambda: not any(p.get('plugin_url') == 'zellij:swamp-workflow'
              for p in json.loads(zellij('action', 'list-panes', '--json'))))
        frame_start = len(ui_output)
        zellij('workflow', 'show', review_question['attributes']['run'])
        until('view reattached', lambda: b'Cancel run' in bytes(ui_output[frame_start:]))
        assert not release.exists(), 'Detach must not release or stop execution'
        released = time.monotonic()
        release.touch()
        until('next question rendered', lambda: b'Start fix agent' in bytes(ui_output[frame_start:]), interval=0.005)
        timings = dict(click_to_feedback_seconds=feedback, submission_seconds=submission,
                       release_to_next_question_seconds=time.monotonic()-released,
                       child_decisions=len(histories('failed-checks')))
        (root / 'latency.json').write_text(json.dumps(timings, indent=2))
        print('LATENCY:', timings, flush=True)
        if args.latency_only:
            raise SystemExit(0)
        os.write(ui_fd, b'\x1b')
        failed = until('failed-test choices', lambda: pending('failed-checks', 'failed-tests'))
        assert failed['attributes']['answers'] == ['start-fix-agent', 'return-to-menu', 'leave']
        check_output_previews(failed)
        choose('failed-checks', 'failed-tests', 'return-to-menu')
        choose('failed-checks', 'review', 'merge')
        informed = until('failed-check merge confirmation', lambda: pending('failed-checks', 'merge'))
        assert 'Tests: FAILED' in informed['attributes']['prompt']
        check_output_previews(informed, nested=True)
        choose('failed-checks', 'merge', 'merge')
        choose('failed-checks', 'cleanup', 'keep')
        report = swamp('report', 'get', '@hammz/workflow', '--workflow', '@hammz/zellij-review-integration')['json']
        assert not report.get('error'), report
        print('PASS: verbose failed-test previews and merge confirmation; full logs retained; no automatic repair', flush=True)
        if args.test_output_only:
            raise SystemExit(0)

        launch('fix', gate=['/bin/sh', '-c', 'printf failure-evidence; exit 1'])
        prepare('fix')
        choose('fix', 'review', 'run-tests')
        choose('fix', 'failed-tests', 'start-fix-agent')
        returned = until('fix exit returns to initial menu', lambda: pending('fix', 'exit'))
        choose('fix', 'exit', 'leave')
        assert topic.exists()
        assert 'failure-evidence' in (topic / 'received-failure-output').read_text()
        (topic / 'received-failure-output').unlink()
        print('PASS: approved fix receives failure output and exits to a fresh menu; Leave preserves checkout', flush=True)

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
        assert first_menu['attributes']['answers'] == ['restart', 'duplicate', 'fork', 'begin-review', 'leave']
        assert first_menu['attributes']['prompt'] == 'Agent exited. Choose Restart, Duplicate, Fork, Begin review, or Leave.'
        until('existing fork picker', lambda: any(p.get('plugin_url') == 'zellij:hocus-new-agent' and not p['is_suppressed'] for p in json.loads(zellij('action', 'list-panes', '--json'))))
        zellij('action', 'start-or-reload-plugin', 'zellij:swamp-bridge')
        until('picker dispatch becomes uncertain after reload', lambda: bridge(method='observe', work='fork-cancel', key=first_menu['attributes']['run'] + '-pane-choice')['outcome'] == 'unknown')
        os.write(ui_fd, b'\x1b')
        cancelled = until('cancelled fork returns to fresh exit menu', lambda: pending('fork-cancel', 'exit'))
        assert cancelled['attributes']['run'] != first_menu['attributes']['run']
        choose('fork-cancel', 'exit', 'leave')
        launch('duplicate-choice', sourcePane=original['id'], kind='claude')
        before_ids = {p['id'] for p in agents()}
        choose('duplicate-choice', 'exit', 'duplicate')
        duplicated = until('duplicate launches original kind', lambda: next((p for p in agents() if p['id'] not in before_ids), None))
        assert duplicated['agent']['kind'] == 'claude'
        assert duplicated.get('pane_cwd', duplicated['agent'].get('cwd')) == str(topic)
        launch('restart-choice', sourcePane=original['id'], kind='claude')
        choose('restart-choice', 'exit', 'restart')
        until('restart uses original pane', lambda: any(p['id'] == original['id'] and not p['exited'] for p in agents()))
        print('PASS: fork cancellation returns to a fresh menu; duplicate preserves kind/cwd; restart reuses original pane', flush=True)


        launch('unresolved-review', agentArgs=[str(main / 'extensions/tests/stub_agent.py'), '--exit', '--findings'])
        prepare('unresolved-review')
        choose('unresolved-review', 'review', 'start-code-review')
        unresolved = until('unresolved review confirmation', lambda: pending('unresolved-review', 'merge'))
        assert 'Review: FAILED / unresolved findings' in unresolved['attributes']['prompt']
        choose('unresolved-review', 'merge', 'merge')
        choose('unresolved-review', 'cleanup', 'keep')
        print('PASS: informed merge accepts unresolved review findings', flush=True)

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
        launch('conflict-resolve')
        choose('conflict-resolve', 'exit', 'begin-review')
        first_conflict = choose('conflict-resolve', 'conflict', 'resolve-with-agent')
        next_conflict = choose('conflict-resolve', 'conflict', 'resolve-with-agent')
        assert first_conflict['attributes']['run'] != next_conflict['attributes']['run']
        choose('conflict-resolve', 'review', 'leave')
        assert not run('git', 'diff', '--name-only', '--diff-filter=U', cwd=topic).stdout.strip()
        assert run('git', 'symbolic-ref', '--short', 'HEAD', cwd=topic).stdout.strip() == 'topic'
        print('PASS: paused rebase requires an explicit resolver; workflow alone continues the resolved index', flush=True)

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
