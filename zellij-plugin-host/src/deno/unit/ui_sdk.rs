#[test]
fn deno_markdown_lines_keep_native_styles_when_composed_into_transcripts() {
    use zellij_tile::ui_components::{serialize_text, Text};
    use zellij_tile_markdown::{render, style_text, Options};
    let mut runtime = sdk();
    let source = "# 日本🙂\n\n**bold** and ~~gone~~\n\n| Name | Value |\n| --- | --- |\n| a | `code` |";
    run(&mut runtime,&format!("globalThis.prose = zellij.ui.markdown({},{{dim:true}});",json!(source)));
    for width in [40, 12, 0, 40] {
        let lines = render(source,width,Options::default());
        let expected: Vec<_> = lines.iter().map(|line| {
            let text = style_text(&line.text,&line.styles,true).selected().color_all(2);
            json!([line.text,serialize_text(&text)])
        }).collect();
        run(&mut runtime,&format!("check(JSON.stringify(prose.lines({width}).map(line => [line.content(),line.selected().emphasis(2).render()])) === JSON.stringify({}));",json!(expected)));
    }
    let expected = serialize_text(&Text::new("日🙂x").strike_range(1..3));
    run(&mut runtime,&format!("check(zellij.ui.text('日🙂x').strike(1,3).render() === {});",json!(expected)));
    run(&mut runtime,r#"
        const struck = zellij.ui.text('done').strike();
        check(zellij.ui.nestedList([struck]).render().includes('0,1,2,3$'));
        const first = prose.lines(20)[0], pristine = first.render();
        first.selected();
        check(prose.lines(20)[0].render() === pristine);
        let refused = false; try { zellij.ui.text('x').emphasis(9); } catch (_) { refused = true; } check(refused);
    "#);
}

#[test]
fn deno_text_editor_supports_custom_prompt_cursors_history_and_completion() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
        const prompt = zellij.ui.textInput({value:'日本🙂 draft'});
        const inputKey = (bare_key, key_modifiers = []) => prompt.handleInputKey({Key:{bare_key,key_modifiers}});
        check(prompt.cursorPosition() === 9);
        prompt.setCursorPosition(2).insertText('x\r\ny\u0007');
        check(prompt.value() === '日本x\ny🙂 draft' && prompt.cursorPosition() === 5);
        check(prompt.undo() && prompt.value() === '日本🙂 draft' && prompt.cursorPosition() === 2);
        check(prompt.redo() && prompt.value() === '日本x\ny🙂 draft');
        prompt.replaceText('日🙂a',1);
        check(prompt.cursorPosition() === 1 && !prompt.undo() && !prompt.redo());
        prompt.setCursorPosition(999);
        check(prompt.cursorPosition() === 3);
        prompt.setCursorPosition(Number.MAX_SAFE_INTEGER);
        check(prompt.cursorPosition() === 3);
        prompt.setHistory(['first','日本🙂']).replaceText('draft',2);
        check(inputKey('Up') === 'continue' && prompt.value() === '日本🙂');
        check(inputKey('Up') === 'continue' && prompt.value() === 'first');
        inputKey('Down'); inputKey('Down');
        check(prompt.value() === 'draft');
        check(inputKey('Tab') === 'complete' && inputKey('Enter') === 'submit');
        check(inputKey('Esc') === 'cancel' && inputKey({Char:'q'},['Ctrl']) === 'ignored');
        check(prompt.drainText() === 'draft' && prompt.value() === '' && prompt.cursorPosition() === 0);
        check(JSON.stringify(prompt.history()) === JSON.stringify(['first','日本🙂','draft']));
        inputKey({Char:'p'},['Ctrl']);
        check(prompt.value() === 'draft');
        for (const action of [() => prompt.setCursorPosition(-1), () => prompt.replaceText('x',0.5), () => prompt.setHistory([1])]) {
            let refused = false; try { action(); } catch (_) { refused = true; } check(refused);
        }
    "#);
    assert!(runtime.op_state().borrow().borrow::<TestHost>().commands.is_empty());
}

