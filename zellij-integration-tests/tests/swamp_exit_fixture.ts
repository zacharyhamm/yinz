import real, {definePlugin} from 'zellij:plugin';
let handler: any, token: string, worker: number, agentHandler: any;
let status = 'running', step = 'prepare', refreshFailed = false;
const owners: Record<number, string> = {}, hosts: any[] = [];
const z: any = {...real,
  command(name: any, args: any) {
    const result = (real.command as any)(name, args);
    if (name === 'GetPaneInfo' && args.Plugin === real.getPluginIds().plugin_id && result) result.plugin_url = 'zellij:agent-exit';
    if (name === 'OpenCommandPaneBackground' && result) hosts.push({terminal_id: result.Terminal, attached: true, session_name: real.serverSession, host_id: `host-${result.Terminal}`, child_pid: 100 + result.Terminal});
    return result;
  },
  projectConfig: {read: () => ({root: '/host', config: {workflow: 'fixture'}})},
  workflowRemotePanes: () => [],
  rpc: {handle(_name: string, callback: any) {handler = callback;}, call(request: any) {return call(request.payload);}},
  on(name: any, callback: any) {if(name === 'AgentEvent') agentHandler = callback; return real.on(name, callback);},
  agent: {...real.agent, spawnDetails(opts: any) {
    const agent = real.agent.spawnDetails(opts), id = (agent.pane_id as {Terminal: number}).Terminal;
    owners[id] = opts.context.swamp_owner;
    hosts.push({terminal_id: id, attached: true, session_name: real.serverSession, host_id: `host-${id}`, child_pid: 100 + id});
    return agent;
  }},
  async runCommand(opts: any) {
    const stdout = opts.command[0] === 'git' ? '/host\n' : JSON.stringify(hosts);
    return {exitCode: 0, stdout: real.encodeUtf8(stdout), stderr: new Uint8Array()};
  },
  async swampRequest(opts: any) {
    const op = opts.operation, p = opts.payload;
    if (op === 'workflow.get') return {name: 'fixture'};
    if (op === 'workflow.validate') return {passed: true};
    if (op === 'workflow.run') return {runId: 'run'};
    if (op === 'model.method.run') {
      if (p.inputs.run !== 'run' || p.modelIdOrName !== 'questions' || p.inputs.question !== 'exit' || p.inputs.answer !== 'leave') throw Error('bad answer');
      status = 'completed';
      if ((await call({method: 'pane_action', work: token, key: 'leave', action: 'leave'})).outcome !== 'completed') throw Error('leave failed');
    } else if (op === 'workflow.cancel') status = 'cancelled';
    else if (op === 'workflow.history.search') return {results: [{runId: 'run', workflowName: 'fixture', inputs: {session: real.serverSession, launch: token}}]};
    else if (op === 'workflow.history.logs') return {type: 'log', log: {lines: Array.from({length: 100}, (_, i) => `LOG ${String(i + 1).padStart(3, '0')} selectable output`)}};
    else if (op === 'workflow.history.get') {
      if (refreshFailed) throw Error('fixture history unavailable');
      return {id: 'run', workflowName: 'fixture', status, jobs: [{name: 'workflow', steps: [{name: step, status: status === 'suspended' ? 'waiting_approval' : status}]}],
        inputs: {session: real.serverSession, launch: token}};
    }
    else if (op === 'data.query') {
      let results: any[] = [];
      if (p.predicate.includes('question') && status === 'suspended') {
        if (p.predicate !== 'specName == "question" && attributes.run in ["run"]') throw Error('bad predicate');
        results = [{model: 'questions', version: 2, attributes: {question: 'exit', run: 'run', kind: 'choice', prompt: 'Agent exited. Choose Restart, Fork, Begin review, or Leave.', answers: ['restart', 'fork', 'begin-review', 'leave'], state: 'pending'}},
          {model: 'questions', version: 1, attributes: {question: 'exit', run: 'run', kind: 'choice', prompt: 'Stale question', answers: ['old'], state: 'pending'}}];
      }
      return {results: [], projected: {shape: 'map', rows: results}};
    }
    return {};
  },
};
function call(args: any) {return handler(args, {source: {Plugin: real.getPluginIds().plugin_id}});}
function emit(hook: string, prompt?: string) {agentHandler({pane_id: worker, hook, prompt, session_id: 'agent-session', raw: JSON.stringify({_zellij_workflow_owner: owners[worker], turn_id: 'turn'})});}
