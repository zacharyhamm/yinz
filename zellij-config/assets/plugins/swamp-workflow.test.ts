import {test, loadPlugin, assert, assertEquals} from 'zellij:testing';
test('swamp-workflow starts unattached and supports scroll keys', async () => {
  const p = await loadPlugin('./swamp-workflow.ts', {replies:{SetSelfMouseSelectionSupport:null}});
  assert((await p.render()).includes('No run attached'));
  await p.event({Key:{bare_key:'End',key_modifiers:[]}});
  await p.event({Key:{bare_key:'Home',key_modifiers:[]}});
  assert((await p.render()).includes('No run attached'));
  await p.advance(2000);
  assertEquals(p.commands.filter(c => typeof c === 'object' && c !== null && 'MessageToPlugin' in c).length, 0);
});
test('swamp-workflow attaches, renders a question and shows answer errors', async () => {
  const p = await loadPlugin('./swamp-workflow.ts', {replies:{SetSelfMouseSelectionSupport:null,GetPluginIds:{plugin_id:1},SetPaneSubtitle:null,MessageToPlugin:null,SubscribePaneOutput:null,ShowSelf:null}});
  await p.pipe({name:'show',payload:JSON.stringify({directory:'/fixture',run:'run-1'})});
  async function reply(result: unknown, error?: string) {
    const commands = p.commands as {MessageToPlugin?:{message_payload:string}}[];
    const request = JSON.parse(commands.filter(c => c.MessageToPlugin).at(-1)!.MessageToPlugin!.message_payload);
    await p.pipe({name:'zellij_rpc_reply',source:{Plugin:2},payload:JSON.stringify({version:1,call_id:request.call_id,...(error ? {error} : {result})})});
    return request;
  }
  await reply({run:'run-1',root:'/fixture',history:{status:'suspended'},questions:{results:[{model:'question',version:1,attributes:{run:'run-1',question:'q',state:'pending',kind:'choice',prompt:'Continue the fixture?',answers:['yes','no']}}]}});
  assert((await p.render()).includes('Continue the fixture?'));
  await p.event({Key:{bare_key:'Enter',key_modifiers:[]}});
  const answer = await reply(null,'fixture answer rejected');
  assertEquals(answer.payload.method,'workflow_answer');
  assert((await p.render()).includes('fixture answer rejected'));
});
