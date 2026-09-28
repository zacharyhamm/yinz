import yinz from 'yinz:plugin';
export default {
  load() { yinz.subscribe(['Key']); yinz.requestPermission(['ChangeApplicationState', 'OpenTerminalsOrPlugins']); },
  update(event) {
    const key=yinz.event(event).key;
    if (key === 't') yinz.command('OpenTerminal', {path:'/tmp/typescript-fixture-cwd', line_number:null, cwd:null});
    else if (key === 'Esc') yinz.host.closeSelf();
    return !!event.Key;
  },
  render() { return 'typescript fixture with permissions'; }
};
