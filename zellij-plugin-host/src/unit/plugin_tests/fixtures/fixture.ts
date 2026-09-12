import zellij from 'zellij:plugin';
let greeting, payload, presses = 0, tabs;
export default {
  defaults: {greeting: 'hello'},
  load(config) { greeting=config.greeting; payload=config.payload; zellij.subscribe(['Key', 'TabUpdate']); zellij.host.setSelectable(true); },
  update(event) {
    if (event.Key) { presses++; return true; }
    if (event.TabUpdate) { tabs=event.TabUpdate.length; return true; }
    return false;
  },
  render(rows, cols) { return `${greeting} from typescript | ${rows}x${cols} | presses ${presses} | tabs ${tabs ?? 'nil'} | payload ${payload ?? 'nil'}`; }
};
