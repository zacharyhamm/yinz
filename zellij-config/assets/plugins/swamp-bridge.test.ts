import {test, loadPlugin, assert} from 'zellij:testing';
test('swamp-bridge rejects unauthenticated callers before dispatch', async () => {
  const p = await loadPlugin('./swamp-bridge.ts', {replies:{GetPluginIds:{host_id:'test',plugin_id:1},DataDefineEntity:null,DataFetch:{columns:[],rows:[],truncated:false,next_cursor:null},MessageToPlugin:null,GetPaneInfo:{plugin_url:'file:untrusted.ts'}}});
  await p.pipe({name:'zellij_rpc_call',source:{Plugin:2},payload:JSON.stringify({version:1,call_id:'a',name:'swamp',payload:{method:'workflow_run',work:'x',key:'one'}})});
  assert(JSON.stringify(p.commands).includes('Only builtin workflow views'));
  assert(!JSON.stringify(p.commands).includes('SwampRequest'));
});
import {assertEquals} from 'zellij:testing';
import {storeReplies} from './testing-fixtures/store.ts';
test('swamp-bridge correlates persisted attachments and rejects conflicting duplicates', async () => {
  const store = storeReplies();
  const p = await loadPlugin('./swamp-bridge.ts', {replies:{...store.replies,GetPluginIds:{host_id:'test',plugin_id:1},MessageToPlugin:null,GetPaneInfo:{plugin_url:'zellij:swamp-workflow'}}});
  let id = 0;
  async function request(payload: unknown) {
    const call_id = String(++id);
    await p.pipe({name:'zellij_rpc_call',source:{Plugin:2},payload:JSON.stringify({version:1,call_id,name:'swamp',payload})});
    const commands = p.commands as {MessageToPlugin?:{message_name:string;message_payload:string}}[];
    const reply = commands.filter(c => c.MessageToPlugin?.message_name === 'zellij_rpc_reply').at(-1)!.MessageToPlugin!;
    const result = JSON.parse(reply.message_payload);
    assertEquals(result.call_id, call_id);
    return result;
  }
  const attach = {method:'workflow_attach',run:'run-1',work:'work-1',key:'attach',workflow:'fixture',directory:'/fixture'};
  assertEquals((await request(attach)).result.outcome,'completed');
  assertEquals(store.rows.size,2);
  const saved = JSON.stringify([...store.rows]);
  assertEquals((await request(attach)).result.outcome,'completed');
  assertEquals(JSON.stringify([...store.rows]),saved);
  assert((await request({...attach,work:'other'})).error.includes('Run belongs to a different launch'));
  assert((await request({method:'lookup',work:'bad key',key:'k'})).error.includes('Invalid correlation key'));
});
