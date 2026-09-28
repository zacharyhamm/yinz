import yinz from 'yinz:plugin';
export default { load() { yinz.subscribe(['Key']); }, update(event) { if (event.Key) { while (true) {} } return false; }, render() { return 'runaway plugin, idle'; } };
