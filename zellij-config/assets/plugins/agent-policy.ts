// Headless hook: log each firing and focus a waiting agent when its toast is clicked.
// Bind with: zellij action set-hook agent_waiting zellij:agent-policy --write
// Builtins have permissions; file copies must request ChangeApplicationState in load().
import zellij, {definePlugin} from 'zellij:plugin';

zellij.on('Hook', ({point, payload}) => {
  zellij.log(`hook ${point} ${JSON.stringify(payload)}`);
  if (point === 'agent_waiting' && 'agent' in payload) {
    const message = `${payload.agent.kind || 'agent'} in pane ${payload.pane_id}: ${payload.agent.detail || 'is waiting for you'}`;
    zellij.command('Notify', [message, {Terminal: payload.pane_id}, null]);
  }
});

export default definePlugin({load() { zellij.subscribe(['Hook']); }});
