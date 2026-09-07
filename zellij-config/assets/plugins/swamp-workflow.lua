-- A detachable view of authoritative Swamp history. The runner lives in another pane.
local z = require('zellij')
local M = {status='No run attached', scroll=0, pending={}, selected=1}
local form
local function text(s) return z.text.sanitize(tostring(s or '')) end
function M.refresh()
  if not M.attachment or M.busy then return end
  M.busy = true
  z.task(function()
    local ok,err=pcall(function()
    local request = {method='workflow_show',directory=M.attachment.directory,run=M.attachment.run,launch=M.attachment.launch}
    local reply = z.await(z.rpc.call {plugin_url='zellij:swamp-bridge',name='swamp',payload=request,timeout=30})
    if reply.error then M.status = reply.error
    else
      M.snapshot = reply.result
      M.active = M.snapshot.active or M.snapshot.history.status == 'running' or M.snapshot.history.status == 'suspended'
      for _,child in ipairs(M.snapshot.children or {}) do
        M.active = M.active or child.status == 'running' or child.status == 'suspended'
      end
      M.attachment.run = M.snapshot.run
      M.status = 'Run ' .. (M.snapshot.run or M.snapshot.launch) .. ': ' .. M.snapshot.history.status
      M.pending = {}
      local latest={}
      for _,entry in ipairs((M.snapshot.questions or {}).results or {}) do
        local key=entry.model..'/'..entry.attributes.question
        if not latest[key] or entry.version>latest[key].version then latest[key]=entry end
      end
      for _, entry in pairs(latest) do
        if entry.attributes.state == 'pending' then M.pending[#M.pending+1] = entry end
      end
      table.sort(M.pending,function(a,b) return a.model..a.attributes.question < b.model..b.attributes.question end)
      M.selected=math.min(M.selected,math.max(1,#M.pending))
    end
    end)
    if not ok then M.status=tostring(err) end
    M.busy=false
    z.request_render()
  end)
end
function M.focused_control() return form:focused_id() end
function M.attach(attachment)
  M.attachment=attachment
  M.active=true
  M.refresh()
end
function M.load()
  z.subscribe {'key','mouse','pasted_text'}
  form = z.ui.form({{kind='text_input',id='answer',label='Answer',value=''}, {kind='button',id='submit',label='Submit answer'}, {kind='button',id='next',label='Next question'}, {kind='button',id='cancel_run',label='Cancel run'}, {kind='button',id='detach',label='Detach'}}, {enter_submits=true})
  z.every(2,M.refresh)
end
function M.pipe(message)
  if message.name ~= 'show' or not message.payload then return end
  M.attach(z.json.decode(message.payload))
  z.show_self(true)
  local pipe = type(message.source)=='table' and message.source.Cli
  if pipe then z.cli_pipe_output(pipe,z.json.encode({attached=true})); z.unblock_cli_pipe_input(pipe) end
end
function M.update(event)
  if event.name=='key' and event.key=='PageUp' then M.scroll=math.max(0,M.scroll-10);return true end
  if event.name=='key' and event.key=='PageDown' then M.scroll=M.scroll+10;return true end
  local action
  if event.name=='key' then action=form:handle_key(event)
  elseif event.name=='mouse' then action=form:handle_mouse(event)
  elseif event.name=='pasted_text' then action=form:insert_pasted_text(event.value) end
  if not action then return true end
  if action.kind=='cancel' or action.id=='detach' then z.close_self();return true end
  if action.id=='next' then M.selected=M.selected % math.max(1,#M.pending)+1;return true end
  if action.id=='submit' or action.kind=='submit' then
    local entry = M.pending[M.selected]
    if not entry or M.busy then return true end
    local q, answer = entry.attributes, form:value('answer')
    M.busy=true
    z.task(function()
      local ok,result=pcall(function() return z.await(z.async.run_command {cwd=M.snapshot.root,context={swamp_owner='view'},command={z.server_binary or 'zellij','workflow','answer',q.run,entry.model,q.question,answer},max_bytes_returned=8192}) end)
      M.status=ok and (result.exit_code==0 and 'Answer recorded' or result.stderr) or tostring(result)
      M.busy=false
      M.refresh()
    end)
  elseif action.id=='cancel_run' and M.snapshot and M.attachment.run then
    M.busy=true
    z.task(function()
      local ok,result=pcall(function() return z.await(z.async.run_command {cwd=M.snapshot.root,context={swamp_owner='view'},command={z.server_binary or 'zellij','workflow','cancel',M.attachment.run},max_bytes_returned=8192}) end)
      M.status=ok and (result.exit_code==0 and 'Cancellation requested; resources preserved' or result.stderr) or tostring(result)
      M.busy=false
      M.refresh()
    end)
  end
  return true
end
function M.render(rows,cols)
  local lines={text(M.status), 'Closing this view detaches it. PgUp/PgDn: history.'}
  local entry=M.pending[M.selected]
  if entry then
    local q=entry.attributes
    lines[#lines+1]=text(q.kind .. ': ' .. q.prompt)
    lines[#lines+1]=text(table.concat(q.answers,' | '))
  end
  if M.snapshot then
    local histories={M.snapshot.history}
    for _,child in ipairs(M.snapshot.children or {}) do histories[#histories+1]=child end
    for _,history in ipairs(histories) do
    lines[#lines+1]=text(history.workflowName).." / "..text(history.id)..": "..text(history.status)
    for _,job in ipairs(history.jobs or {}) do
      for _,step in ipairs(job.steps or {}) do lines[#lines+1]=text(job.name .. '/' .. step.name .. ': ' .. step.status) end
    end
    end
    for _,entry in ipairs((M.snapshot.outputs or {}).results or {}) do
      lines[#lines+1]=text(entry.model..'/'..entry.name..' v'..entry.version)
      local attrs=entry.attributes or {}
      local review=attrs.review or attrs
      for _,finding in ipairs(review.findings or {}) do lines[#lines+1]=text(finding.severity..' '..finding.file..': '..finding.message) end
      if attrs.pane then lines[#lines+1]='Pane '..attrs.pane..' / '..text(attrs.log)..' / '..text(attrs.outcome) end
      if attrs.output then
        if attrs.outputTruncated then lines[#lines+1]='Retained command scrollback (may omit older output):' end
        for line in attrs.output:gmatch('[^\n]+') do lines[#lines+1]=text(line) end
      end
      if attrs.logError then lines[#lines+1]='Log unavailable: '..text(attrs.logError) end
      if attrs.test then lines[#lines+1]='Gate '..text(attrs.test.outcome)..' / exit '..text(attrs.test.exitCode)..' / log '..text(attrs.test.log) end
      local evidence=attrs.evidence or (attrs.source and attrs)
      if evidence then lines[#lines+1]='Source '..text(evidence.source.commit)..' / target '..text(evidence.target.commit) end
    end
    lines[#lines+1]='Workflow log (last 100 lines):'
    for line in (M.snapshot.logs or ''):gmatch('[^\n]+') do lines[#lines+1]=text(line) end
  end
  local wrapped={}
  for _,line in ipairs(lines) do
    for _,part in ipairs(z.text.wrap(line,math.max(1,cols))) do wrapped[#wrapped+1]=part end
  end
  lines=wrapped
  local page=math.max(0,rows-6)
  M.scroll=math.min(M.scroll,math.max(0,#lines-page))
  local visible={}
  for i=M.scroll+1,math.min(#lines,M.scroll+page) do visible[#visible+1]=lines[i] end
  local frame={}
  for i,line in ipairs(visible) do frame[#frame+1]=z.ui.text(z.text.truncate(line,cols)):render{x=0,y=i-1,width=cols,height=1} end
  return table.concat(frame) .. form:render{x=0,y=page,width=cols,height=math.min(6,rows)}
end
return M
