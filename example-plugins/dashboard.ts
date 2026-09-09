// Tabs as ribbons, panes as a table, and periodic command output.
import z, {definePlugin, type TabInfo, type PaneInfo, type PaneManifest} from 'zellij:plugin';
let tabs: TabInfo[] = [], manifest: PaneManifest['panes'] = {}, selected = 0, output = '(waiting)', last = 'press a key or click a row', command = 'uptime';
const panes = (): PaneInfo[] => manifest[String(tabs.find(tab => tab.active)?.position)] ?? [];
async function refresh() {
  try {const result = await z.runCommand({command: ['sh', '-c', command], cwd: '.', maxBytesReturned: 4096}); output = result.exitCode === 0 ? z.decodeUtf8(result.stdout).trimEnd() : `exit ${result.exitCode}: ${z.decodeUtf8(result.stderr)}`;}
  catch(error) {output = String(error);} return true;
}
z.on('TabUpdate', value => {tabs = value; return true;});
z.on('PaneUpdate', value => {manifest = value.panes; return true;});
z.on('Key', event => {
  const key = typeof event.bare_key === 'object' && 'Char' in event.bare_key ? event.bare_key.Char : event.bare_key;
  if (key === 'Esc' || key === 'q') z.command('CloseSelf');
  else if (key === 'Down' || key === 'j') selected = Math.min(selected + 1, Math.max(0, panes().length - 1));
  else if (key === 'Up' || key === 'k') selected = Math.max(selected - 1, 0);
  else if (key === 'Enter') {const pane = panes()[selected]; if(pane) {try {z.command(pane.is_plugin ? 'FocusPluginPane' : 'FocusTerminalPane', [pane.id, false, false]); last = `focused ${pane.title}`;} catch(error) {last = String(error);}}}
  else {last = `key: ${JSON.stringify(key)}`; z.log(last);} return true;
});
z.on('Mouse', mouse => {if ('LeftClick' in mouse) {const row = Number(mouse.LeftClick[0]) - 3; if(row >= 0 && row < panes().length) {selected = row; last = `clicked row ${row + 1}`;}} return true;});
export default definePlugin({
  defaults: {command: 'uptime', every: '5'},
  async load(config) {command = config.command; z.subscribe(['TabUpdate', 'PaneUpdate', 'Key', 'Mouse']); await z.waitForPermissions(['ReadApplicationState', 'RunCommands', 'ChangeApplicationState']); await refresh(); z.requestRender(); z.every((Number(config.every) || 5) * 1000, refresh);},
  render(rows, cols) {
    const table = z.ui.table([['pane', 'title', 'kind']]);
    panes().forEach((pane, index) => {const title = z.ui.text(z.text.pad(pane.title, Math.max(10, cols - 20))); if(index === selected) title.selected(); table.row(String(pane.id), title, pane.is_plugin ? z.ui.text('plugin').emphasis(2) : z.ui.text('terminal').dim());});
    return [tabs.map(tab => {const ribbon = z.ui.ribbon(tab.name); return (tab.active ? ribbon.selected() : ribbon).render();}).join(''), '', table.render(), '', z.ui.text(command).emphasis(0).render(), ...output.split('\n').map(line => '  ' + z.text.truncate(line, Math.max(0, cols - 2))), '', z.ui.nestedList([last, {text: 'j/k or click to select, Enter to focus, q to close', indent: 1}]).render()].slice(0, rows).join('\n');
  },
});
