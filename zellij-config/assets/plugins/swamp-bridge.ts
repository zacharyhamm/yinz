// Durable delivery adapter. Swamp owns the DAG, evidence, questions and decisions.
import z, {definePlugin, type Json, type DataRecord, type Integer, type PaneId,
  type PaneTarget, type AgentSpawnOptions, type AgentEvent, type MessageToPlugin} from 'zellij:plugin';

type Owner = {host_id: string; session: string; incarnation: string; work: string; agent: string};
type Identity = Pick<Owner, 'host_id' | 'session' | 'incarnation'>;
type Request = {
  method: string; work: string; key: string; agent: string; operation: string;
  directory: string; run: string; launch: string; workflow: string;
  prompt: string; prompt_sha256?: string; waitForExit?: boolean; includeOutput?: boolean; foreground?: boolean;
  action: string; spec: AgentSpawnOptions & {source_directory?: string; automatic_firecracker_vm?: boolean}; argv: string[]; cwd: string;
  inputs?: Record<string, Json>;
  payload: Record<string, Json>; model: string;
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
  transport?: string; requestId?: string; correlation?: string; run?: string;
};
type Supervisor = {terminal_id: number; host_id: string; child_pid?: number; attached: boolean;
  session_name: string; child_exit_status?: number | null; owner?: string | null; remote_host?: string; pane_token?: string};
