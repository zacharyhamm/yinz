import zellij from 'zellij:plugin';
export default {
  load() { zellij.subscribe(['Key']); zellij.requestPermission(['RunCommands']); },
  update(event) {
    const key=zellij.event(event).key;
    if (key === 's' || key === 'h') {
      zellij.agent.spawn({kind:'claude', prompt:'fix it', cwd:'/host', allow_outside_worktree:true,
        ...(key === 's' ? {hooks:false} : {name:'fixer'})});
      return true;
    }
    return false;
  },
  render() { return 'spawn fixture'; }
};
