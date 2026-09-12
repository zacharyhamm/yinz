import zellij from 'zellij:plugin';
export default { load() { zellij.subscribe(['Key']); }, update(event) { if (event.Key) { while (true) {} } return false; }, render() { return 'runaway plugin, idle'; } };