#[test]
fn deno_lossless_input_wrapping_and_fuzzy_matches_use_the_shared_native_helpers() {
    use zellij_tile::{text::wrap_input, ui_components::forms::fuzzy_filter};
    let mut runtime = sdk();
    for source in ["", "one two  three", "日本🙂 a\nb", "full\n", "a\u{0301}bc"] {
        for width in [0, 1, 3, 4, 80] {
            let expected: Vec<_> = wrap_input(source, width).into_iter()
                .map(|line| json!({"text":line.text,"start":line.start})).collect();
            run(&mut runtime, &format!("check(JSON.stringify(zellij.text.wrapInput({}, {width})) === JSON.stringify({}));",json!(source),json!(expected)));
        }
    }
    let options: Vec<_> = ["日本🙂 layout", "compact", "default", "compact"].into_iter().map(str::to_owned).collect();
    for query in ["", "layout", "cmp", "dt", "impossible"] {
        let expected: Vec<_> = fuzzy_filter(&options, query).into_iter()
            .map(|m| json!({"index":m.index,"score":m.score,"indices":m.indices})).collect();
        run(&mut runtime,&format!("check(JSON.stringify(zellij.text.fuzzyFilter({}, {})) === JSON.stringify({}));",json!(options),json!(query),json!(expected)));
    }
    run(&mut runtime, r#"
        let refused = false;
        try { zellij.text.fuzzyFilter(['x'.repeat(10000)], 'x'.repeat(10000)); } catch (_) { refused = true; }
        check(refused);
        const match = zellij.text.fuzzyFilter(['日本🙂 layout'],'layout')[0];
        check(match.indices[0] === 4);
        const paths = Array.from({length:10000}, (_,i) => `src/${'directory/'.repeat(8)}file-${i}.ts`);
        const matches = zellij.text.fuzzyFilter(paths,'directory');
        check(matches.length === paths.length && matches.every(match => match.indices.length === 9));
    "#);
}

#[test]
fn deno_filesystem_reads_forward_optional_offsets_exactly() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
        bind({load() {
          zellij.fs.read('/host/large', {maxBytes:4096,offset:0});
          zellij.fs.read('/host/large', {maxBytes:4096n,offset:9007199254740993n});
          zellij.fs.read('/host/small');
        }});
        bridge('load',{},1);
    "#);
    let state = runtime.op_state();
    let state = state.borrow();
    let commands = &state.borrow::<TestHost>().commands;
    assert_eq!(commands[0].0, json!({"FsRead":{"path":"/host/large","max_bytes":4096,"offset":0}}));
    assert_eq!(commands[1].0, json!({"FsRead":{"path":"/host/large","max_bytes":4096,"offset":9007199254740993_u64}}));
    assert_eq!(commands[2].0, json!({"FsRead":{"path":"/host/small","max_bytes":null,"offset":null}}));
}

#[test]
fn deno_padding_preserves_long_combining_sequences_that_fit() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
        const combined = 'x' + '\u0300'.repeat(40000);
        check(zellij.text.pad(combined, 80) === combined + ' '.repeat(79));
        check(zellij.text.padLeft(combined, 80) === ' '.repeat(79) + combined);
        check(zellij.text.center(combined, 80) === ' '.repeat(39) + combined + ' '.repeat(40));
        for (const pad of [zellij.text.pad, zellij.text.padLeft, zellij.text.center]) {
            check(pad(combined, 1) === combined);
            check(pad(combined, 0) === '');
            check(pad('日本語', 4) === '日本');
            check(pad('日', 1) === ' ');
            check(pad('', 0) === '');
        }
    "#);
}

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
        let expected = super::ui::action(form.handle_mouse(mouse));
        run(&mut runtime,&format!("check(JSON.stringify(form.handleMouse({})) === JSON.stringify({expected}));",json!(mouse)));
        // The raw event as it arrived is accepted too, and means the same thing.
        let expected = super::ui::action(form.handle_mouse(mouse));
        run(&mut runtime,&format!("check(JSON.stringify(form.handleMouse({{Mouse: {}}})) === JSON.stringify({expected}));",json!(mouse)));
    }
    let key = KeyWithModifier::new(BareKey::Tab);
    let expected = super::ui::action(form.handle_key(key.clone()));
    run(&mut runtime,&format!("check(JSON.stringify(form.handleKey({{Key: {}}})) === JSON.stringify({expected}));",json!(key)));
    let expected = super::ui::action(form.handle_key(key.clone()));
    run(&mut runtime,&format!("check(JSON.stringify(form.handleKey({{InterceptedKeyPress: {}}})) === JSON.stringify({expected}));",json!(key)));
    run(&mut runtime, "check(`${zellij.ui.text('x').dim()}` === zellij.ui.text('x').dim().render()); check(String(zellij.ui.ribbon('r')) === zellij.ui.ribbon('r').render());");
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
