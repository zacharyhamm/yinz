// A command sequence in one pane, preserving each step's scrollback.
// This demonstrates pane commands; project automation belongs in a Swamp workflow.
import z, {definePlugin, type PaneId, type CommandToRun, type TimerHandle} from 'zellij:plugin';
let host = '', skill = 'deploy', running = false, cancelled = false, pane: PaneId | null = null;
const lines = ['r: run the sequence, c: cancel it'];
let killTimer: TimerHandle | undefined;
function say(line: string) {lines.push(line); z.requestRender();}
function cancel() {
  if (!running || cancelled) return;
  cancelled = true;
  if (pane) {
    const target = pane;
    z.command('SendSigintToPaneId', target);
    killTimer = z.after(2000, () => {z.command('SendSigkillToPaneId', target);});
  }
}
async function run() {
  if(running) {say('already running'); return;} running = true; cancelled = false;
  const remote = `~/.hocus/skills/${skill}/SKILL.md`;
  const steps: {name: string; command: CommandToRun; timeout: number}[] = host ? [
    {name: 'list', command: {path: 'ssh', args: [host, 'ls ~/.hocus/skills']}, timeout: 30000},
    {name: 'read', command: {path: 'ssh', args: [host, `cat ${remote}`]}, timeout: 30000},
    {name: 'copy', command: {path: 'scp', args: [`${host}:${remote}`, `/tmp/${skill}.SKILL.md`]}, timeout: 60000},
  ] : ['echo listing; ls; sleep 1', 'echo reading; head -3 AGENTS.md; sleep 1', 'echo copying; sleep 1; echo done'].map((script, i) => ({name: ['list', 'read', 'copy'][i], command: {path: 'sh', args: ['-c', script]}, timeout: 30000}));
  const exits: (number | null)[] = [];
  try {
    for (let index = 0; index < steps.length && !cancelled; index++) {
      const step = steps[index], context = {sequence: z.instanceId, step: String(index)};
      // Register before dispatch so a fast child cannot exit before observation starts.
      const exited = z.waitForEvent('CommandPaneExited', ([, , c]) => c.sequence === context.sequence && c.step === context.step);
      let timer: TimerHandle | undefined;
      try {
        if(index === 0) pane = z.command('OpenCommandPaneFloating', [step.command, null, context]);
        else if(pane) z.command('RunCommandInPane', {pane_id: pane, command: step.command, context, keep_scrollback: true});
        if(!pane) throw new Error('No pane was returned');
        timer = z.after(step.timeout, cancel);
        const [, code] = await exited; exits.push(code); say(`step ${index + 1} (${step.name}) exited ${code}`);
        if(code !== 0) break;
      } finally {timer?.cancel(); killTimer?.cancel(); killTimer = undefined; exited.catch(() => {}); exited.cancel();}
    }
    say(`sequence ${cancelled ? 'cancelled' : exits.length === steps.length && exits.every(code => code === 0) ? 'completed' : 'failed'}; exit codes: ${JSON.stringify(exits)}`);
  } catch(error) {say(String(error));} finally {running = false;}
}
z.on('Key', key => {if(typeof key.bare_key === 'object' && 'Char' in key.bare_key) {if(key.bare_key.Char === 'r') void run(); else if(key.bare_key.Char === 'c') {cancel(); say('cancelling...');}} return true;});
export default definePlugin({defaults: {skill: 'deploy'}, load(config) {host = config.host ?? ''; skill = config.skill; if(!/^[\w.-]+$/.test(skill) || host.startsWith('-')) throw new Error('Invalid host or skill'); z.subscribe(['Key']); z.requestPermission(['RunCommands', 'ReadApplicationState', 'ChangeApplicationState']);}, render: () => lines.join('\n')});
