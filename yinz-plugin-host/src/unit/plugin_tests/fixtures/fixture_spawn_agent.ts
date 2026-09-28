import yinz from 'yinz:plugin';
export default {
  load() { yinz.subscribe(['Key']); yinz.requestPermission(['RunCommands']); },
  update(event) {
    const key=yinz.event(event).key;
    if (key === 's' || key === 'h') {
      yinz.agent.spawn({kind:'claude', prompt:'fix it', cwd:'/host', allow_outside_worktree:true,
        ...(key === 's' ? {hooks:false} : {name:'fixer'})});
      return true;
    }
    return false;
  },
  render() { return 'spawn fixture'; }
};
