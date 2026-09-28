import yinz from 'yinz:plugin';
let greeting, payload, presses = 0, tabs;
export default {
  defaults: {greeting: 'hello'},
  load(config) { greeting=config.greeting; payload=config.payload; yinz.subscribe(['Key', 'TabUpdate']); yinz.host.setSelectable(true); },
  update(event) {
    if (event.Key) { presses++; return true; }
    if (event.TabUpdate) { tabs=event.TabUpdate.length; return true; }
    return false;
  },
  render(rows, cols) { return `${greeting} from typescript | ${rows}x${cols} | presses ${presses} | tabs ${tabs ?? 'nil'} | payload ${payload ?? 'nil'}`; }
};
