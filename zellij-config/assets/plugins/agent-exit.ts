// Attach a project-owned Swamp workflow. Retain the source until Leave.
import z, {definePlugin, type Event} from 'zellij:plugin';
import {view} from 'zellij:workflow-view';
let status = 'Agent exited.', busy = false, workflow = false;
let pane: number | undefined, directory: string | undefined, launchToken: string | undefined;
async function launch(config: Record<string, string>) {
  const ownPane = {Plugin: z.getPluginIds().plugin_id};
  const visible = () => {
    try { const info = z.command('GetPaneInfo', ownPane); return !!info && !info.is_suppressed; }
    catch { return false; }
  };
  // Subscribe before checking: load can precede placement of this hook.
  const placed = z.waitForEvent('PaneUpdate', visible);
  if (!visible()) await placed;
  else { placed.catch(() => {}); placed.cancel(); }
  const token = config.agent_exit_token;
  if (!token) throw new Error('Missing host exit correlation token');
  view.watch(token);
  try {
    await z.rpc.call({pluginUrl: 'zellij:swamp-bridge', name: 'swamp', timeoutMs: 30000,
      payload: {method: 'workflow_run', work: token, key: 'launch', directory: directory!,
        inputs: {sourcePane: pane ?? null, kind: config.agent_kind ?? '', firecracker: config.agent_firecracker_vm ?? ''}}});
  } catch (error) { view.status = String(error); } // A failed reply can follow dispatch; keep reconciling.
  view.attach({directory: directory!, launch: token}); busy = false; z.requestRender();
}
export default definePlugin({
  load(config) {
    launchToken = config.agent_exit_token;
    pane = config.agent_pane_id == null ? undefined : Number(config.agent_pane_id);
    directory = config.agent_firecracker_worktree ?? config.agent_cwd;
    z.subscribe(['Key']);
    if (config.swamp_owner || config.agent_workflow_owner) { status = 'Managed helper exited. Its owning operation receives the exit event.'; return; }
    if (!directory || config.agent_target === 'ssh') { status = 'No local control checkout is available. Enter: restart. Esc: return to scrollback.'; return; }
    try {
      const snapshot = z.projectConfig.read(directory);
      if (snapshot.error || !snapshot.config.workflow) throw new Error(snapshot.error || 'No exit workflow configured.');
    } catch (error) { status = `${error}\nUse Hocus /project-setup to configure .zellij/.swamp-workflows.kdl.`; return; }
    view.active = true;
    view.load({onRestart() {
      if (!busy && !view.active && !view.submitting && !view.pending.length && pane != null) {
        busy = true;
        void z.rpc.call({pluginUrl: 'zellij:swamp-bridge', name: 'swamp',
          payload: {method: 'pane_action', work: launchToken!, key: 'restart', action: 'restart'}})
          .catch(error => { view.status = String(error); z.requestRender(); }).finally(() => { busy = false; });
      }
    }});
    view.status = 'Starting workflow...'; workflow = true; busy = true;
    void launch(config).catch(error => { view.status = String(error); z.requestRender(); });
  },
  update(event: Event) {
    if (workflow) {
      if (typeof event === 'object' && 'Key' in event && !view.pending.length && event.Key.bare_key === 'Delete') {
        if (!busy && !view.active && !view.submitting && pane != null) { z.command('CloseTerminalPane', pane); z.command('CloseSelf'); }
        return true;
      }
      return view.update(event);
    }
    if (typeof event !== 'object' || !('Key' in event)) return false;
    if (event.Key.bare_key === 'Esc') z.command('CloseSelf');
    else if (event.Key.bare_key === 'Enter' && !busy) { if (pane != null) z.command('RerunCommandPane', pane); z.command('CloseSelf'); }
    else if (event.Key.bare_key === 'Delete' && !busy) { if (pane != null) z.command('CloseTerminalPane', pane); z.command('CloseSelf'); }
    return true;
  },
  pipe: message => view.pipe(message),
  render(rows, cols) {
    if (workflow) return view.render(Math.max(0, rows - 1), cols) + z.ui.text(z.text.truncate(busy || view.active || view.submitting
      ? 'Enter: activate button. Leave: close held pane. Cancel run: explicit cancellation.'
      : 'Enter: activate button  Delete: leave and close held pane', cols)).dim().render({x: 0, y: Math.max(0, rows - 1), width: cols, height: 1});
    return [...status.split('\n').filter(Boolean), `Directory: ${directory ?? 'unknown'}`, 'Enter: restart agent  Esc: scrollback  Delete: close held pane']
      .slice(0, rows).map((line, y) => z.ui.text(z.text.truncate(z.text.sanitize(line), cols)).render({x: 0, y, width: cols, height: 1})).join('');
  },
});