const D = z.data;
let identity: Identity;
const placements = new Map<string, {view: number; queue: string[]; current?: number; detached?: boolean; manual?: boolean}>();
function assert(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
// Lua's encoder sorts object keys recursively. Preserve exact request/owner comparisons on reload.
function encode(value: unknown): string {
  return JSON.stringify(value, (_key, item) => item && typeof item === 'object' && !Array.isArray(item)
    ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
}
// Lua decoded null object fields as absent and encoded empty tables as arrays.
// Normalize both sides when comparing a persisted request from either runtime.
function sameRequest(saved: string, request: unknown): boolean {
  const previous = JSON.parse(saved), current = request as Partial<Request>;
  const forwarded = current?.method === 'swamp_dispatch' ? 'payload' : current?.method === 'workflow_run' ? 'inputs' : undefined;
  if (forwarded && encode(previous[forwarded]) !== encode(current[forwarded])) return false;
  const normalize = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(normalize);
    if (value && typeof value === 'object') {
      const entries = Object.entries(value).filter(([, v]) => v != null).map(([k, v]) => [k, normalize(v)]);
      return entries.length ? Object.fromEntries(entries) : [];
    }
    return value;
  };
  return encode(normalize(previous)) === encode(normalize(request));
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
function closeView(work: string, restoreSource?: number) {
  const [placement] = read(scope(work, 'view'));
  if (!placement || placement.incarnation !== identity.incarnation || placement.view == null) return;
  const pane = z.command('GetPaneInfo', plugin(placement.view));
  if (pane?.plugin_url === 'zellij:agent-exit') {
    if (restoreSource != null && !pane.is_suppressed) z.command('ReplacePaneWithExistingPane', [plugin(placement.view), terminal(restoreSource), true]);
    z.command('ClosePluginPane', placement.view);
  }
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
  if ((info?.plugin_url !== 'zellij:agent-exit' && info?.plugin_url !== 'zellij:swamp-workflow') || !currentInfo || currentInfo.is_suppressed) { placement.detached = true; return; }
  let next: number | undefined;
  while (placement.queue.length) {
    const [operation] = read(placement.queue[0]);
    if (operation?.outcome === 'pending') { next = operation.pane; break; }
    placement.queue.shift();
  }
  if (next === placement.current) return;
  const target = next == null ? view : terminal(next);
  if (!z.command('GetPaneInfo', target)?.is_suppressed) { placement.detached = true; return; }
  if (placement.current == null && next != null) {
    const source = read(scope(work, 'source'))[0];
    if (source?.pane != null) z.command('HidePaneWithId', terminal(source.pane));
  }
  z.command('ReplacePaneWithExistingPane', [current, target, true]);
  placement.current = next;
}
function attachManual(work: string, view: number) {
  if (read(scope(work, 'view'))[0]) return;
  const existing = placements.get(work);
  if (existing && z.command('GetPaneInfo', plugin(existing.view))) return;
  const placement = {view, queue: [] as string[], manual: true};
  placements.set(work, placement);
  let cursor: Integer | null = null;
  do {
    // Default row-id order preserves delivery order across pages.
    const {records, raw} = D.fetch({entity: 'delivery', afterId: cursor});
    for (const row of records) {
      const id: string[] = JSON.parse(String(row.uid));
      if (id[0] !== identity.host_id || id[1] !== identity.session || id[2] !== work) continue;
      const operation: Delivery = JSON.parse(String(row.value));
      if (operation.outcome !== 'pending' || operation.pane == null) continue;
      const request: Request = JSON.parse(operation.request);
      if (request.method === 'command' && request.foreground) placement.queue.push(String(row.uid));
    }
    cursor = raw.next_cursor;
  } while (cursor != null);
  present(work);
}
async function command(argv: string[], cwd = '/'): Promise<string> {
  const r = await z.runCommand({command: argv, cwd, context: {swamp_owner: encode(identity)}, maxBytesReturned: 1048576});
  const stdout = z.decodeUtf8(r.stdout), stderr = z.decodeUtf8(r.stderr);
  assert(r.exitCode === 0, `Host command failed: ${encode(argv)}\nDirectory: ${cwd}\nExit status: ${r.exitCode}\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  return stdout;
}
type SwampRequest = {operation: string; payload?: Record<string, Json>; id?: string; started?: boolean};
const tracked = new Map<string, string>(), watching = new Set<string>();
const watchIds = new Map<string, {run: string; work: string}>(), notifications = new Set<string>();
function changed(work: string) {
  if (notifications.has(work)) return;
  notifications.add(work);
  void z.sleep(100).then(() => {
    notifications.delete(work);
    const view = read(scope(work, 'view'))[0];
    if (view?.incarnation === identity.incarnation && view.view != null) message({destination_plugin_id: view.view, message_name: 'workflow-changed'});
    const placement = placements.get(work);
    if (placement?.manual && !placement.detached) message({destination_plugin_id: placement.view, message_name: 'workflow-changed'});
  });
}
async function swamp(request: SwampRequest, directory: string): Promise<string> {
  const reply = await z.swampRequest({...request, directory}) as Record<string, any>;
  if (reply.error) throw Error(encode(reply.error));
  if (request.operation === 'data.query' && reply.limited) throw Error('Swamp data query reached its result limit; narrow the saved-data query');
  if (request.operation === 'data.query' && reply.projected) {
    reply.results = reply.projected.rows ?? reply.projected.values ?? []; delete reply.projected;
  } else if (request.operation === 'workflow.history.logs' && reply.log) return (reply.log.lines ?? []).join('\n');
  return encode(reply);
}
function watchRun(run: string, work: string, root: string) {
  if (watching.has(run)) return;
  const id = `${z.instanceId}-watch-${run}`;
  watching.add(run); watchIds.set(id, {run, work});
  void swamp({operation: 'run.attach', payload: {runId: run, afterSeq: 0}, id}, root)
    .catch(() => {}).finally(() => {watching.delete(run); watchIds.delete(id);});
}
async function dispatch(uid: string, root: string, operation: string, payload: Record<string, Json>) {
  let [value, row] = read(uid);
  assert(value, 'No operation with that identity');
  Object.assign(value, {transport: 'serve', requestId: z.instanceId + uid, checkout: root, correlation: uid, outcome: 'unknown'});
  put(uid, value, row);
  tracked.set(value.requestId!, uid);
  if (operation === 'workflow.run') {
    payload = {...payload, noSupersede: true, runtimeTags: {...(payload.runtimeTags as Record<string, Json> ?? {}), zellijOperation: uid}};
  }
  const reply = JSON.parse(await swamp({operation, payload, id: value.requestId, started: true}, root));
  [value, row] = read(uid); assert(value, 'No operation with that identity');
  if (reply.runId) value.run = reply.runId;
  if (value.outcome === 'unknown' && tracked.has(value.requestId!)) value.outcome = 'pending';
  put(uid, value, row); return value;
}
async function serveObserve(uid: string, operation: Delivery, row?: DataRecord): Promise<Delivery> {
  let attachment: {ok: boolean; error?: string} | undefined;
  for (let attempt = 1; attempt <= 8; attempt++) {
    if (!['pending', 'unknown'].includes(operation.outcome)) return operation;
    const request = JSON.parse(operation.request);
    if (request.operation === 'model.method.run') {
      if (operation.run) {
        const id = `${z.instanceId}-attach-${uid}`;
        if (![...tracked.values()].includes(uid) && !attachment) {
          tracked.set(id, uid);
          try { await swamp({operation: 'run.attach', payload: {runId: operation.run, afterSeq: 0}, id, started: true}, operation.checkout!); attachment = {ok: true}; }
          catch (error) { tracked.delete(id); attachment = {ok: false, error: String(error)}; }
        }
        if (attachment) {
          operation.outcome = attachment.ok && tracked.has(id) ? 'pending' : 'unknown';
          if (attachment.ok) operation.dispatcher = z.instanceId; else operation.error = attachment.error;
        }
      } else operation.outcome = 'unknown';
    } else {
      if (request.operation === 'workflow.resume') operation.run ??= request.payload.runId;
      if (!operation.run && request.operation !== 'workflow.resume') {
        const history = JSON.parse(await swamp({operation: 'workflow.history.search', payload: {filter: `has(tags.zellijOperation) && tags.zellijOperation == ${encode(operation.correlation)}`}}, operation.checkout!));
        assert((history.results ?? []).length <= 1, 'Multiple runs have the same operation identity; reconcile explicitly');
        operation.run = history.results?.[0]?.runId;
      }
      if (operation.run) {
        const history = JSON.parse(await swamp({operation: 'workflow.history.get', payload: {workflowIdOrName: operation.run}}, operation.checkout!));
        if (request.operation !== 'workflow.resume' || history.inputs?.submission === request.payload.inputs.submission) {
          if (history.status === 'running') {
            if (operation.outcome === 'unknown' || operation.dispatcher !== z.instanceId) {
              try { await swamp({operation: 'run.attach', payload: {runId: operation.run, afterSeq: 0}, started: true}, operation.checkout!); operation.outcome = 'pending'; operation.dispatcher = z.instanceId; }
              catch { operation.outcome = 'unknown'; operation.error = 'Execution is unavailable; reconcile persisted history before proceeding'; }
            } else operation.outcome = 'pending';
          } else if (['succeeded', 'suspended'].includes(history.status)) {operation.outcome = 'completed'; operation.exitCode = 0;}
          else if (['failed', 'cancelled', 'interrupted'].includes(history.status)) {operation.outcome = 'failed'; operation.exitCode = 1;}
        }
      }
    }
    try {put(uid, operation, row); return operation;} catch (error) {if (!D.isConflict(error) || attempt === 8) throw error;}
    const [fresh, freshRow] = read(uid); assert(fresh, 'No operation with that identity');
    operation = fresh; row = freshRow;
  }
  return operation;
}
z.on('CustomMessage', ([name, payload]) => {
  if (name !== 'swamp-response') return;
  const frame = JSON.parse(payload), watch = watchIds.get(frame.id);
  if (watch) {changed(watch.work); return;}
  const uid = tracked.get(frame.id);
  if (!uid) return;
  const [operation, row] = read(uid);
  if (!operation) return;
  let write = false;
  const pending = ['pending', 'unknown'].includes(operation.outcome), method = JSON.parse(operation.request).operation === 'model.method.run';
  if (frame.event?.runId) {operation.run = frame.event.runId; if (pending) operation.outcome = 'pending'; write = true;}
  if (pending && frame.event && (['completed', 'cancelled'].includes(frame.event.kind) || (!method && frame.event.kind === 'suspended'))) {
    const status = frame.event.run?.status;
    const succeeded = frame.event.kind !== 'cancelled' && (status === 'succeeded' || (!method && status === 'suspended'));
    operation.outcome = succeeded ? 'completed' : 'failed'; operation.exitCode = succeeded ? 0 : 1;
    if (!succeeded) operation.error = encode(frame.event); write = true;
  }
  if (pending && frame.event?.kind === 'error') {operation.error = encode(frame.event.error); operation.outcome = 'failed'; operation.exitCode = 1; write = true;}
  if (frame.type === 'done') {
    if (['pending', 'unknown'].includes(operation.outcome)) operation.outcome = 'unknown';
    tracked.delete(frame.id); write = true;
  } else if (frame.type === 'error') {
    if (pending) {
      operation.error = encode(frame.error);
      const attaching = method && frame.id === `${z.instanceId}-attach-${uid}`;
      operation.outcome = attaching || ['transport_unknown', 'host_error', 'not_found'].includes(frame.error.code) ? 'unknown' : 'failed';
    }
    tracked.delete(frame.id); write = true;
  }
  if (write) put(uid, operation, row);
  changed(read<Attachment>(scope(operation.work, 'attachment'))[0]?.work ?? operation.work);
});
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
  assert(agent.owner.work === args.work && agent.owner.agent === args.agent, 'Wrong work or agent');
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
  const history = JSON.parse(await swamp({operation: 'workflow.history.search', payload: {inputs: {launch: work}}}, root));
  for (const run of history.results ?? []) if (run.inputs.session === identity.session) {
    await handle({method: 'workflow_attach', work, key: 'attachment', run: run.runId, workflow: run.workflowName, directory: root});
  }
  return read(scope(work, 'runs'))[0];
}
async function handle(input: Partial<Request>, manualView?: number): Promise<unknown> {
  assert(input && typeof input === 'object' && !Array.isArray(input), 'Expected a typed request');
  // Each method validates the fields it consumes; correlation keys are checked by scope().
  const args = input as Request;
  if (args.method === 'identity') return identity;
  if (args.method === 'swamp_read') {
    assert(['workflow.history.get', 'workflow.history.search', 'data.query', 'workflow.approve'].includes(args.operation), 'Unsupported bridge request');
    return JSON.parse(await swamp({operation: args.operation, payload: args.payload}, args.directory));
  }
  if (args.method === 'workflow_answer') {
    const reply = JSON.parse(await swamp({operation: 'model.method.run', payload: {modelIdOrName: args.model, methodName: 'submit', inputs: args.inputs ?? {}}}, args.directory));
    if (['failed', 'cancelled'].includes(reply.run?.status)) throw Error(encode(reply.run));
    return reply.run ?? reply;
  }
  if (args.method === 'workflow_cancel') {
    const snapshot = await handle({method: 'workflow_show', directory: args.directory, run: args.run}) as {root: string; history: History; children: History[]};
    const operations = [];
    for (const history of [snapshot.history, ...snapshot.children]) if (history.status === 'running') {
      operations.push(JSON.parse(await swamp({operation: 'workflow.cancel', payload: {runId: history.id}}, snapshot.root)));
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
        const history: History = JSON.parse(await swamp({operation: 'workflow.history.get', payload: {workflowIdOrName: args.run}}, config.root));
        assert(history.inputs.session === identity.session, 'Run belongs to a different session');
        work = history.inputs.launch;
      }
    } else if (args.launch) work = args.launch;
    if (work && manualView != null) attachManual(work, manualView);
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
    const history = JSON.parse(await swamp({operation: 'workflow.history.get', payload: {workflowIdOrName: run}}, config.root));
    const ids = Object.keys(result(scope(attachment.work, 'runs')).entries ?? {});
    const questions = JSON.parse(await swamp({operation: 'data.query', payload: {predicate: `specName == "question" && attributes.run in ${encode(ids)}`, select: '{"model": modelName, "version": version, "attributes": attributes}'}}, config.root));
    const outputs = JSON.parse(await swamp({operation: 'data.query', payload: {predicate: `has(tags.workflowRunId) && tags.workflowRunId in ${encode(ids)}`, select: outputProjection}}, config.root));
    const children = [];
    if (history.status === 'running') watchRun(run, attachment.work, config.root);
    for (const id of ids) if (id !== run) {
      const child = JSON.parse(await swamp({operation: 'workflow.history.get', payload: {workflowIdOrName: id}}, config.root));
      children.push(child);
      if (child.status === 'running') watchRun(child.id, attachment.work, config.root);
    }
    return {run, launch: attachment.work, root: config.root, history, children, active: active(attachment.work), outputs,
      logs: await swamp({operation: 'workflow.history.logs', payload: {runIdOrWorkflow: run, tail: 100}}, config.root), questions};
  }
  key(args.work);
  if (args.method === 'lookup') return read(scope(args.work, args.key))[0] ?? null;
  if (args.method === 'observe') {
    const uid = scope(args.work, args.key), [operation, row] = read(uid);
    assert(operation, 'No operation with that identity');
    if (operation.transport === 'serve') return serveObserve(uid, operation, row);
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
    if (!existing) changed(args.work);
    return {work: args.work, key: args.key, request: encode(args), incarnation: identity.incarnation, outcome: 'completed'};
  }
  if (args.method === 'reconcile') {
    const uid = scope(args.work, args.operation), [operation, row] = read(uid);
    assert(operation, 'Operation is absent');
    if (JSON.parse(operation.request).method === 'workflow_run' && operation.checkout) await discover(args.work, operation.checkout);
    if (operation.transport === 'serve') return serveObserve(uid, operation, row);
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
  let uid: string | undefined, value: Delivery | undefined, row: DataRecord | undefined;
  if (args.method === 'vm_status') value = {work: args.work, key: args.key, request: encode(args), incarnation: identity.incarnation} as Delivery;
  else {
    const [claimed, previous] = claim(args); uid = claimed;
    if (previous) return previous;
    [value, row] = read(uid); assert(value, 'No operation with that identity');
  }
  if (args.method === 'workflow_run') {
    assert(args.directory, 'Missing project directory');
    const config = z.projectConfig.read(args.directory);
    assert(!config.error, config.error ?? 'Invalid project config');
    const name = args.workflow ?? config.config.workflow;
    assert(name, 'No exit workflow configured. Use Hocus /project-setup.');
    const validation = await swamp({operation: 'workflow.validate', payload: {workflowIdOrName: name}}, config.root);
    assert(JSON.parse(validation).passed === true, 'Workflow validation failed: ' + validation);
    const inputs = {...args.inputs ?? {}};
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
    return dispatch(uid!, config.root, 'workflow.run', {workflowIdOrName: name, inputs});
  } else if (args.method === 'swamp_dispatch') {
    return dispatch(uid!, args.directory, args.operation, args.payload);
  } else if (args.method === 'pane_action') {
    let source: (Delivery & {pane: number}) | undefined;
    const saved = read(scope(args.work, 'source'))[0];
    if (args.action !== 'leave' || saved) {
      const closed = args.action === 'leave' && saved?.owner?.incarnation === identity.incarnation && saved.pane != null && !z.command('GetPaneInfo', terminal(saved.pane));
      if (closed) assert(saved.owner?.host_id === identity.host_id && saved.owner.session === identity.session && saved.owner.work === args.work && saved.owner.agent === 'source' && saved.supervisor, 'Wrong source ownership');
      else source = await owned({work: args.work, agent: 'source'});
    }
    if (source) assert(z.command('GetPaneInfo', terminal(source.pane))?.exited, 'Source pane is no longer exited');
    if (args.action === 'leave') {
      if (source) z.command('CloseTerminalPane', source.pane);
      value.outcome = 'completed'; put(uid!, value, row); closeView(args.work); return value;
    }
    assert(source, 'Missing source pane');
    if (args.action === 'restart') {
      closeView(args.work, source.pane); z.command('RerunCommandPane', source.pane); value.outcome = 'completed'; put(uid!, value, row);
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
    [value, row] = read(uid!); assert(value, 'No operation with that identity');
    value.pane = spawned.pane_id.Terminal; value.branch = spawned.branch ?? null; value.checkout = spawned.cwd ?? null;
    put(uid!, value, row);
    const info = z.command('GetPaneInfo', terminal(value.pane));
    [value, row] = read(uid!); assert(value, 'No operation with that identity'); value.target = info?.target; put(uid!, value, row);
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
    [value, row] = read(uid!); assert(value, 'No operation with that identity');
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
    if (placement && !placement.manual && !placement.detached) { placement.queue.push(uid!); present(args.work); }
    dispatchPrompt(agent);
  } else if (args.method === 'command') {
    assert(Array.isArray(args.argv) && args.argv.length > 0 && args.argv.every(arg => typeof arg === 'string'), 'Missing literal argv');
    const owner = {...identity, work: args.work, agent: args.key};
    value.owner = owner; value.outcome = 'pending'; put(uid!, value, row);
    const pane = z.command('OpenCommandPaneBackground', [{path: args.argv[0], args: args.argv.slice(1), cwd: args.cwd ?? null},
      {swamp_owner: encode(owner), swamp_operation: args.key, swamp_work: read<Attachment>(scope(args.work, 'attachment'))[0]?.work ?? args.work}]);
    [value, row] = read(uid!); assert(value, 'No operation with that identity');
    assert(pane && 'Terminal' in pane, 'Command dispatch was not acknowledged; reconcile the saved operation before any further action');
    value.pane = pane.Terminal; value.checkout = args.cwd; value.log = `terminal_${value.pane}`; put(uid!, value, row);
    const placement = placements.get(args.work);
    if (args.foreground && placement && !placement.detached) { placement.queue.push(uid!); present(args.work); }
    const host = await supervisor(value.pane);
    [value, row] = read(uid!); assert(value, 'No operation with that identity'); value.supervisor = host.host_id; value.process = host.child_pid; put(uid!, value, row);
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
  return `{"evidence":${path}.evidence,"outcome":${path}.outcome,"exitCode":${path}.exitCode,"log":${path}.log,"passed":${path}.passed,"outputRef":has(${path}.outputRef) ? ${path}.outputRef : null,${preview(path)}}`;
}
const outputProjection = `{"model":modelName,"name":name,"version":version,"attributes":
  specName == "diff" ? {"evidence":attributes.evidence,${preview('attributes')}} :
  specName == "test" ? ${testPreview('attributes')} :
  specName == "operation" ? {
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
let timingAttempt = 0;
async function invoke(args: Partial<Request>, manualView?: number): Promise<Json> {
  if (!z.swampTiming) return invokeInner(args, manualView);
  const attempt = ++timingAttempt, start = z.monotonicMs();
  let outcome = 'error', response: any;
  try {response = await invokeInner(args, manualView); outcome = 'ok'; return response;}
  finally {
    try {z.log('swamp_timing ' + encode({layer: 'deno', phase: 'invoke', attempt: `${z.instanceId}:${attempt}`,
      elapsedMs: z.monotonicMs() - start, outcome, method: args?.method, work: args?.work, key: args?.key,
      requestId: response?.requestId, run: response?.run}));} catch { /* Keep the operation's result. */ }
  }
}
async function invokeInner(args: Partial<Request>, manualView?: number): Promise<Json> {
  const newLaunch = args?.method === 'workflow_run' && !read(scope(args.work!, args.key!))[0];
  try { return JSON.parse(encode(await handle(args, manualView))); } catch (error) {
    try {
      if (newLaunch && !read(scope(args.work!, 'swamp-process'))[0] && !read(scope(args.work!, args.key!))[0]?.transport) saveEvent(scope(args.work!, args.key!), 'failed', {error: String(error)});
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
      return invoke(args, args.method === 'workflow_show' && pane.plugin_url === 'zellij:swamp-workflow' ? from : undefined);
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
      if (reply.choice === 'started') closeView(reply.work);
      message({destination_plugin_id: from, message_name: 'swamp-pane-ack'});
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
    const [current, row] = read(uid); assert(current, 'No operation with that identity'); current.ready = true; put(uid, current, row);
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
    const [current, currentRow] = read(agent.outstanding);
    assert(current, 'No operation with that identity');
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
  if (request.method === 'agent_spawn') { const [current, row] = read(uid); assert(current, 'No operation with that identity'); current.processExited = true; put(uid, current, row); }
  if (value.outstanding) saveEvent(value.outstanding, 'process-exit', {exitCode});
  present(owner.work); changed(owner.work);
});
