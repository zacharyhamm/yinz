// Durable delivery adapter. Swamp owns the DAG, evidence, questions and decisions.
import z, {definePlugin, type Json, type DataRecord, type Integer, type PaneId,
  type PaneTarget, type AgentSpawnOptions, type AgentEvent, type MessageToPlugin} from 'zellij:plugin';

type Owner = {host_id: string; session: string; incarnation: string; work: string; agent: string};
type Identity = Pick<Owner, 'host_id' | 'session' | 'incarnation'>;
type Request = {
  method: string; work: string; key: string; agent: string; operation: string;
  directory: string; run: string; launch: string; workflow: string;
  prompt: string; prompt_sha256?: string; waitForExit?: boolean; includeOutput?: boolean;
  action: string; spec: AgentSpawnOptions & {source_directory?: string; automatic_firecracker_vm?: boolean}; argv: string[]; cwd: string;
  inputs?: Record<string, Json>;
};
type Attachment = {run: string; work: string; workflow: string; root: string};
type Delivery = {
  request: string; key: string; work: string; outcome: string; incarnation: string; dispatcher: string;
  owner?: Owner; pane?: number; supervisor?: string; process?: number; transportOwner?: string | null;
  outstanding?: string; ready?: boolean; processExited?: boolean; accepted?: boolean; dispatched?: boolean;
  agentSession?: string | null; turn?: string; message?: string | null; hook?: string;
  target?: PaneTarget; remoteHost?: string; remotePaneId?: string; branch?: string | null;
  checkout?: string | null; log?: string; error?: string; output?: string; outputTruncated?: boolean;
  logError?: string; exitCode?: number | null; workflow?: string; launch?: string;
  view?: number; entries?: Record<string, Attachment>; choice?: string;
  vm?: string | null; clean?: boolean;
};
type Supervisor = {terminal_id: number; host_id: string; child_pid?: number; attached: boolean;
  session_name: string; child_exit_status?: number | null; owner?: string | null; remote_host?: string; pane_token?: string};
