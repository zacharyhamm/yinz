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
local streams={}
local latest_stream
local output_render_pending=false
local follow_live=true
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
  if not M.attachment then return end
  if M.refreshing or M.submitting then M.refresh_queued=true;return end
  M.refresh_queued=false
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
      if M.snapshot.launch and M.snapshot.launch~=M.launch then M.watch(M.snapshot.launch) end
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
    if revision~=generation or M.refresh_queued then M.refresh() end
  end)
end
function M.focused_control() controls();return form:focused_id() end
function M.watch(launch)
  if launch~=M.launch then streams={};follow_live=true end
  M.launch=launch
  z.subscribe_pane_output(launch and {swamp_work=launch} or z.null)
end
function M.attach(attachment)
  M.watch(attachment.launch)
  generation=generation+1
  M.attachment=attachment
  M.submitting=false
  M.started=nil
  M.snapshot=nil
  M.pending={}
  M.selected=1
  M.scroll=0
  M.notice=nil
  M.notice_error=nil
  M.refresh_error=nil
  M.active=true
  M.status='Loading workflow...'
  M.refresh()
end
function M.load(options)
  on_restart=options and options.on_restart
  form_key=nil
  z.subscribe {'key','mouse','pasted_text','pane_output','command_pane_exited'}
  z.set_self_mouse_selection_support(true)
  controls()
  z.every(2,M.refresh)
  z.every(1,function() if M.started and (M.submitting or M.active) then repaint() end end)
end
function M.pipe(message)
  if message.name=='workflow-changed' then M.refresh();return end
  if message.name ~= 'show' or not message.payload then return end
  M.attach(z.json.decode(message.payload))
  z.show_self(true)
  local pipe = type(message.source)=='table' and message.source.Cli
  if pipe then z.cli_pipe_output(pipe,z.json.encode({attached=true})); z.unblock_cli_pipe_input(pipe) end
