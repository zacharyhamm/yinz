import {test, session, assert} from 'zellij:testing';
test('agent-policy receives the real session-start hook headlessly', async () => {
  session.start({hooks:{session_start:'zellij:agent-policy'}});
  await session.waitFor('session hook in plugin log', () => session.logs.includes('hook session_start'));
  assert(session.logs.includes('session_start'));
});
