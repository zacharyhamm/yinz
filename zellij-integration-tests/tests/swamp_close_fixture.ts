import real, {definePlugin} from 'zellij:plugin';
let token: string, source: number, view: number, worker: number, release: ((value: any) => void) | undefined;
let currentRun = '', delayed = false, handler: any, agentHandler: any;
const runs: Record<string, any> = {};
const cancelled: Record<string, boolean> = {}, hosts: any[] = [], owners: Record<number, string> = {};
const z: any = {...real,
  command(name: any, args: any) {
    const info = (real.command as any)(name, args);
    if (name === 'GetPaneInfo' && info?.plugin_url?.endsWith('/exit.ts')) info.plugin_url = 'zellij:agent-exit';
    return info;
  },
  projectConfig: {read: () => ({root: '/control', config: {workflow: 'fixture'}})},
  workflowRemotePanes: () => [],
  rpc: {...real.rpc, handle(name: string, fn: any) {
    handler = fn;
    real.rpc.handle(name, (args: any, call) => {
      if (args.method === 'workflow_run') {
        token = args.work; source = args.inputs.sourcePane; view = call.source!.Plugin!;
        hosts.push({terminal_id: source, attached: true, session_name: real.serverSession, host_id: 'source-' + source, child_pid: 100 + source});
      }
      return fn(args, call);
    });
  }},
  on(name: any, callback: any) {if (name === 'AgentEvent') agentHandler = callback; return real.on(name, callback);},
  agent: {...real.agent, spawnDetails(opts: any) {
    const agent = real.agent.spawnDetails(opts), id = (agent.pane_id as {Terminal: number}).Terminal;
    owners[id] = opts.context.swamp_owner;
    hosts.push({terminal_id: id, attached: true, session_name: real.serverSession, host_id: 'host-' + id, child_pid: 100 + id});
    return agent;
  }},
  async runCommand(opts: any) {
    return {exitCode: 0, stdout: real.encodeUtf8(opts.command[0] === 'git' ? '/control\n' : JSON.stringify(hosts)), stderr: new Uint8Array()};
  },
  async swampRequest(opts: any) {
    const op = opts.operation, p = opts.payload;
    if (op === 'workflow.get') return {name: 'fixture'};
    if (op === 'workflow.validate') return {passed: true};
    if (op === 'workflow.run') {
      currentRun = `run-${token}`;
      runs[currentRun] = {id: currentRun, workflowName: 'fixture', status: 'running', jobs: [], inputs: p.inputs, tags: p.runtimeTags};
      return delayed ? new Promise(resolve => {release = resolve;}) : {runId: currentRun};
    }
    if (op === 'workflow.cancel') {cancelled[p.runId] = true; runs[p.runId].status = 'cancelled';}
    if (op === 'workflow.history.search') return {results: delayed ? [] : Object.values(runs)
      .filter(run => (!p.workflow || p.workflow === run.workflowName)
        && Object.entries(p.inputs ?? {}).every(([key, value]) => run.inputs[key] === value)
        && (!p.filter || p.filter === `has(tags.zellijOperation) && tags.zellijOperation == ${JSON.stringify(run.tags.zellijOperation)}`))
      .map(run => ({runId: run.id, workflowName: run.workflowName, inputs: run.inputs}))};
    if (op === 'workflow.history.get') return runs[p.workflowIdOrName];
    if (op === 'workflow.history.logs') return {log: {lines: ['fixture workflow']}};
    if (op === 'data.query') return {results: runs[currentRun]?.status === 'suspended' && p.predicate.includes('question')
      ? [{model: 'questions', version: 1, attributes: {question: 'exit', run: currentRun, kind: 'choice', prompt: 'Agent exited. Choose Restart, Fork or Leave.', answers: ['restart', 'fork', 'leave'], state: 'pending'}}] : []};
    return {};
  },
};
function call(args: any) {return handler(args, {source: {Plugin: view}});}
function delivery(key: string) {
  if (!token) return {};
  const uid = JSON.stringify([real.getPluginIds().host_id, real.serverSession, token, key]);
  const rows = real.data.fetch({entity: 'delivery', filter: real.data.eq('uid', uid), limit: 1}).records;
  return rows[0] ? JSON.parse(String(rows[0].value)) : {};
}
// The shipped bridge is inserted here by the Rust harness.
function plugin(bridge: any) {
  return definePlugin({
    async load() {
      await real.waitForPermissions(['ReadApplicationState', 'ChangeApplicationState', 'RunCommands', 'WriteToStdin', 'ReadCliPipes', 'PluginData', 'ReadPaneContents', 'MessageAndLaunchOtherPlugins']);
      bridge.load();
    },
    async pipe(message) {
      if (message.name === 'agent_subscribe') {
        real.command('MessageToPlugin', {plugin_url: null, destination_plugin_id: message.source.Plugin, plugin_config: {}, message_name: 'agent_snapshot', message_payload: '{"panes":[],"activity":{}}', message_args: {}, new_plugin_args: null, floating_pane_coordinates: null});
        return;
      }
      if (message.name === 'swamp-pane-result') {
        const reply = JSON.parse(message.payload);
        if (reply.choice === 'started' && reply.pane_id?.Terminal !== source) throw Error('fork did not reuse its source');
        if (reply.choice === 'cancelled' && reply.pane_id !== null) throw Error('cancelled fork returned a pane');
        await bridge.pipe(message);
        if (reply.choice === 'cancelled' && reply.key === 'fork') {
          runs[currentRun].status = 'suspended';
          real.command('MessageToPlugin', {plugin_url: null, destination_plugin_id: view, plugin_config: {}, message_name: 'workflow-changed', message_payload: null, message_args: {}, new_plugin_args: null, floating_pane_coordinates: null});
        }
        return;
      }
      if (message.name !== 'fixture') return bridge.pipe(message);
      if (typeof message.source !== 'object' || !('Cli' in message.source)) return;
      const pipe = message.source.Cli;
      real.command('BlockCliPipeInput', pipe);
      try {
        if (message.payload === 'delay') delayed = true;
        else if (message.payload === 'release') {delayed = false; release!({runId: currentRun});}
        else if (message.payload === 'fork') await call({method: 'pane_action', work: token, key: 'fork', action: 'fork'});
        else if (message.payload === 'restart') real.command('RerunCommandPane', source);
        else if (message.payload === 'close-source') real.command('CloseTerminalPane', source);
        else if (message.payload === 'manual-close-source') {
          const uid = JSON.stringify([real.getPluginIds().host_id, real.serverSession, token, 'view']);
          real.data.apply([real.data.delete('delivery', real.data.eq('uid', uid))]);
          real.command('HidePaneWithId', {Plugin: view});
          real.command('CloseTerminalPane', source);
        }
        else if (message.payload === 'question') {
          runs[currentRun].status = 'suspended';
          real.command('MessageToPlugin', {plugin_url: null, destination_plugin_id: view, plugin_config: {}, message_name: 'workflow-changed', message_payload: null, message_args: {}, new_plugin_args: null, floating_pane_coordinates: null});
        } else if (message.payload === 'agent') {
          worker = (await call({method: 'agent_spawn', work: token, key: 'worker', spec: {kind: 'claude', cwd: '/control', allow_outside_worktree: true}})).pane;
          agentHandler({pane_id: worker, hook: 'SessionStart', raw: JSON.stringify({_zellij_workflow_owner: owners[worker]})});
          await call({method: 'agent_prompt', work: token, key: 'turn', agent: 'worker', prompt: 'work'});
        }
        const fork = message.payload === 'state' ? delivery('fork') : {};
        real.command('CliPipeOutput', [pipe, message.payload === 'status' ? runs[currentRun]?.status ?? 'running'
          : JSON.stringify({closed: delivery('view').closed ?? false, cancelled: cancelled[currentRun] ?? false, waiting: !!release, ...fork})]);
      } finally {real.command('UnblockCliPipeInput', pipe);}
    },
  });
}
