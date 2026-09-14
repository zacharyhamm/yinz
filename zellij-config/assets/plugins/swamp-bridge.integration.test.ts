import {test, session, assert} from 'zellij:testing';
test('swamp-bridge initializes its persistent headless delivery store', async () => {
  session.start({backgroundPlugins:['zellij:swamp-bridge']});
  await session.waitForText('zellij-test>');
  await session.waitFor('delivery schema created', () => session.catalog().some(entity => entity.def.name === 'delivery'));
  assert(session.data('zellij:swamp-bridge', 'delivery').rows.length === 0);
});

import {assertEquals} from 'zellij:testing';
import {failedWorkflow, deliveryValues} from './testing-fixtures/workflow.ts';
test('swamp-bridge retains correlated failure data across headless reload', {grants:['RunCommands']}, async () => {
  const directory = failedWorkflow();
  session.start({backgroundPlugins:['zellij:swamp-bridge']});
  session.loadPlugin('zellij:agent-exit', {configuration:{agent_cwd:directory,agent_exit_token:'persisted-launch'}});
  await session.waitFor('failed delivery', () => deliveryValues().some(value => value.outcome === 'failed'));
  const failed = deliveryValues().find(value => value.outcome === 'failed');
  assertEquals(JSON.parse(failed.request).work, 'persisted-launch');
  session.reload('zellij:swamp-bridge');
  session.loadPlugin('zellij:swamp-workflow');
  await session.waitForText('No run attached');
  assertEquals(JSON.parse(session.pipe('show', JSON.stringify({directory,launch:'persisted-launch'}), {plugin:'zellij:swamp-workflow'})), {attached:true});
  await session.waitForText('persisted-launch');
  await session.waitForText('failed');
  assertEquals(deliveryValues().find(value => value.outcome === 'failed'), failed);
});