const D = z.data;
let identity: Identity;
const placements = new Map<string, {view: number; queue: string[]; current?: number; detached?: boolean}>();
function assert(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
// Lua's encoder sorts object keys recursively. Preserve exact request/owner comparisons on reload.
function encode(value: unknown): string {
  return JSON.stringify(value, (_key, item) => item && typeof item === 'object' && !Array.isArray(item)
    ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
}
// Lua decoded null object fields as absent and encoded empty tables as arrays.
// Normalize both sides when comparing a persisted request from either runtime.
function sameRequest(saved: string, request: unknown): boolean {
  const normalize = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(normalize);
    if (value && typeof value === 'object') {
      const entries = Object.entries(value).filter(([, v]) => v != null).map(([k, v]) => [k, normalize(v)]);
      return entries.length ? Object.fromEntries(entries) : [];
    }
    return value;
  };
  return encode(normalize(JSON.parse(saved))) === encode(normalize(request));
}
function key(value: string): string {
  assert(typeof value === 'string' && /^[A-Za-z0-9][A-Za-z0-9_.:\-]*$/.test(value) && value.length <= 200, 'Invalid correlation key');
  return value;
}
function scope(work: string, operation: string): string { return encode([identity.host_id, identity.session, key(work), key(operation)]); }
function read<T = Delivery>(uid: string): [T | undefined, DataRecord | undefined] {
  const row = D.fetch({entity: 'delivery', filter: D.eq('uid', uid), limit: 1}).records[0];
  return [row ? JSON.parse(String(row.value)) as T : undefined, row];
}
function put(uid: string, value: unknown, row?: DataRecord) {
  D.apply([D.put('delivery', {value: encode(value)}, {uid, expectRev: row?.rev ?? 0})]);
}
function result(uid: string): Delivery { const [value] = read(uid); assert(value, 'No operation with that identity'); return value; }
function saveEvent(uid: string, outcome: string, fields: Partial<Delivery> = {}) {
  const [value, row] = read(uid);
  if (!value || !['pending', 'unknown'].includes(value.outcome)) return;
  put(uid, {...value, outcome, ...fields}, row);
}
const terminal = (id: number): PaneId => ({Terminal: id});
const plugin = (id: number): PaneId => ({Plugin: id});
function closeView(work: string) {
  const [placement] = read(scope(work, 'view'));
  if (!placement || placement.incarnation !== identity.incarnation || placement.view == null) return;
  if (z.command('GetPaneInfo', plugin(placement.view))?.plugin_url === 'zellij:agent-exit') z.command('ClosePluginPane', placement.view);
}
function active(work: string): boolean {
  const works = new Set([work, ...Object.keys(read(scope(work, 'runs'))[0]?.entries ?? {})]);
  let cursor: Integer | null = null;
  do {
    const {records, raw} = D.fetch({entity: 'delivery', afterId: cursor});
    for (const row of records) {
      const id: string[] = JSON.parse(String(row.uid));
      if (id[0] === identity.host_id && id[1] === identity.session && works.has(id[2])) {
        const operation: Delivery = JSON.parse(String(row.value));
        if (['pending', 'unknown'].includes(operation.outcome) && ['command', 'agent_prompt'].includes(JSON.parse(operation.request).method)) return true;
      }
    }
    cursor = raw.next_cursor;
  } while (cursor != null);
  return false;
}
function launchFailure(work: string, launch: Delivery, logs: string, directory?: string) {
  return {launch: work, root: launch.checkout ?? directory, history: {id: work, workflowName: launch.workflow ?? 'Workflow launch', status: 'failed'}, children: [], active: active(work), logs};
}
function present(work: string) {
  const placement = placements.get(work);
  if (!placement || placement.detached) return;
  const view = plugin(placement.view);
  const info = z.command('GetPaneInfo', view);
  const current = placement.current == null ? view : terminal(placement.current);
  const currentInfo = z.command('GetPaneInfo', current);
  if (info?.plugin_url !== 'zellij:agent-exit' || !currentInfo || currentInfo.is_suppressed) { placement.detached = true; return; }
  let next: number | undefined;
  while (placement.queue.length) {
    const [operation] = read(placement.queue[0]);
    if (operation?.outcome === 'pending') { next = operation.pane; break; }
    placement.queue.shift();
  }
  if (next === placement.current) return;
  const target = next == null ? view : terminal(next);
  if (!z.command('GetPaneInfo', target)?.is_suppressed) { placement.detached = true; return; }
  z.command('ReplacePaneWithExistingPane', [current, target, true]);
  placement.current = next;
}
async function command(argv: string[], cwd = '/'): Promise<string> {
  if (argv[0] === 'swamp' && argv[1] === 'workflow' && argv[2] === 'history' && argv[3] !== 'logs') {
    argv = ['bash', '-o', 'pipefail', '-c', '"$@" | jq -c \'del(.inputs.state.test.output, .results[]?.inputs.state.test.output)\'', 'workflow-history', ...argv];
  }
  const r = await z.runCommand({command: argv, cwd, context: {swamp_owner: encode(identity)}, maxBytesReturned: 1048576});
  const stdout = z.decodeUtf8(r.stdout), stderr = z.decodeUtf8(r.stderr);
  assert(r.exitCode === 0, `Host command failed: ${encode(argv)}\nDirectory: ${cwd}\nExit status: ${r.exitCode}\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  return stdout;
}
async function supervisor(pane: number, saved?: string): Promise<Supervisor> {
  for (const row of z.workflowRemotePanes?.() ?? []) {
    if (saved ? row.host_id === saved : row.terminal_id === pane) {
      const info = z.command('GetPaneInfo', terminal(row.terminal_id));
      // Remote identities have no child PID; their owner token guards the process instead.
      return {...row, session_name: identity.session, attached: true,
        child_exit_status: info?.exited ? info.exit_status : undefined};
    }
  }
  const rows: Supervisor[] = JSON.parse(await command([z.serverBinary || 'zellij', 'pty-host', 'list', '--json']));
  const found = rows.find(row => row.session_name === identity.session && (saved ? row.host_id === saved : row.terminal_id === pane));
  assert(found, 'No supervisor identity for pane; reconcile before sending input');
  return found;
}
async function owned(args: {work: string; agent: string}, running = false): Promise<Delivery & {pane: number}> {
  let agent = result(scope(args.work, args.agent));
  assert(agent.owner?.host_id === identity.host_id && agent.owner.session === identity.session, 'Wrong host or session');
  assert(agent.pane != null && agent.supervisor, 'Agent dispatch is unknown; reconcile it first');
  const current = await supervisor(agent.pane, agent.supervisor);
  assert(current.host_id === agent.supervisor, 'Pane belongs to a different supervisor');
  assert(current.child_pid === agent.process, 'Agent process changed');
  assert(current.attached, 'Supervisor is detached; adopt and reconcile before sending input');
  if (current.owner) assert(current.owner === (agent.transportOwner ?? encode(agent.owner)), 'Remote ownership changed');
  if (current.terminal_id !== agent.pane) {
    const uid = scope(args.work, args.agent), [fresh, row] = read(uid);
    assert(fresh, 'Agent disappeared');
    fresh.pane = current.terminal_id; put(uid, fresh, row); agent = fresh;
  }
  if (running) assert(!agent.processExited && current.child_exit_status == null, 'Agent process exited');
  return {...agent, pane: current.terminal_id};
}
function dispatchPrompt(agent: Delivery) {
  if (!agent.ready || !agent.outstanding) return;
  const [operation, row] = read(agent.outstanding);
  if (!operation || operation.outcome !== 'pending' || operation.dispatched) return;
  assert(agent.pane != null, 'Missing prompt pane');
  operation.dispatched = true;
  put(agent.outstanding, operation, row); // A crash after this point is an uncertain send.
  z.agent.send(terminal(agent.pane) as {Terminal: number}, JSON.parse(operation.request).prompt);
}
function claim(args: Partial<Request>): [string, Delivery | undefined] {
  const uid = scope(args.work!, args.key!), [previous] = read(uid), request = encode(args);
  if (previous) { assert(sameRequest(previous.request, args), 'Conflicting arguments for existing operation'); return [uid, previous]; }
  const value = {request, key: args.key, work: args.work, outcome: 'unknown', incarnation: identity.incarnation, dispatcher: z.instanceId};
  try { put(uid, value); } catch (error) {
    if (!D.isConflict(error)) throw error;
    const previous = result(uid);
    assert(sameRequest(previous.request, args), 'Conflicting concurrent operation');
    return [uid, previous];
  }
  return [uid, undefined];
}
async function discover(work: string, root: string) {
  const launch = result(scope(work, 'launch'));
  assert(launch.checkout === root, 'Launch belongs to a different control checkout');
  const history = JSON.parse(await command(['swamp', 'workflow', 'history', 'search', '--input', `launch=${work}`, '--json'], root));
  for (const run of history.results ?? []) if (run.inputs.session === identity.session) {
    await handle({method: 'workflow_attach', work, key: 'attachment', run: run.runId, workflow: run.workflowName, directory: root});
  }
  return read(scope(work, 'runs'))[0];
}
async function handle(input: Partial<Request>): Promise<unknown> {
  assert(input && typeof input === 'object' && !Array.isArray(input), 'Expected a typed request');
  // Each method validates the fields it consumes; correlation keys are checked by scope().
  const args = input as Request;
  if (args.method === 'identity') return identity;
  if (args.method === 'workflow_cancel') {
    const snapshot = await handle({method: 'workflow_show', directory: args.directory, run: args.run}) as {root: string; history: History; children: History[]};
    const operations = [];
    for (const history of [snapshot.history, ...snapshot.children]) if (['running', 'suspended'].includes(history.status)) {
      operations.push(await handle({method: 'command', work: history.inputs.launch, key: `cancel-${history.id}`, cwd: snapshot.root,
        argv: ['swamp', 'workflow', 'cancel', history.workflowName, '--run', history.id, '--json']}));
    }
    return {operations};
  }
  if (args.method === 'workflow_show') {
    const failed = args.launch ? read(scope(args.launch, 'launch'))[0] : undefined;
    if (failed?.outcome === 'failed' && failed.error) return launchFailure(args.launch, failed, failed.error, args.directory);
    assert(args.directory, 'Missing control checkout');
    const config = z.projectConfig.read(args.directory);
    assert(!config.error, config.error ?? 'Invalid project config');
    let attachment: Attachment | undefined, work: string | undefined;
    if (args.run) {
      [attachment] = read<Attachment>(scope(args.run, 'attachment'));
      if (attachment) work = attachment.work;
      else {
        const history: History = JSON.parse(await command(['swamp', 'workflow', 'history', 'get', args.run, '--json'], config.root));
        assert(history.inputs.session === identity.session, 'Run belongs to a different session');
        work = history.inputs.launch;
      }
    } else if (args.launch) work = args.launch;
    const runs = work ? await discover(work, config.root) : undefined;
    if (args.run) [attachment] = read<Attachment>(scope(args.run, 'attachment'));
    else if (runs) for (const [id, entry] of Object.entries(runs.entries ?? {})) {
      if (entry.workflow === '@hammz/zellij-agent-exit' || !attachment) attachment = {...entry, run: id};
    }
    if (!attachment && work) {
      let runner = read(scope(work, 'swamp-process'))[0];
      if (runner && ['completed', 'failed', 'process-exit'].includes(runner.outcome)) {
        runner = await handle({method: 'observe', work, key: 'swamp-process'}) as Delivery;
        let logs = `Workflow runner exited without reporting a run. Exit status: ${runner.exitCode}`;
        if (runner.output) logs += `\nRetained command scrollback (may omit older output):\n${runner.output}`;
        if (runner.logError) logs += `\nLog unavailable: ${runner.logError}`;
        return launchFailure(work, result(scope(work, 'launch')), logs);
      }
    }
    assert(attachment, 'Launch has no reported run yet; reconcile by launch token without repeating it');
    const run = attachment.run;
    const history = JSON.parse(await command(['swamp', 'workflow', 'history', 'get', run, '--json'], config.root));
    const ids = Object.keys(result(scope(attachment.work, 'runs')).entries ?? {});
    const questions = JSON.parse(await command(['swamp', 'data', 'query', `specName == "question" && attributes.run in ${encode(ids)}`,
      '--select', '{"model": modelName, "version": version, "attributes": attributes}', '--json'], config.root));
    const outputs = JSON.parse(await command(['swamp', 'data', 'query', `has(tags.workflowRunId) && tags.workflowRunId in ${encode(ids)}`,
      '--select', outputProjection, '--json'], config.root));
    const children = [];
    for (const id of ids) if (id !== run) children.push(JSON.parse(await command(['swamp', 'workflow', 'history', 'get', id, '--json'], config.root)));
    return {run, launch: attachment.work, root: config.root, history, children, active: active(attachment.work), outputs,
      logs: await command(['swamp', 'workflow', 'history', 'logs', run, '--tail', '100', '--json'], config.root), questions};
  }
  key(args.work);
  if (args.method === 'lookup') return read(scope(args.work, args.key))[0] ?? null;
  if (args.method === 'observe') {
    const uid = scope(args.work, args.key), [operation, row] = read(uid);
    assert(operation, 'No operation with that identity');
    if (operation.outcome === 'pending' && operation.dispatcher !== z.instanceId) { operation.outcome = 'unknown'; put(uid, operation, row); }
    if (args.includeOutput !== false && operation.log && operation.pane != null && operation.supervisor) {
      try {
        const host = await supervisor(operation.pane, operation.supervisor);
        assert(host.host_id === operation.supervisor, 'Log belongs to a different supervisor');
        assert(host.child_pid === operation.process, 'Log process changed');
        assert(host.attached, 'Log supervisor is detached');
        operation.pane = host.terminal_id; operation.log = `terminal_${host.terminal_id}`;
        const contents = z.command('GetPaneScrollback', {pane_id: terminal(host.terminal_id), get_full_scrollback: true});
        operation.output = [...contents.lines_above_viewport, ...contents.viewport, ...contents.lines_below_viewport].join('\n');
        operation.outputTruncated = true;
      } catch (error) { operation.logError = String(error); }
    }
    return operation;
  }
  if (args.method === 'agent_inspect') return owned(args);
  if (args.method === 'workflow_attach') {
    const entry: Attachment = {run: args.run, work: args.work, workflow: args.workflow, root: args.directory};
    const uid = scope(args.run, 'attachment'), [existing, row] = read(uid);
    assert(!existing || encode(existing) === encode(entry), 'Run belongs to a different launch');
    if (!existing) put(uid, entry, row);
    const runsUid = scope(args.work, 'runs');
    for (let attempt = 1; attempt <= 8; attempt++) {
      const [saved, runsRow] = read<{entries: Record<string, Attachment>}>(runsUid);
      const runs = saved ?? {entries: {}};
      if (runs.entries[args.run]) break;
      runs.entries[args.run] = entry;
      try { put(runsUid, runs, runsRow); break; } catch (error) { if (!D.isConflict(error) || attempt === 8) throw error; }
    }
    return {work: args.work, key: args.key, request: encode(args), incarnation: identity.incarnation, outcome: 'completed'};
  }
  if (args.method === 'reconcile') {
    const uid = scope(args.work, args.operation), [operation, row] = read(uid);
    assert(operation, 'Operation is absent');
    if (JSON.parse(operation.request).method === 'workflow_run' && operation.checkout) await discover(args.work, operation.checkout);
    if (operation.pane != null && operation.supervisor) {
      const host = await supervisor(operation.pane, operation.supervisor);
      assert(host.host_id === operation.supervisor, 'Supervisor mismatch; attachment remains unknown');
      assert(host.child_pid === operation.process, 'Supervisor process changed');
      operation.pane = host.terminal_id;
      if (operation.log) operation.log = `terminal_${host.terminal_id}`;
      if (host.child_exit_status != null) {
        operation.outcome = JSON.parse(operation.request).method === 'command' ? (host.child_exit_status === 0 ? 'completed' : 'failed') : 'process-exit';
        operation.exitCode = host.child_exit_status;
      } else if (operation.outcome === 'pending' && operation.incarnation !== identity.incarnation) operation.outcome = 'unknown';
      put(uid, operation, row);
    }
    return operation;
  }
  if (args.method === 'agent_prompt') assert(typeof args.prompt === 'string' && z.encodeUtf8(args.prompt).length > 0 && z.encodeUtf8(args.prompt).length <= 262144,
    'Prompt must contain 1–262144 UTF-8 bytes (256 KiB)');
  let uid: string | undefined, value: Delivery, row: DataRecord | undefined;
  if (args.method === 'vm_status') value = {work: args.work, key: args.key, request: encode(args), incarnation: identity.incarnation} as Delivery;
  else {
    const [claimed, previous] = claim(args); uid = claimed;
    if (previous) return previous;
    value = result(uid); [, row] = read(uid);
  }
  if (args.method === 'workflow_run') {
    assert(args.directory, 'Missing project directory');
    const config = z.projectConfig.read(args.directory);
    assert(!config.error, config.error ?? 'Invalid project config');
    const name = args.workflow ?? config.config.workflow;
    assert(name, 'No exit workflow configured. Use Hocus /project-setup.');
    await command(['swamp', 'workflow', 'validate', name, '--json'], config.root);
    const inputs = args.inputs ?? {};
    inputs.launch = args.work; inputs.session = identity.session; inputs.binary = z.serverBinary || 'zellij';
    inputs.checkout ??= (await command(['git', 'rev-parse', '--show-toplevel'], args.directory)).replace(/\n$/, '');
    if (typeof inputs.sourcePane === 'number' && inputs.sourcePane >= 0) {
      const pane = z.command('GetPaneInfo', terminal(inputs.sourcePane));
      assert(pane?.exited, 'Source pane must be exited');
      const host = await supervisor(inputs.sourcePane);
      put(scope(args.work, 'source'), {pane: inputs.sourcePane, supervisor: host.host_id, process: host.child_pid, target: pane.target, transportOwner: host.owner,
        owner: {...identity, work: args.work, agent: 'source'}});
      inputs.kind = pane.agent?.kind ?? inputs.kind ?? '';
      const vm = typeof pane.target === 'object' && 'Firecracker' in pane.target ? pane.target.Firecracker : undefined;
      if (vm) { inputs.firecracker = vm.vm; inputs.firecrackerImage = vm.image ?? ''; }
      else assert(!inputs.firecracker, 'Local source cannot select a different VM');
    }
    assert(!inputs.firecracker || (typeof inputs.sourcePane === 'number' && inputs.sourcePane >= 0), 'VM work requires its original pane identity');
    inputs.target ??= config.root; inputs.gate ??= config.config.ci ?? ['cargo', 'x', 'ci', 'check'];
    inputs.targetBranch ??= config.config.default_branch ?? 'main';
    Object.assign(value, {workflow: name, checkout: config.root, launch: args.work}); put(uid!, value, row);
    const launch = await handle({method: 'command', work: args.work, key: 'swamp-process', cwd: config.root,
      argv: ['swamp', 'workflow', 'run', name, '--no-supersede', '--input', encode(inputs), '--json']}) as Delivery;
    value = result(uid!); [, row] = read(uid!);
    Object.assign(value, {outcome: launch.outcome, pane: launch.pane, supervisor: launch.supervisor, process: launch.process,
      checkout: config.root, log: launch.log, workflow: name, launch: args.work}); put(uid!, value, row);
  } else if (args.method === 'pane_action') {
    if (args.action === 'leave') { value.outcome = 'completed'; put(uid!, value, row); closeView(args.work); return value; }
    const source = await owned({work: args.work, agent: 'source'});
    assert(z.command('GetPaneInfo', terminal(source.pane))?.exited, 'Source pane is no longer exited');
    if (args.action === 'restart') {
      z.command('RerunCommandPane', source.pane); value.outcome = 'completed'; put(uid!, value, row); closeView(args.work);
    } else {
      assert(args.action === 'fork' || args.action === 'duplicate', 'Unknown pane choice');
      value.outcome = 'pending'; put(uid!, value, row);
      message({plugin_url: 'zellij:hocus-new-agent', message_name: 'swamp-pane-action',
        plugin_config: {caller_terminal_pane_id: String(source.pane), [args.action === 'fork' ? 'fork_focused' : 'duplicate_focused']: 'true',
          swamp_reply_plugin: String(z.getPluginIds().plugin_id), swamp_work: args.work, swamp_key: args.key},
        new_plugin_args: {should_float: true, should_focus: true, pane_id_to_replace: null, pane_title: null, cwd: null, skip_cache: false}});
    }
  } else if (args.method === 'agent_spawn') {
    assert(args.spec, 'Missing agent specification');
    assert(!args.spec.prompt, 'Spawn without prompt, then call agent_prompt to register observation first');
    const owner = {...identity, work: args.work, agent: args.key};
    value.owner = owner; value.outcome = 'pending'; put(uid!, value, row);
    const {source_directory, automatic_firecracker_vm, ...spec} = args.spec;
    const opts = {...spec,
      ...(source_directory !== undefined ? {sourceDirectory: source_directory} : {}),
      ...(automatic_firecracker_vm !== undefined ? {automaticFirecrackerVm: automatic_firecracker_vm} : {}),
      context: {swamp_owner: encode(owner), swamp_operation: args.key}, startSuppressed: !!read(scope(args.work, 'view'))[0]};
    const spawned = z.agent.spawnDetails(opts);
    assert('Terminal' in spawned.pane_id, 'Expected an agent terminal');
    value = result(uid!); [, row] = read(uid!);
    value.pane = spawned.pane_id.Terminal; value.branch = spawned.branch ?? null; value.checkout = spawned.cwd ?? null;
    put(uid!, value, row);
    const info = z.command('GetPaneInfo', terminal(value.pane));
    value = result(uid!); [, row] = read(uid!); value.target = info?.target; put(uid!, value, row);
    let host: Supervisor;
    if (typeof value.target === 'object' && 'Firecracker' in value.target) {
      const deadline = Date.now() + 150000;
      for (;;) {
        try { host = await supervisor(value.pane!); break; } catch (error) {
          assert(Date.now() < deadline, 'VM dispatch has a pane but no verified transport; reconcile it without repeating spawn');
          await z.sleep(200);
        }
      }
    } else host = await supervisor(value.pane!);
    value = result(uid!); [, row] = read(uid!);
    Object.assign(value, {supervisor: host.host_id, process: host.child_pid, remoteHost: host.remote_host, remotePaneId: host.pane_token, outcome: 'completed'});
    put(uid!, value, row);
  } else if (args.method === 'agent_prompt') {
    await owned(args, true);
    const agentUid = scope(args.work, args.agent), [agent, agentRow] = read(agentUid);
    assert(agent && agentRow && row, 'Agent disappeared');
    assert(!agent.outstanding, 'One outstanding prompt is permitted per agent');
    Object.assign(value, {owner: agent.owner, pane: agent.pane, outcome: 'pending', supervisor: agent.supervisor, process: agent.process,
      remoteHost: agent.remoteHost, remotePaneId: agent.remotePaneId, checkout: agent.checkout, branch: agent.branch, target: agent.target});
    agent.outstanding = uid;
    // Publish observation and the exclusive prompt slot in one transaction before sending bytes.
    D.apply([D.put('delivery', {value: encode(value)}, {uid, expectRev: row.rev}),
      D.put('delivery', {value: encode(agent)}, {uid: agentUid, expectRev: agentRow.rev})]);
    const placement = placements.get(args.work);
    if (placement && !placement.detached) { placement.queue.push(uid!); present(args.work); }
    dispatchPrompt(agent);
  } else if (args.method === 'command') {
    assert(Array.isArray(args.argv) && args.argv.length > 0 && args.argv.every(arg => typeof arg === 'string'), 'Missing literal argv');
    const owner = {...identity, work: args.work, agent: args.key};
    value.owner = owner; value.outcome = 'pending'; put(uid!, value, row);
    const pane = z.command('OpenCommandPaneBackground', [{path: args.argv[0], args: args.argv.slice(1), cwd: args.cwd},
      {swamp_owner: encode(owner), swamp_operation: args.key, swamp_work: read<Attachment>(scope(args.work, 'attachment'))[0]?.work ?? args.work}]);
    value = result(uid!); [, row] = read(uid!);
    assert(pane && 'Terminal' in pane, 'Command dispatch was not acknowledged; reconcile the saved operation before any further action');
    value.pane = pane.Terminal; value.checkout = args.cwd; value.log = `terminal_${value.pane}`; put(uid!, value, row);
    const host = await supervisor(value.pane);
    value = result(uid!); [, row] = read(uid!); value.supervisor = host.host_id; value.process = host.child_pid; put(uid!, value, row);
  } else if (['source_sync', 'vm_inspect', 'vm_sync', 'vm_status'].includes(args.method)) {
    const agent: Partial<Delivery> = args.method === 'source_sync'
      ? (read(scope(args.work, 'source'))[0] ? await owned({work: args.work, agent: 'source'}) : {target: 'Local'}) : await owned(args);
    value.owner = agent.owner; value.target = agent.target;
    const target = typeof agent.target === 'object' && 'Firecracker' in agent.target ? agent.target.Firecracker : undefined;
    if (!target) Object.assign(value, {outcome: 'completed', vm: null, clean: true});
    else {
      const name = target.vm;
      assert(name, 'Missing VM identity');
      let warnings = '', output = '';
      if (args.method === 'vm_sync' || args.method === 'source_sync') {
        const r = await z.runCommand({command: [z.serverBinary || 'zellij', 'firecracker', 'sync', name, '--session', identity.session], cwd: '/',
          context: {swamp_owner: encode(identity)}, maxBytesReturned: 1048576});
        warnings = z.decodeUtf8(r.stderr); output = z.decodeUtf8(r.stdout);
        assert(r.exitCode === 0, `VM sync failed; inspect provider reports before retrying: ${warnings}`);
      }
      const count = await command([z.serverBinary || 'zellij', 'firecracker', 'changes', name, '--session', identity.session]);
      assert(/^\d+\s*$/.test(count), 'Invalid guest change count');
      Object.assign(value, {outcome: 'completed', vm: name, output: output + warnings + count, clean: Number(count) === 0 && warnings === ''});
    }
    if (uid) put(uid, value, row); else return value;
  } else throw new Error('Unknown bridge method');
  return result(uid!);
}
type History = {id: string; workflowName: string; status: string; inputs: {launch: string; session: string}};
function preview(path: string): string {
  return String.raw`"output":has(VALUE.output) ? (VALUE.output.size() > 16384 ? VALUE.output.substring(0,
    VALUE.output.substring(16383,16384).matches(r"[\uD800-\uDBFF]") ? 16383 : 16384) : VALUE.output) : "",
    "previewTruncated":has(VALUE.output) && VALUE.output.size() > 16384`.replaceAll('VALUE', path);
}
function testPreview(path: string): string {
  return `{"evidence":${path}.evidence,"outcome":${path}.outcome,"exitCode":${path}.exitCode,"log":${path}.log,"passed":${path}.passed,${preview(path)}}`;
}
const outputProjection = `{"model":modelName,"name":name,"version":version,"attributes":
  specName == "diff" ? {"evidence":attributes.evidence,${preview('attributes')}} :
  specName == "test" ? ${testPreview('attributes')} :
  specName == "operation" && has(attributes.output) ? {
    "outcome":attributes.outcome,"pane":has(attributes.pane) ? attributes.pane : false,
    "log":has(attributes.log) ? attributes.log : "", "exitCode":has(attributes.exitCode) ? attributes.exitCode : null,
    "outputTruncated":has(attributes.outputTruncated) && attributes.outputTruncated,
    "logError":has(attributes.logError) ? attributes.logError : false,${preview('attributes')}} :
  specName in ["checks","assessment"] ? {"review":attributes.review,
    "test":attributes.test == null ? attributes.test : ${testPreview('attributes.test')},
    "evidence":has(attributes.evidence) ? attributes.evidence : null,
    "tests":has(attributes.tests) ? attributes.tests : "", "codeReview":has(attributes.codeReview) ? attributes.codeReview : "",
    "ready":has(attributes.ready) && attributes.ready} : attributes}`;
function message(options: Partial<MessageToPlugin> & {message_name: string}) {
  z.command('MessageToPlugin', {plugin_url: null, destination_plugin_id: null, plugin_config: {}, message_payload: null,
    message_args: {}, new_plugin_args: null, floating_pane_coordinates: null, ...options});
}
async function invoke(args: Partial<Request>): Promise<Json> {
  const newLaunch = args?.method === 'workflow_run' && !read(scope(args.work!, args.key!))[0];
  try { return JSON.parse(encode(await handle(args))); } catch (error) {
    try {
      if (newLaunch && !read(scope(args.work!, 'swamp-process'))[0]) saveEvent(scope(args.work!, args.key!), 'failed', {error: String(error)});
      const [value] = read(scope(args.work!, args.key!));
      if (value && sameRequest(value.request, args) && value.outcome === 'pending') {
        saveEvent(scope(args.work!, args.key!), 'unknown', {error: String(error)}); present(args.work!);
      }
    } catch { /* Preserve the original dispatch error. */ }
    throw error;
  }
}
export default definePlugin({
  load() {
    identity = {host_id: z.getPluginIds().host_id, session: z.serverSession, incarnation: z.serverIncarnation};
    assert(identity.session !== '', 'Bridge requires a server session');
    D.define('delivery', {value: {kind: 'Text', required: true}}, {version: 1});
    z.subscribe(['AgentEvent', 'CommandPaneExited']);
    z.rpc.handle('swamp', async (payload, call) => {
      const from = call.source?.Plugin;
      assert(from != null, 'Workflow RPC requires a host-authenticated plugin source');
      const pane = z.command('GetPaneInfo', plugin(from));
      assert(pane?.plugin_url === 'zellij:agent-exit' || pane?.plugin_url === 'zellij:swamp-workflow', 'Only builtin workflow views may use the privileged bridge RPC');
      const args = payload as Partial<Request>;
      if (args.method === 'workflow_run' && pane.plugin_url === 'zellij:agent-exit') {
        const uid = scope(args.work!, 'view'), [existing] = read(uid);
        if (existing) assert(existing.view === from && existing.incarnation === identity.incarnation, 'Launch already belongs to another hook instance');
        else { put(uid, {view: from, incarnation: identity.incarnation}); placements.set(args.work!, {view: from, queue: []}); }
      }
      return invoke(args);
    });
  },
  async pipe(messageIn) {
    if (messageIn.name === 'swamp-pane-result' && messageIn.payload) {
      const from = typeof messageIn.source === 'object' && 'Plugin' in messageIn.source ? messageIn.source.Plugin : undefined;
      if (from == null) return;
      assert(z.command('GetPaneInfo', plugin(from))?.plugin_url === 'zellij:hocus-new-agent', 'Pane result must come from the agent picker');
      const reply: {work: string; key: string; choice: string} = JSON.parse(messageIn.payload);
      assert(reply.choice === 'started' || reply.choice === 'cancelled', 'Invalid picker result');
      const uid = scope(reply.work, reply.key), [current] = read(uid);
      assert(current && JSON.parse(current.request).method === 'pane_action', 'No picker operation');
      if (current.outcome === 'completed') assert(current.choice === reply.choice, 'Picker result already differs');
      else assert(['pending', 'unknown'].includes(current.outcome), 'Picker dispatch cannot accept a result');
      saveEvent(uid, 'completed', {choice: reply.choice});
      message({destination_plugin_id: from, message_name: 'swamp-pane-ack'});
      if (reply.choice === 'started') closeView(reply.work);
      return;
    }
    if (messageIn.name !== 'swamp' || !messageIn.payload || typeof messageIn.source !== 'object' || !('Cli' in messageIn.source)) return;
    const pipe = messageIn.source.Cli;
    z.command('BlockCliPipeInput', pipe);
    try {
      let reply;
      try { reply = {result: await invoke(JSON.parse(messageIn.payload))}; } catch (error) { reply = {error: String(error)}; }
      z.command('CliPipeOutput', [pipe, encode(reply)]);
    } finally { z.command('UnblockCliPipeInput', pipe); }
  },
});
z.on('AgentEvent', event => {
  let raw: { _zellij_workflow_owner?: string; prompt_sha256?: string; turn_id?: string };
  try { raw = JSON.parse(event.raw || ''); } catch { return; }
  if (!raw._zellij_workflow_owner) return;
  const owner: Owner = JSON.parse(raw._zellij_workflow_owner);
  if (owner.host_id !== identity.host_id || owner.session !== identity.session) return;
  const uid = scope(owner.work, owner.agent), [agent] = read(uid);
  if (!agent || (agent.pane != null && agent.pane !== event.pane_id) || encode(agent.owner) !== encode(owner)) return;
  if (event.hook === 'SessionStart') {
    const current = result(uid), [, row] = read(uid); current.ready = true; put(uid, current, row);
    if (current.outstanding && current.supervisor) void owned({work: owner.work, agent: owner.agent}, true).then(dispatchPrompt).catch(error => z.log(String(error)));
  }
  if (event.hook === 'UserPromptSubmit' && agent.outstanding) {
    const [value, row] = read(agent.outstanding);
    if (value?.outcome === 'pending') {
      const request: Request = JSON.parse(value.request);
      const matches = request.prompt_sha256 && raw.prompt_sha256 ? request.prompt_sha256 === raw.prompt_sha256 : request.prompt === event.prompt;
      if (matches && !value.accepted) {
        value.accepted = true; value.agentSession = event.session_id; value.turn = raw.turn_id; put(agent.outstanding, value, row);
      }
    }
  }
  finishAgent(owner, event, raw);
});
function finishAgent(owner: Owner, event: AgentEvent, raw: {turn_id?: string}) {
  const uid = scope(owner.work, owner.agent), [agent, row] = read(uid);
  if (!agent || encode(agent.owner) !== encode(owner) || !agent.outstanding) return;
  const outcome = ({Stop: 'completed', StopFailure: 'failed', Interrupt: 'interrupted'} as Record<string, string>)[event.hook];
  if (!outcome) return;
  const operation = result(agent.outstanding);
  if (!operation.accepted || operation.agentSession !== event.session_id || (operation.turn && operation.turn !== raw.turn_id)) return;
  if (JSON.parse(operation.request).waitForExit) {
    const current = result(agent.outstanding), [, currentRow] = read(agent.outstanding);
    current.message = event.last_message ?? null; current.hook = event.hook; put(agent.outstanding, current, currentRow); return;
  }
  saveEvent(agent.outstanding, outcome, {message: event.last_message ?? null, hook: event.hook, agentSession: event.session_id ?? null});
  delete agent.outstanding; put(uid, agent, row); present(owner.work);
}
z.on('CommandPaneExited', ([pane, exitCode, context]) => {
  if (!context.swamp_owner) return;
  const owner: Owner = JSON.parse(context.swamp_owner);
  if (owner.host_id !== identity.host_id || owner.session !== identity.session) return;
  const uid = scope(owner.work, context.swamp_operation || owner.agent), [value] = read(uid);
  if (!value || encode(value.owner) !== encode(owner) || (value.pane != null && value.pane !== pane)) return;
  const request: Request = JSON.parse(value.request);
  saveEvent(uid, request.method === 'command' ? (exitCode === 0 ? 'completed' : 'failed') : 'process-exit', {exitCode});
  if (request.method === 'agent_spawn') { const current = result(uid), [, row] = read(uid); current.processExited = true; put(uid, current, row); }
  if (value.outstanding) saveEvent(value.outstanding, 'process-exit', {exitCode});
  present(owner.work);
});
