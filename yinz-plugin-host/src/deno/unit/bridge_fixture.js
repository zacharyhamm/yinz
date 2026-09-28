globalThis.rows=new Map(); globalThis.owners={}; globalThis.panes={}; globalThis.handlers={};
globalThis.spawned=0; globalThis.sent=0; globalThis.opened=0; globalThis.nextPane=6;
globalThis.handler=null; globalThis.conflictNext=false; globalThis.failure=null;
const clone=v=>JSON.parse(JSON.stringify(v));
globalThis.mock={...yinz, serverSession:'session',serverIncarnation:'incarnation',serverBinary:'yinz',instanceId:'instance',
  subscribe(){}, getPluginIds:()=>({host_id:'host',plugin_id:1}), on(name,callback){handlers[name]=callback;},
  workflowRemotePanes:()=>[], sleep:()=>new Promise(()=>{}),
  data:{...yinz.data,
    define(){}, eq:(_column,uid)=>uid,
    fetch(q){
      if(q.filter) return {records:rows.has(q.filter)?[clone(rows.get(q.filter))]:[],raw:{next_cursor:null}};
      const entries=[...rows.entries()], start=Number(q.afterId??0);
      return {records:entries.slice(start,start+3).map(([uid,row])=>({uid,...clone(row)})),raw:{next_cursor:start+3<entries.length?start+3:null}};
    },
    put(_entity,value,options){return {...value,...options}},
    apply(changes){
      if(conflictNext){conflictNext=false; throw Error('Conflict');}
      for(const change of changes) if(change.expectRev!==(rows.get(change.uid)?.rev??0)) throw Error('Conflict');
      for(const change of changes) rows.set(change.uid,{value:change.value,rev:change.expectRev+1});
    },isConflict:e=>String(e).includes('Conflict')},
  rpc:{handle(_name,callback){handler=callback}},
  command(name,args){
    if(name==='GetPaneInfo') return 'Plugin' in args ? {plugin_url:args.Plugin===99?'file:impostor.ts':'yinz:swamp-workflow'} : {target:'Local'};
    if(name==='SetPaneSubtitle') return;
    if(name==='OpenCommandPaneBackground') {opened++;const pane=++nextPane;owners[pane]=args[1].swamp_owner;panes[pane]=true;return {Terminal:pane};}
    throw Error('unexpected command: '+name);
  },
  async runCommand(opts){
    check(opts.command[1]==='pty-host');
    return {exitCode:0,stdout:yinz.encodeUtf8(JSON.stringify(Object.keys(panes).map(p=>({terminal_id:Number(p),host_id:'supervisor-'+p,child_pid:100+Number(p),session_name:'session',attached:true})))),stderr:new Uint8Array()};
  },
  agent:{
    spawnDetails(opts){spawned++;const pane=++nextPane;owners[pane]=opts.context.swamp_owner;panes[pane]=true;event(pane,'SessionStart');return {pane_id:{Terminal:pane},branch:'topic',cwd:'/work/topic'}},
    send(pane,prompt){sent++;if(prompt==='fast'){event(pane.Terminal,'UserPromptSubmit',prompt);event(pane.Terminal,'Stop')}}
  }
};
globalThis.event=(pane,hook,prompt,hash,turn,session=null)=>handlers.AgentEvent({pane_id:pane,hook,prompt,session_id:session,last_message:'done',
  raw:JSON.stringify({_yinz_workflow_owner:owners[pane],prompt_sha256:hash,turn_id:turn})});
