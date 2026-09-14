import real, {definePlugin} from 'zellij:plugin';
let token: string, source: number, view: number, worker: number, release: ((value: any) => void) | undefined;
let status = 'running', delayed = false, handler: any, agentHandler: any;
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
    if (op === 'workflow.run') return delayed ? new Promise(resolve => {release = resolve;}) : {runId: 'run'};
    if (op === 'workflow.cancel') {cancelled[p.runId] = true; status = 'cancelled';}
    if (op === 'workflow.history.search') return {results: delayed ? [] : [{runId: 'run', workflowName: 'fixture', inputs: {session: real.serverSession, launch: token}}]};
    if (op === 'workflow.history.get') return {id: 'run', workflowName: 'fixture', status, jobs: [], inputs: {session: real.serverSession, launch: token}};
    if (op === 'workflow.history.logs') return {log: {lines: ['fixture workflow']}};
    if (op === 'data.query') return {results: status === 'suspended' && p.predicate.includes('question')
      ? [{model: 'questions', version: 1, attributes: {question: 'exit', run: 'run', kind: 'choice', prompt: 'Agent exited. Choose', answers: ['restart', 'fork', 'leave'], state: 'pending'}}] : []};
    return {};
  },
};
function call(args: any) {return handler(args, {source: {Plugin: view}});}
function closed() {
  if (!token) return false;
  const uid = JSON.stringify([real.getPluginIds().host_id, real.serverSession, token, 'view']);
  const rows = real.data.fetch({entity: 'delivery', filter: real.data.eq('uid', uid), limit: 1}).records;
  return rows[0] ? JSON.parse(String(rows[0].value)).closed ?? false : false;
}
// The shipped bridge is inserted here by the Rust harness.
function plugin(bridge: any) {
  return definePlugin({
    async load() {
      await real.waitForPermissions(['ReadApplicationState', 'ChangeApplicationState', 'RunCommands', 'WriteToStdin', 'ReadCliPipes', 'PluginData', 'ReadPaneContents', 'MessageAndLaunchOtherPlugins']);
      bridge.load();
    },
    async pipe(message) {
      if (message.name !== 'fixture') return bridge.pipe(message);
      if (typeof message.source !== 'object' || !('Cli' in message.source)) return;
      const pipe = message.source.Cli;
      real.command('BlockCliPipeInput', pipe);
      try {
        if (message.payload === 'delay') delayed = true;
        else if (message.payload === 'release') {delayed = false; release!({runId: 'run'});}
        else if (message.payload === 'question') {
          status = 'suspended';
          real.command('MessageToPlugin', {plugin_url: null, destination_plugin_id: view, plugin_config: {}, message_name: 'workflow-changed', message_payload: null, message_args: {}, new_plugin_args: null, floating_pane_coordinates: null});
        } else if (message.payload === 'agent') {
          worker = (await call({method: 'agent_spawn', work: token, key: 'worker', spec: {kind: 'claude', cwd: '/control', allow_outside_worktree: true}})).pane;
          agentHandler({pane_id: worker, hook: 'SessionStart', raw: JSON.stringify({_zellij_workflow_owner: owners[worker]})});
          await call({method: 'agent_prompt', work: token, key: 'turn', agent: 'worker', prompt: 'work'});
        }
        real.command('CliPipeOutput', [pipe, JSON.stringify({closed: closed(), cancelled: cancelled.run ?? false, waiting: !!release})]);
      } finally {real.command('UnblockCliPipeInput', pipe);}
    },
  });
}
