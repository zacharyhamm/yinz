#[test]
fn deno_widgets_match_shared_rust_controls_and_markdown() {
    use zellij_tile::ui_components::{forms::*, serialize_text_with_coordinates};
    let mut runtime = sdk();
    run(&mut runtime, r#"
        const ui = zellij.ui;
        globalThis.form = ui.form([
          {kind:'textInput',id:'name',label:'Name',value:'日'},
          {kind:'select',id:'layout',label:'Layout',options:['default','compact']},
          {kind:'combo',id:'branch',label:'Branch',options:['main','feature']},
          {kind:'checkbox',id:'frames',label:'Frames'},
          {kind:'radioGroup',id:'mode',label:'Mode',options:['one','two']},
          {kind:'button',id:'save',label:'Save'},
        ]);
        globalThis.refused = (fn) => { let rejected = false; try { fn(); } catch (_) { rejected = true; } check(rejected); };
        check(form.value('layout') === 0 && form.value('mode') === 0);
        const select = ui.select({options:['a','b'],value:null});
        check(select.value() === null);
        select.setValue(1); check(select.value() === 1);
        select.setValue(null); check(select.value() === null);
        select.setOptions([]); check(select.value() === null);
        refused(() => select.setValue(0));
        const radio = ui.radioGroup({options:['a','b'],value:1});
        radio.setOptions(['b','c']); check(radio.value() === 0);
        radio.setValue(null); check(radio.value() === null);
        const combo = ui.combo({options:['a']}); combo.setOptions([]).setValue('日本語'); check(combo.value() === '日本語');
        const checkbox = ui.checkbox(); checkbox.setValue(true); check(checkbox.value());
        const button = ui.button({label:'Save',disabled:true}); check(button.handleKey({bare_key:'Enter',key_modifiers:[]}).kind !== 'buttonPressed');
        for (const fn of [
          () => ui.form([{kind:'textInput'}]),
          () => ui.form([{kind:'textInput',id:'x'},{kind:'checkbox',id:'x'}]),
          () => ui.select({options:[false]}), () => ui.select({options:['a'],value:1}),
          () => ui.checkbox({value:'yes'}), () => ui.textInput({value:1}),
          () => ui.textInput({typo:true}), () => form.focus('missing'),
          () => form.render({width:-1,height:2}), () => form.render({width:0.5,height:2}),
          () => form.handleKey(false), () => form.handleMouse({bad:1}),
          () => ui.text('🙂').emphasis(1,0,2),
        ]) refused(fn);
        check(zellij.text.width('日🙂') === 4);
        check(zellij.text.truncate('日本語',4,'') === '日本');
        check(zellij.text.truncate('hello',1) === '…');
        check(zellij.text.truncate('hello',1,'日') === '');
        check(zellij.text.pad('日',4) === '日  ');
        check(zellij.text.padLeft('日',4) === '  日');
        check(zellij.text.center('日',5) === ' 日  ');
        check(ui.text('🙂a').len() === 2);
        check(ui.text('hi').selected().opaque().disabled().success().render().includes('104,105'));
        check(ui.ribbon('go').render().includes('zribbon;'));
        check(ui.table([['name','value']]).row('a',ui.text('b').dim()).render().includes('ztable;2;2;'));
        check(ui.nestedList([ui.text('x').disabled()]).render() === '\x1bPznested_list;d120\x1b\\');
        form.insertPastedText('本語\nhello');
    "#);
    let mut form = Form::new()
        .field("name", "Name", TextInput::new("日".into()))
        .select("layout", "Layout", Select::new(["default", "compact"]))
        .combo("branch", "Branch", Combo::new(["main", "feature"]))
        .checkbox("frames", "Frames", Checkbox::new(false))
        .radio("mode", "Mode", RadioGroup::new(["one", "two"]))
        .button("save", Button::new("Save"));
    form.insert_pasted_text("本語\nhello");
    use zellij_types::data::{BareKey, KeyWithModifier, Mouse};
    for bare in [BareKey::Left, BareKey::Backspace, BareKey::Tab, BareKey::Enter, BareKey::Down, BareKey::Char('c'), BareKey::Enter, BareKey::Tab, BareKey::Char('f'), BareKey::Down, BareKey::Enter, BareKey::Tab, BareKey::Char(' '), BareKey::Tab, BareKey::Enter, BareKey::Right, BareKey::Tab, BareKey::Enter, BareKey::Esc] {
        let key = KeyWithModifier::new(bare);
        let event = form.handle_key(key.clone());
        let expected = super::ui::action(event);
        run(&mut runtime, &format!("check(JSON.stringify(form.handleKey({})) === JSON.stringify({expected}));", json!(key)));
        for (width,height) in [(30,8),(10,2),(1,1),(0,0),(30,8)] {
            let output = form.render_buffer_in_viewport(2,3,width,height).output;
            run(&mut runtime,&format!("check(form.render({{x:2,y:3,width:{width},height:{height}}}) === {});",json!(output)));
        }
    }
    for mouse in [Mouse::LeftClick(3,3),Mouse::Release(3,3),Mouse::Hover(8,3)] {
        let expected = super::ui::action(form.handle_mouse(mouse.clone()));
        run(&mut runtime,&format!("check(JSON.stringify(form.handleMouse({})) === JSON.stringify({expected}));",json!(mouse)));
    }
    let source = "# 日本語\n\n**bold** ~~gone~~\n\n- [x] done\n\n---\n\n|a|b|\n|-|-|\n|1|2|";
    run(&mut runtime,&format!("globalThis.md = zellij.ui.markdown({});",json!(source)));
    for width in [30,1,0,12,30] {
        let lines = zellij_tile_markdown::render(source,width,Default::default());
        let expected: String = if width == 0 { String::new() } else { lines.iter().skip(1).take(3).enumerate().map(|(row,l)| serialize_text_with_coordinates(&zellij_tile_markdown::style_text(&l.text,&l.styles,false),0,row,Some(width),Some(1))).collect() };
        run(&mut runtime,&format!("{{ const result = md.render({{width:{width},height:3,scroll:1}}); check(result.text === {}); check(result.totalRows === {}); }}",json!(expected),lines.len()));
    }
}

#[test]
fn deno_widget_cursors_belong_only_to_accepted_client_frames() {
    let mut runtime = sdk();
    run(&mut runtime,r#"
        globalThis.resolveRender = null;
        globalThis.input = zellij.ui.textInput({value:'日'});
        bind({async render(rows,columns) {
          const width = await new Promise(resolve => { resolveRender = resolve; });
          return input.render({width,height:rows});
        }});
        bridge('render',[3,20],1);
        zellij.requestRender();
        resolveRender(20);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime,"check(!sent.some(([cmd]) => Object.hasOwn(cmd,'ShowCursor'))); bridge('render',[3,10],2); resolveRender(10);");
    capture_commands(&mut runtime);
    run(&mut runtime,"check(!sent.some(([cmd]) => Object.hasOwn(cmd,'ShowCursor'))); bridge('render',[3,10],2);");
    capture_commands(&mut runtime);
    run(&mut runtime,"check(sent.filter(([cmd]) => Object.hasOwn(cmd,'ShowCursor')).every(([,client]) => client === 2)); check(sent.some(([cmd]) => Object.hasOwn(cmd,'ShowCursor')));");
}

#[test]
fn deno_hocus_registration_replacement_rpc_provenance_and_async_clients() {
    let mut runtime = sdk();
    run(&mut runtime,r#"
        globalThis.pipeCount = 0;
        globalThis.finishTool = null;
        bind({load() {
          check(zellij.hocus.launch().namespace === 'weather');
          zellij.hocus.tools({forecast:{handler: async (args,call) => {
            await new Promise(resolve => { finishTool = resolve; });
            return {client:zellij.getPluginIds().client_id,source:call.source.Plugin,city:args.city};
          }}, failure:{handler() {throw Error('tool failed');}}});
        },pipe() {pipeCount++;}});
        bridge('load',{hocus_extension_name:'weather',hocus_extension_path:'/plugins/hocus/weather.ts'},1);
        globalThis.message = (name,payload,source={Plugin:7}) => ({name,payload:JSON.stringify(payload),source,args:{},is_private:true,pane_id:null});
        bridge('pipe',message('hocus_extension_discover',{}),2);
        bridge('pipe',message('zellij_rpc_call',{version:1,call_id:'a',name:'hocus_tool:forecast',payload:{city:'Austin'},source:{Plugin:99},reply_to:99}),3);
        bridge('pipe',message('zellij_rpc_call',{version:1,call_id:'b',name:'hocus_tool:failure'}),4);
        bridge('pipe',message('zellij_rpc_call',{version:2,call_id:'bad',name:'hocus_tool:failure'}),4);
        bridge('pipe',message('zellij_rpc_call',{version:1,call_id:'forged',name:'hocus_tool:failure',reply_to:7},'Keybind'),4);
        bridge('pipe',message('ordinary',{}),4);
        check(pipeCount === 1);
        finishTool();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime,r#"
        const messages = sent.filter(([cmd]) => cmd.MessageToPlugin).map(([cmd,client]) => [cmd.MessageToPlugin,client]);
        check(messages.filter(([m]) => m.message_name === 'hocus_extension_register').length === 2);
        const replies = messages.filter(([m]) => m.message_name === 'zellij_rpc_reply');
        check(replies.length === 2 && replies.every(([m]) => m.destination_plugin_id === 7));
        const [asyncReply,client] = replies.find(([m]) => JSON.parse(m.message_payload).call_id === 'a');
        check(client === 3 && JSON.parse(asyncReply.message_payload).result.client === 3);
        check(JSON.parse(asyncReply.message_payload).result.source === 7);
        check(JSON.parse(replies.find(([m]) => JSON.parse(m.message_payload).call_id === 'b')[0].message_payload).error.includes('tool failed'));
        bind({load() {zellij.hocus.tools({replacement:{handler: () => 'new'}});}});
        bridge('load',{hocus_extension_name:'weather',hocus_extension_path:'/plugins/hocus/weather.ts'},1);
        bridge('pipe',message('zellij_rpc_call',{version:1,call_id:'removed',name:'hocus_tool:forecast'}),1);
        bridge('pipe',message('zellij_rpc_call',{version:1,call_id:'new',name:'hocus_tool:replacement'}),1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime,r#"
        const last = sent.filter(([cmd]) => cmd.MessageToPlugin).slice(-3).map(([cmd]) => JSON.parse(cmd.MessageToPlugin.message_payload));
        check(last[0].tools.length === 1 && last[0].tools[0].name === 'replacement');
        check(last[1].error.includes('no rpc handler') && last[2].result === 'new');
    "#);
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some("MessageToPlugin");
    run(&mut runtime,r#"
        bind({load() {let denied = false; try {zellij.hocus.tools({bad:{handler:()=>''}});} catch (_) {denied=true;} check(denied);}});
        bridge('load',{hocus_extension_name:'weather',hocus_extension_path:'/plugins/hocus/weather.ts'},1);
        bind({load() {check(zellij.hocus.launch() === null); zellij.hocus.tools({unlaunched:{handler:()=>''}});}});
        bridge('load',{},1);
    "#);
}

#[test]
fn deno_dialog_rendering_matches_rust_and_reports_intent() {
    use zellij_tile::ui_components::{forms::*, Text};
    let mut runtime = sdk();
    run(&mut runtime,r#"
        globalThis.dialog = zellij.ui.dialog({title:'Confirm',message:'Continue?',help:'Esc: cancel',fields:[{kind:'textInput',id:'answer'},{kind:'button',id:'yes',label:'Yes'}]});
        dialog.setStatus('Ready');
        dialog.insertPastedText('日本語');
        check(dialog.value('answer') === '日本語');
    "#);
    let mut form = Form::new().field("answer","",TextInput::empty()).button("yes",Button::new("Yes"));
    form.insert_pasted_text("日本語");
    let mut dialog = Dialog::new(Text::new("Confirm"),form).message(Text::new("Continue?")).help(Text::new("Esc: cancel"));
    dialog.set_status(Some(Text::new("Ready")));
    for (width,height) in [(40,12),(10,4),(1,1),(0,0),(40,12)] {
        let expected = dialog.render_buffer(Rect::new(2,3,width,height)).output;
        run(&mut runtime,&format!("check(dialog.render({{x:2,y:3,width:{width},height:{height}}}) === {});",json!(expected)));
    }
    run(&mut runtime,r#"
        dialog.focus('yes');
        const result = dialog.handleKey({bare_key:'Enter',key_modifiers:[]});
        check(result.kind === 'buttonPressed' && result.id === 'yes');
        check(dialog.handleKey({bare_key:'Esc',key_modifiers:[]}).kind === 'cancel');
    "#);
    assert!(runtime.op_state().borrow().borrow::<TestHost>().commands.is_empty());
}
