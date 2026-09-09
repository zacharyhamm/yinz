import real, {definePlugin} from 'zellij:plugin';
let handler: any, token: string, worker: number, agentHandler: any;
let status = 'running';
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
    let argv = opts.command, stdout = '';
    if(argv[0] === 'bash') {if(argv[5] !== 'workflow-history') throw Error('bad history wrapper'); argv = argv.slice(6);}
    if(argv[0] === 'git') stdout = '/host\n';
    else if(argv[1] === 'pty-host') stdout = JSON.stringify(hosts);
    else if(argv[1] === 'workflow' && argv[2] === 'answer') {
      if(JSON.stringify(argv.slice(3)) !== JSON.stringify(['run', 'questions', 'exit', 'leave'])) throw Error('bad answer');
      status = 'completed';
    } else if(argv[1] === 'workflow' && argv[2] === 'cancel') status = 'cancelled';
    else if(argv[1] === 'workflow' && argv[2] === 'history') {
      if(argv[3] === 'search') stdout = JSON.stringify({results: [{runId: 'run', workflowName: 'fixture', inputs: {session: real.serverSession, launch: token}}]});
      else if(argv[3] === 'logs') stdout = Array.from({length: 100}, (_, i) => `LOG ${String(i + 1).padStart(3, '0')} selectable output`).join('\n');
      else if(argv[3] === 'get') stdout = JSON.stringify({id: 'run', workflowName: 'fixture', status, jobs: [], inputs: {session: real.serverSession, launch: token}});
    } else if(argv[1] === 'data') {
      let results: any[] = [];
      if(argv[3].includes('question') && status === 'suspended') {
        if(argv[3] !== 'specName == "question" && attributes.run in ["run"]') throw Error('bad question predicate');
        results = [{model: 'questions', version: 2, attributes: {question: 'exit', run: 'run', kind: 'choice', prompt: 'Agent exited. Choose Restart, Duplicate, Fork, Begin review, or Leave.', answers: ['restart', 'duplicate', 'fork', 'begin-review', 'leave'], state: 'pending'}},
          {model: 'questions', version: 1, attributes: {question: 'exit', run: 'run', kind: 'choice', prompt: 'Stale question', answers: ['old'], state: 'pending'}}];
      }
      stdout = JSON.stringify({results});
    }
    return {exitCode: 0, stdout: real.encodeUtf8(stdout), stderr: new Uint8Array()};
  },
};
function call(args: any) {return handler(args, {source: {Plugin: real.getPluginIds().plugin_id}});}
function emit(hook: string, prompt?: string) {agentHandler({pane_id: worker, hook, prompt, session_id: 'agent-session', raw: JSON.stringify({_zellij_workflow_owner: owners[worker], turn_id: 'turn'})});}
