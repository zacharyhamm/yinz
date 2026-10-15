use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn set_pane_regex_highlights_via_plugin() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread_struct_variant!(
        received_screen_instructions,
        ScreenInstruction::SetPluginRegexHighlights,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Super+g key event to trigger set_pane_regex_highlights in the fixture plugin
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('g')).with_super_modifier()),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let highlight_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::SetPluginRegexHighlights {
            pane_id,
            plugin_id,
            highlights,
        } = i
        {
            Some((*pane_id, *plugin_id, highlights.clone()))
        } else {
            None
        }
    });

    assert!(
        highlight_instruction.is_some(),
        "Expected SetPluginRegexHighlights instruction"
    );
    let (pane_id, _plugin_id, highlights) = highlight_instruction.unwrap();
    assert_eq!(pane_id, PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(highlights.len(), 1);
    assert_eq!(highlights[0].pattern, "test_pattern");
    assert!(highlights[0].italic);
    assert!(highlights[0].underline);
}

#[test]
#[serial(plugin_tests)]
pub fn clear_pane_highlights_via_plugin() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread_struct_variant!(
        received_screen_instructions,
        ScreenInstruction::ClearPluginHighlights,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Super+h key event to trigger clear_pane_highlights in the fixture plugin
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('h')).with_super_modifier()),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let clear_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::ClearPluginHighlights { pane_id, plugin_id } = i {
            Some((*pane_id, *plugin_id))
        } else {
            None
        }
    });

    assert!(
        clear_instruction.is_some(),
        "Expected ClearPluginHighlights instruction"
    );
    let (pane_id, _plugin_id) = clear_instruction.unwrap();
    assert_eq!(pane_id, PaneId::Terminal(crate::TerminalId::new(1)));
}

#[test]
#[serial(plugin_tests)]
pub fn highlight_clicked_event_delivered_to_plugin() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
        PermissionStatus::Granted,
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    wait_for_permissions(&received_screen_instructions);

    // Send a HighlightClicked event directly to the plugin
    let _ = plugin_thread_sender.send(PluginInstruction::HighlightClicked {
        plugin_id: crate::PluginId::new(0),
        client_id,
        pane_id: PaneId::Terminal(crate::TerminalId::new(1)),
        pattern: "test".into(),
        matched_string: "test_match".into(),
        context: BTreeMap::new(),
    });

    let frame = wait_for_render_containing(&received_screen_instructions, "HighlightClicked");
    teardown();
    screen_thread.join().unwrap();

    assert!(
        fixture_events(frame.as_bytes()).contains(&Event::HighlightClicked {
            pane_id: PaneId::Terminal(crate::TerminalId::new(1)),
            pattern: "test".into(),
            matched_string: "test_match".into(),
            context: BTreeMap::new(),
        })
    );
}