end
function M.update(event)
  if event.name=='pane_output' and event.context and event.context.swamp_work==M.launch then
    M.output(event.context.swamp_operation or 'command','pane '..event.pane_id,event.data)
    return false
  end
  if event.name=='command_pane_exited' and event.context and event.context.swamp_work==M.launch then M.refresh();return false end
  controls()
  if event.name=='key' and event.key=='End' then follow_live=true;M.scroll=math.huge;return true end
  if event.name=='key' and event.key=='PageUp' then follow_live=false; M.scroll=math.max(0,M.scroll-content_height);return true end
  if event.name=='key' and event.key=='PageDown' then M.scroll=M.scroll+content_height;return true end
  if event.name=='mouse' then
    if event.kind=='scroll_up' then follow_live=false;M.scroll=math.max(0,M.scroll-math.min(event.lines or 3,content_height));return true end
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
  if action.kind=='cancel' or action.id=='detach' then z.subscribe_pane_output(z.null);z.close_self();return true end
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
  M.notice_error=false
  M.started=os.time()
  M.action=choice and entry.attributes.answers[choice] or action.id
  M.notice=success=='Answer recorded' and 'Submitting answer...' or 'Requesting cancellation...'
  local attachment=M.attachment
  local operation=(entry and identity(entry) or command[4])..': '..M.action
  repaint()
  z.task(function()
    local ok,result=pcall(function() return z.await(z.async.run_command {cwd=M.snapshot.root,context={swamp_owner='view'},command=command,max_bytes_returned=8192,
      on_output=function(event) M.output(operation,event.stderr and 'stderr' or 'stdout',event.data) end}) end)
    local accepted=ok and result.exit_code==0
    if M.attachment~=attachment then return end
    M.notice_error=not accepted
    M.notice=accepted and success or (ok and ((result.stderr and result.stderr~='' and result.stderr) or (result.stdout and result.stdout~='' and result.stdout) or 'Command failed') or tostring(result))
    if not accepted then M.output(operation,'submission error',M.notice) end
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
function M.output(operation,source,data)
  local stream
  for _,entry in ipairs(streams) do
    if entry.operation==operation and entry.source==source then stream=entry;break end
  end
  if not stream then
    stream={operation=operation,source=source,raw=''}
    streams[#streams+1]=stream
  end
  latest_stream=stream
  stream.raw=stream.raw..data
  if #stream.raw>65536 then
    stream.raw=stream.raw:sub(-65536)
    -- Drop any UTF-8 continuation bytes at the retained boundary.
    stream.raw=stream.raw:gsub('^[\128-\191]+','')
    stream.omitted=true
  end
  local raw=stream.raw
  -- Withhold an incomplete final codepoint until the next chunk arrives.
  local last=raw:match('([\194-\244][\128-\191]*)$')
  if last then
    local first=last:byte()
    local need=first<224 and 2 or first<240 and 3 or 4
    if #last<need then raw=raw:sub(1,#raw-#last) end
  end
  -- Remove complete terminal escape sequences; raw retains incomplete ones across chunks.
  raw=raw:gsub('\27%][^\7\27]*\7',''):gsub('\27%][^\27]*\27\\','')
  raw=raw:gsub('\27%[[0-?]*[ -/]*[@-~]',''):gsub('\27[@-_]','')
  stream.value=z.text.sanitize_bytes(raw)
  -- Deliver every chunk, but let queued chunks share the next render.
  if not output_render_pending then
    output_render_pending=true
    z.after(0,function() output_render_pending=false;repaint() end)
  end
end
local markdown_cache={}
local function failed(status)
  return status=='failed' or status=='error'
end
function M.render(rows,cols)
  controls()
  local blocks={}
  local cache={}
  local function markdown(source,dim)
    local slot=#cache+1
    local previous=markdown_cache[slot]
    local widget=previous and previous.source==source and previous.dim==dim and previous.widget
      or z.ui.markdown(source,{dim=dim})
    cache[slot]={source=source,dim=dim,widget=widget}
    local _,count=widget:render{x=0,y=0,width=cols,height=0}
    return {widget=widget,count=count,scroll=0}
  end
  local function add(source,important,is_markdown)
    if is_markdown then
      blocks[#blocks+1]=markdown(source,not important)
    else
      local lines=z.text.wrap(text(source),math.max(1,cols))
      blocks[#blocks+1]={lines=lines,count=#lines,important=important,scroll=0}
    end
  end
  local entry=M.pending[M.selected]
  local question=entry and markdown(entry.attributes.prompt,false)
  add(M.status,M.snapshot and failed(M.snapshot.history.status))
  if M.notice then add(M.notice,M.notice_error) end
  if M.refresh_error then add(M.refresh_error,true) end
  if not entry and M.active then add('Waiting for the workflow. No question pending.') end
  if M.snapshot then
    local histories={M.snapshot.history}
    for _,child in ipairs(M.snapshot.children or {}) do histories[#histories+1]=child end
    for _,history in ipairs(histories) do
    add(text(history.workflowName).." / "..text(history.id)..": "..text(history.status), failed(history.status))
    for _,job in ipairs(history.jobs or {}) do
      for _,step in ipairs(job.steps or {}) do add(text(job.name .. '/' .. step.name .. ': ' .. step.status), failed(step.status)) end
    end
    end
    for _,entry in ipairs((M.snapshot.outputs or {}).results or {}) do
      add(text(entry.model..'/'..entry.name..' v'..entry.version))
      local attrs=entry.attributes or {}
      local review=attrs.review or attrs
      for _,finding in ipairs(review.findings or {}) do add(text(finding.severity..' '..finding.file..': '..finding.message), true) end
      if attrs.pane then add('Pane '..attrs.pane..' / '..text(attrs.log)..' / '..text(attrs.outcome), failed(attrs.outcome)) end
      local function show_output(value,path)
        if not value.output then return end
        if value.previewTruncated then
          add('Output preview (up to 16,384 characters). Retrieve the complete saved version:')
          local predicate='modelName == '..z.json.encode(entry.model)..' && name == '..z.json.encode(entry.name)..' && version == '..entry.version
          add(text('swamp data query '..quote(predicate)..' --select '..path..' --json --repo-dir '..quote(M.snapshot.root)))
        end
        if value.outputTruncated then add('Retained command scrollback (may omit older output):') end
        add(value.output, false, true)
      end
      show_output(attrs,'attributes.output')
      if attrs.logError then add('Log unavailable: '..text(attrs.logError), true) end
      if attrs.test then
        add('Gate '..text(attrs.test.outcome)..' / exit '..text(attrs.test.exitCode)..' / log '..text(attrs.test.log), failed(attrs.test.outcome) or (type(attrs.test.exitCode)=='number' and attrs.test.exitCode~=0))
        show_output(attrs.test,'attributes.test.output')
      end
      local evidence=attrs.evidence or (attrs.source and attrs)
      if evidence then add('Source '..text(evidence.source.commit)..' / target '..text(evidence.target.commit)) end
    end
    add('Workflow log (last 100 lines):')
    add(M.snapshot.logs or '', false, true)
  end
  for _,stream in ipairs(streams) do
    add(stream.value)
    add('Live '..stream.operation..' / '..stream.source..(stream.omitted and ' [older output omitted]' or '')..' (End: live)')
    blocks[#blocks].live=stream==latest_stream
  end
  markdown_cache=cache
  local status_height=M.notice and rows>=3 and 1 or 0
  local height=math.min(form_height,math.max(0,rows-status_height-(rows>=2 and 1 or 0)))
  local footer_height=question and math.min(question.count,math.floor((rows-height-status_height)/2)) or 0
  if question and question.count>footer_height then
    table.insert(blocks,1,{widget=question.widget,count=question.count-footer_height,scroll=footer_height})
  end
  local separator=footer_height>0 and rows-height-status_height-footer_height>=2 and 1 or 0
  local page=math.max(0,rows-height-status_height-footer_height-separator)
  content_height=page
  M.form_y=rows-height
  local total,live_end=0,nil
  for _,block in ipairs(blocks) do
    total=total+block.count
    if block.live then live_end=total end
  end
  if #streams>0 and follow_live then M.scroll=math.max(0,(live_end or total)-page) end
  M.scroll=math.max(0,math.min(M.scroll,math.max(0,total-page)))
  local frame={}
  local function draw(block,y,skip,count)
    if block.widget then
      frame[#frame+1]=block.widget:render{x=0,y=y,width=cols,height=count,scroll=block.scroll+skip}
    else
      for i=1,count do
        local line=z.ui.text(block.lines[skip+i])
        line=block.important and line:error() or line:dim()
        frame[#frame+1]=line:render{x=0,y=y+i-1,width=cols,height=1}
      end
    end
  end
  local skip,y=M.scroll,0
  for _,block in ipairs(blocks) do
    local count=math.min(math.max(0,block.count-skip),page-y)
    if count>0 then draw(block,y,skip,count);y=y+count end
    skip=math.max(0,skip-block.count)
  end
  if footer_height>0 then draw(question,page+separator,0,footer_height) end
  if status_height>0 then
    local status=(M.action or '')..': '..M.notice
    if M.started then status=status..' ('..math.max(0,os.time()-M.started)..'s)' end
    if M.notice=='Answer recorded' then status=status..(#M.pending>0 and '; next question ready' or '; waiting for workflow...') end
    local line=z.ui.text(z.text.truncate(text(status),cols))
    if M.notice_error then line=line:error() end
    frame[#frame+1]=line:render{x=0,y=M.form_y-status_height,width=cols,height=1}
  end
  return table.concat(frame) .. form:render{x=0,y=M.form_y,width=cols,height=height}
end
return M
