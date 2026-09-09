// Typed raw commands: queries, named fields, tuples, enums, bytes, and permission errors.
import z, {definePlugin, type PaneId, type TabInfo} from 'zellij:plugin';
let command = 'date', last = 'press a key', tabs: TabInfo[] = [], panes = 0, opened: PaneId | null = null;
const focused = () => tabs.find(tab => tab.active);
const actions: Record<string, () => unknown> = {
  i: () => z.getPluginIds(), v: () => z.command('GetZellijVersion'), p: () => z.command('GetFocusedPaneInfo'),
  d: () => `${z.command('DumpLayout', 'default').length} bytes of kdl`,
  t: () => z.command('NewTab', {name: 'playground', cwd: null}),
  r: () => focused() ? z.command('RenameTab', [focused()!.position, 'renamed-by-typescript']) : 'no focused tab',
  n: () => opened = z.command('OpenTerminal', {path: '/tmp'}),
  c: () => opened = z.command('OpenCommandPane', [{path: 'sh', args: ['-c', command + '; sleep 30']}, {}]),
  w: () => opened ? z.command('WriteToPaneId', [z.encodeUtf8('echo written by typescript\r'), opened]) : 'press c first',
  '+': () => z.command('Resize', 'Increase'), '-': () => z.command('Resize', 'Decrease'),
  x: () => z.command('CopyToClipboard', 'this will not be copied'), // Permission deliberately omitted.
  q: () => z.command('CloseSelf'),
};
z.on('TabUpdate', value => {tabs = value; return true;});
z.on('PaneUpdate', value => {panes = Object.values(value.panes).flat().length; return true;});
z.on('PermissionRequestResult', result => {last = `permissions: ${result}`; return true;});
z.on('Key', event => {
  const key = event.bare_key === 'Esc' ? 'q' : typeof event.bare_key === 'object' && 'Char' in event.bare_key ? event.bare_key.Char : '';
  if (!actions[key]) return false;
  try {last = `${key} -> ${JSON.stringify(actions[key]()) ?? 'ok'}`;} catch(error) {last = `${key} failed: ${error}`;} return true;
});
export default definePlugin({
  defaults: {command: 'date'},
  load(config) {command = config.command; z.subscribe(['Key', 'TabUpdate', 'PaneUpdate', 'PermissionRequestResult']); z.requestPermission(['ReadApplicationState', 'ChangeApplicationState', 'OpenTerminalsOrPlugins', 'RunCommands', 'WriteToStdin']);},
  render(rows, cols) {return ['zellij TypeScript playground', `${tabs.length} tabs, ${panes} panes, focused: ${focused()?.name ?? '-'}`, '', last, '', 'queries', 'i plugin ids  v version  p focused pane  d layout', 'commands', 't new tab  r rename tab  n terminal in /tmp', `c run ${command}  w write to pane  +/- resize`, 'x command without permission', `q close (${rows}x${cols})`].join('\n');},
});
