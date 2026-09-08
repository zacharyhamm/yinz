-- A detachable view of authoritative Swamp history. The runner lives in another pane.
local z = require('zellij')
local M = {status='No run attached', scroll=0, pending={}, selected=1}
local form
local on_restart
local function text(s) return z.text.sanitize(tostring(s or '')) end
local function quote(s) return "'"..s:gsub("'","'\"'\"'").."'" end
local form_key, question_key, form_height, form_has_input
local content_height=0
local answered={}
local generation=0
local function identity(entry)
  return entry and z.json.encode{entry.attributes.run,entry.model,entry.attributes.question}
end
local function controls()
  local entry=M.pending[M.selected]
  local q=entry and entry.attributes
  local key=identity(entry)
  local signature=z.json.encode{key or '',q and q.kind or '',q and q.answers or {},#M.pending>1,M.active or false}
  if signature==form_key then return end
  local focus=form and key==question_key and form:focused_id()
  local has_input=q and #(q.answers or {})==0
  local value=form and key==question_key and form_has_input and has_input and form:value('answer') or ''
  local fields={}
  if q then
    if not has_input then
      for i,answer in ipairs(q.answers) do
        local label=answer:gsub('-', ' '):gsub('^%l',string.upper)
        fields[#fields+1]={kind='button',id='choice_'..i,label=text(label)}
      end
    else
      fields[#fields+1]={kind='text_input',id='answer',label='Answer',value=value}
      fields[#fields+1]={kind='button',id='submit',label='Submit'}
    end
  end
  if #M.pending>1 then fields[#fields+1]={kind='button',id='next',label='Next question'} end
  if M.active then fields[#fields+1]={kind='button',id='cancel_run',label='Cancel run'} end
  if on_restart and not M.active and #M.pending==0 then
    fields[#fields+1]={kind='button',id='restart',label='Restart agent'}
  end
  fields[#fields+1]={kind='button',id='detach',label='Detach'}
  form=z.ui.form(fields)
  if focus then
    for _,field in ipairs(fields) do if field.id==focus then form:focus(focus) end end
  end
  form_key,question_key=signature,key
  form_has_input=has_input
  form_height=has_input and 2 or 1
end
local function repaint()
  if M.selecting then M.dirty=true else z.request_render() end
end
function M.refresh()
  if not M.attachment or M.refreshing or M.submitting then return end
  M.refreshing = true
  local revision=generation
  z.task(function()
    local before=z.json.encode{M.snapshot or {},M.status,M.refresh_error or ''}
    local ok,err=pcall(function()
      local request = {method='workflow_show',directory=M.attachment.directory,run=M.attachment.run,launch=M.attachment.launch}
      local reply = z.await(z.rpc.call {plugin_url='zellij:swamp-bridge',name='swamp',payload=request,timeout=30})
      if revision~=generation then return end
      if reply.error then M.refresh_error=reply.error;return end
      M.refresh_error=nil
      M.snapshot = reply.result
      M.active = M.snapshot.active or M.snapshot.history.status == 'running' or M.snapshot.history.status == 'suspended'
      for _,child in ipairs(M.snapshot.children or {}) do
        M.active = M.active or child.status == 'running' or child.status == 'suspended'
      end
      M.attachment.run = M.snapshot.run
      M.status = 'Run ' .. (M.snapshot.run or M.snapshot.launch) .. ': ' .. M.snapshot.history.status
      local selected=identity(M.pending[M.selected])
      M.pending = {}
      local latest={}
      for _,entry in ipairs((M.snapshot.questions or {}).results or {}) do
        local key=identity(entry)
        if not latest[key] or entry.version>latest[key].version then latest[key]=entry end
      end
      for key,entry in pairs(latest) do
        if entry.attributes.state == 'pending' and (not answered[key] or entry.version>answered[key]) then
          M.pending[#M.pending+1] = entry
        end
      end
      table.sort(M.pending,function(a,b) return identity(a)<identity(b) end)
      M.selected=math.min(M.selected,math.max(1,#M.pending))
      for i,entry in ipairs(M.pending) do if identity(entry)==selected then M.selected=i end end
      controls()
    end)
    if not ok and revision==generation then M.refresh_error=tostring(err) end
    M.refreshing=false
    if before~=z.json.encode{M.snapshot or {},M.status,M.refresh_error or ''} then repaint() end
    if revision~=generation then M.refresh() end
  end)
end
function M.focused_control() controls();return form:focused_id() end
function M.attach(attachment)
  generation=generation+1
  M.attachment=attachment
  M.snapshot=nil
  M.pending={}
  M.selected=1
  M.scroll=0
  M.notice=nil
  M.refresh_error=nil
  M.active=true
  M.status='Loading workflow...'
  M.refresh()
end
function M.load(options)
  on_restart=options and options.on_restart
  form_key=nil
  z.subscribe {'key','mouse','pasted_text'}
  z.set_self_mouse_selection_support(true)
  controls()
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
  controls()
  if event.name=='key' and event.key=='PageUp' then M.scroll=math.max(0,M.scroll-content_height);return true end
  if event.name=='key' and event.key=='PageDown' then M.scroll=M.scroll+content_height;return true end
  if event.name=='mouse' then
    if event.kind=='scroll_up' then M.scroll=math.max(0,M.scroll-math.min(event.lines or 3,content_height));return true end
    if event.kind=='scroll_down' then M.scroll=M.scroll+math.min(event.lines or 3,content_height);return true end
    if event.kind=='left_click' then M.selecting=(event.line or 0)<(M.form_y or 0) end
    if M.selecting then
      if event.kind=='release' then
        M.selecting=false
        if M.dirty then M.dirty=false;repaint() end
      end
      return false
    end
    if event.kind~='release' and (event.line or -1)<(M.form_y or 0) then return false end
  end
  local action
  if event.name=='key' then action=form:handle_key(event)
  elseif event.name=='mouse' then action=form:handle_mouse(event)
  elseif event.name=='pasted_text' then action=form:insert_pasted_text(event.value)
  else return false end
  if not action then return false end
  if action.kind=='cancel' or action.id=='detach' then z.close_self();return true end
  if action.id=='next' then M.selected=M.selected % #M.pending+1;controls();return true end
  if action.kind~='button_pressed' or M.submitting then return action.kind~='ignored' end
  if action.id=='restart' and on_restart and not M.active and #M.pending==0 then
    on_restart()
    return true
  end
  local entry=M.pending[M.selected]
  local choice=action.id and tonumber(action.id:match('^choice_(%d+)$'))
  local command,success
  if entry and (action.id=='submit' or choice) then
    local q=entry.attributes
    if answered[identity(entry)] and entry.version<=answered[identity(entry)] then return false end
    local answer=choice and q.answers[choice] or form:value('answer')
    command={z.server_binary or 'zellij','workflow','answer',q.run,entry.model,q.question,answer}
    success='Answer recorded'
  elseif action.id=='cancel_run' and M.snapshot and M.attachment.run then
    command={z.server_binary or 'zellij','workflow','cancel',M.attachment.run}
    success='Cancellation requested; resources preserved'
  else return true end
  generation=generation+1
  M.submitting=true
  M.notice=success=='Answer recorded' and 'Submitting answer...' or 'Requesting cancellation...'
  z.task(function()
    local ok,result=pcall(function() return z.await(z.async.run_command {cwd=M.snapshot.root,context={swamp_owner='view'},command=command,max_bytes_returned=8192}) end)
    local accepted=ok and result.exit_code==0
    M.notice=accepted and success or (ok and ((result.stderr and result.stderr~='' and result.stderr) or (result.stdout and result.stdout~='' and result.stdout) or 'Command failed') or tostring(result))
    if accepted and command[3]=='answer' then
      answered[identity(entry)]=entry.version
      for i,pending in ipairs(M.pending) do if identity(pending)==identity(entry) then table.remove(M.pending,i);break end end
      M.selected=math.min(M.selected,math.max(1,#M.pending))
      controls()
    end
    M.submitting=false
    repaint()
    M.refresh()
  end)
  return true
end
function M.render(rows,cols)
  controls()
  local lines={}
  local entry=M.pending[M.selected]
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
      local function show_output(value,path)
        if not value.output then return end
        if value.previewTruncated then
          lines[#lines+1]='Output preview (up to 16,384 characters). Retrieve the complete saved version:'
          local predicate='modelName == '..z.json.encode(entry.model)..' && name == '..z.json.encode(entry.name)..' && version == '..entry.version
          lines[#lines+1]=text('swamp data query '..quote(predicate)..' --select '..path..' --json --repo-dir '..quote(M.snapshot.root))
        end
        if value.outputTruncated then lines[#lines+1]='Retained command scrollback (may omit older output):' end
        for line in value.output:gmatch('[^\n]+') do lines[#lines+1]=text(line) end
      end
      show_output(attrs,'attributes.output')
      if attrs.logError then lines[#lines+1]='Log unavailable: '..text(attrs.logError) end
      if attrs.test then
        lines[#lines+1]='Gate '..text(attrs.test.outcome)..' / exit '..text(attrs.test.exitCode)..' / log '..text(attrs.test.log)
        show_output(attrs.test,'attributes.test.output')
      end
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
  local pinned={text(M.status)}
  if M.notice then pinned[#pinned+1]=text(M.notice) end
  if M.refresh_error then pinned[#pinned+1]=text(M.refresh_error) end
  if entry then pinned[#pinned+1]=text(entry.attributes.prompt)
  elseif M.active then pinned[#pinned+1]='Waiting for the workflow. No question pending.' end
  local footer={}
  for _,line in ipairs(pinned) do
    for _,part in ipairs(z.text.wrap(line,math.max(1,cols))) do footer[#footer+1]=part end
  end
  local height=math.min(form_height,math.max(0,rows-(rows>=2 and 1 or 0)))
  local footer_height=math.min(#footer,math.floor((rows-height)/2))
  local content={}
  for i=footer_height+1,#footer do content[#content+1]=footer[i] end
  for _,line in ipairs(lines) do content[#content+1]=line end
  lines=content
  local page=math.max(0,rows-height-footer_height)
  content_height=page
  M.form_y=page+footer_height
  M.scroll=math.max(0,math.min(M.scroll,math.max(0,#lines-page)))
  local visible={}
  for i=M.scroll+1,math.min(#lines,M.scroll+page) do visible[#visible+1]=lines[i] end
  local frame={}
  for i,line in ipairs(visible) do frame[#frame+1]=z.ui.text(z.text.truncate(line,cols)):render{x=0,y=i-1,width=cols,height=1} end
  for i=1,footer_height do frame[#frame+1]=z.ui.text(footer[i]):render{x=0,y=page+i-1,width=cols,height=1} end
  return table.concat(frame) .. form:render{x=0,y=M.form_y,width=cols,height=height}
end
return M
