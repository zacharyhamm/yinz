use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn run_command_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, background_jobs_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_background_jobs_receiver(Some(plugin_host_folder), None);
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
    let received_background_jobs_instructions = Watched::new(vec![]);
    let background_jobs_thread = log_actions_in_thread!(
        received_background_jobs_instructions,
        BackgroundJob::RunCommand,
        background_jobs_receiver,
        1
    );
    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('2')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    background_jobs_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_background_job = received_background_jobs_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let BackgroundJob::RunCommand(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert!(format!("{:#?}", new_background_job).contains("user_value_1"));
}

#[test]
#[serial(plugin_tests)]
pub fn run_command_with_env_vars_and_cwd_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, background_jobs_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_background_jobs_receiver(Some(plugin_host_folder), None);
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
    let received_background_jobs_instructions = Watched::new(vec![]);
    let background_jobs_thread = log_actions_in_thread!(
        received_background_jobs_instructions,
        BackgroundJob::RunCommand,
        background_jobs_receiver,
        1
    );
    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('3')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    background_jobs_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_background_jobs_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let BackgroundJob::RunCommand(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn web_request_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, background_jobs_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_background_jobs_receiver(Some(plugin_host_folder), None);
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
    let received_background_jobs_instructions = Watched::new(vec![]);
    let background_jobs_thread = log_actions_in_thread!(
        received_background_jobs_instructions,
        BackgroundJob::WebRequest,
        background_jobs_receiver,
        1
    );
    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::WebAccess,
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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('4')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    background_jobs_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_background_jobs_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let BackgroundJob::WebRequest(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn unblock_input_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_folder = tempdir().unwrap();
    let cache_path = cache_folder.path().join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let server_log = Watched::new(vec![]);
    let server_thread = log_until_disconnect(&server_log, server_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "input_pipe_id".to_owned(),
        name: "message_name".to_owned(),
        payload: Some("message_payload".to_owned()),
        plugin: None, // broadcast
        args: None,
        configuration: None,
        floating: None,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        source_pane_id: None,
    });
    server_log.wait_until("pipe success without screen acknowledgement", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ServerInstruction::UnblockCliPipeInput(pipe) if pipe == "input_pipe_id")).then_some(())
    });
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap();
    assert!(!server_log
        .lock()
        .iter()
        .any(|i| matches!(i, ServerInstruction::CliPipeError(..))));
}

#[test]
#[serial(plugin_tests)]
pub fn block_input_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
                                          // Permission bookkeeping must not become input to the fixture's filesystem watcher.
    let plugin_host_folder = temp_folder.path().join("host");
    std::fs::create_dir(&plugin_host_folder).unwrap();
    let cache_path = temp_folder.path().join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let server_log = Watched::new(vec![]);
    let server_thread = log_until_disconnect(&server_log, server_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "input_pipe_id".to_owned(),
        name: "message_name_block".to_owned(),
        payload: Some("message_payload".to_owned()),
        plugin: None, // broadcast
        args: None,
        configuration: None,
        floating: None,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        source_pane_id: None,
    });
    received_screen_instructions.wait_until(
        "pipe callback's requested render",
        default_timeout(),
        |log| {
            rendered_assets(log)
                .iter()
                .any(|(_, frame)| frame.contains("input_pipe_id blocked"))
                .then_some(())
        },
    );
    assert!(!server_log.lock().iter().any(
        |i| matches!(i, ServerInstruction::UnblockCliPipeInput(pipe) if pipe == "input_pipe_id")
    ));
    plugin_thread_sender
        .send(PluginInstruction::Unload(crate::PluginId::new(0)))
        .unwrap();
    server_log.wait_until("retirement fails an explicit hold", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ServerInstruction::CliPipeError(pipe, _) if pipe == "input_pipe_id")).then_some(())
    });
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap();
    assert!(server_log
        .lock()
        .iter()
        .any(|i| matches!(i, ServerInstruction::CliPipeError(pipe, _) if pipe == "input_pipe_id")));
}

#[test]
#[serial(plugin_tests)]
pub fn pipe_output_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::CliPipeOutput,
        server_receiver,
        1
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
    wait_for_render_containing(&received_screen_instructions, "Rows: 20, Cols: 121");

    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "input_pipe_id".to_owned(),
        name: "pipe_output".to_owned(),
        payload: Some("message_payload".to_owned()),
        plugin: None, // broadcast
        args: None,
        configuration: None,
        floating: None,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        source_pane_id: None,
    });
    received_server_instruction.wait_until(
        "pipe output reaches the server",
        default_timeout(),
        |log| {
            log.iter()
                .any(|event| matches!(event, ServerInstruction::CliPipeOutput(..)))
                .then_some(())
        },
    );
    teardown();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let plugin_bytes_events = received_server_instruction
        .lock()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::CliPipeOutput(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", plugin_bytes_events));
}

#[test]
#[serial(plugin_tests)]
pub fn pipe_message_to_plugin_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
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
    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "input_pipe_id".to_owned(),
        name: "pipe_message_to_plugin".to_owned(),
        payload: Some("payload_sent_to_self".to_owned()),
        plugin: None, // broadcast
        args: None,
        configuration: None,
        floating: None,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        source_pane_id: None,
    });
    wait_for_render_containing(&received_screen_instructions, "Payload from self:");
    teardown();
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
            for plugin_render_asset in plugin_render_assets {
                let plugin_id = plugin_render_asset.plugin_id;
                let client_id = plugin_render_asset.client_id;
                let plugin_bytes = plugin_render_asset.bytes.clone();
                let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                if plugin_bytes.contains("Payload from self:") {
                    return Some((plugin_id, client_id, plugin_bytes));
                }
            }
        }
        None
    });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn switch_session_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::SwitchSession,
        server_receiver,
        1
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
    wait_for_render_containing(&received_screen_instructions, "Rows: 20, Cols: 121");

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('5')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    received_server_instruction.wait_until(
        "the session switch to reach the server before teardown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|event| matches!(event, ServerInstruction::SwitchSession(..)))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let switch_session_event = received_server_instruction
        .lock()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::SwitchSession(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    // we do the replace below to avoid the randomness of the temporary folder in the snapshot
    // while still testing it
    assert_snapshot!(format!("{:#?}", switch_session_event)
        .replace(&format!("{:?}", temp_folder.path()), "\"CWD\""));
}

#[test]
#[serial(plugin_tests)]
pub fn switch_session_with_layout_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::SwitchSession,
        server_receiver,
        1
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
    wait_for_render_containing(&received_screen_instructions, "Rows: 20, Cols: 121");

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('7')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    received_server_instruction.wait_until(
        "the session switch to reach the server before teardown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|event| matches!(event, ServerInstruction::SwitchSession(..)))
                .then_some(())
        },
    );
    teardown();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let switch_session_event = received_server_instruction
        .lock()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::SwitchSession(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    // we do the replace below to avoid the randomness of the temporary folder in the snapshot
    // while still testing it
    assert_snapshot!(format!("{:#?}", switch_session_event)
        .replace(&format!("{:?}", temp_folder.path()), "\"CWD\""));
}

#[test]
#[serial(plugin_tests)]
pub fn switch_session_with_layout_and_cwd_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::SwitchSession,
        server_receiver,
        1
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
    wait_for_render_containing(&received_screen_instructions, "Rows: 20, Cols: 121");

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('9')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    received_server_instruction.wait_until(
        "the session switch to reach the server before teardown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|event| matches!(event, ServerInstruction::SwitchSession(..)))
                .then_some(())
        },
    );
    teardown();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let switch_session_event = received_server_instruction
        .lock()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::SwitchSession(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", switch_session_event));
}

#[test]
#[serial(plugin_tests)]
pub fn disconnect_other_clients_plugins_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::DisconnectAllClientsExcept,
        server_receiver,
        1
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

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('6')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    std::thread::sleep(std::time::Duration::from_millis(500));
    teardown();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let switch_session_event = received_server_instruction
        .lock()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::DisconnectAllClientsExcept(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", switch_session_event));
}

#[test]
#[serial(plugin_tests)]
pub fn reconfigure_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread_struct!(
        received_server_instruction,
        ServerInstruction::Reconfigure,
        server_receiver,
        1
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

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('0')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    received_server_instruction.wait_until(
        "the configuration change to reach the server before teardown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|event| matches!(event, ServerInstruction::Reconfigure { .. }))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let reconfigure_event = received_server_instruction
        .lock()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::Reconfigure { .. } = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", reconfigure_event));
}

#[test]
#[serial(plugin_tests)]
pub fn run_plugin_in_specific_cwd() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder.clone()), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let plugin_initial_cwd = plugin_host_folder.join("custom_plugin_cwd");
    std::fs::create_dir_all(&plugin_initial_cwd).unwrap();
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        initial_cwd: Some(plugin_initial_cwd.clone()),
        relative_path: None,
        declared_path: None,
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
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::ClientExit,
        server_receiver,
        1
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

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('8')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    wait_for_render_containing(&received_screen_instructions, r#""bare_key":{"Char":"8"}"#);
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('8'))), // this sends this quit command so tha the test exits cleanly
    )]));
    received_server_instruction.wait_until(
        "the plugin's quit command to reach the server before teardown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|event| matches!(event, ServerInstruction::ClientExit(..)))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap();
    assert!(
        std::fs::read_dir(plugin_initial_cwd)
            .unwrap()
            .map(|d| d.unwrap().path().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(",")
            .contains("hi-from-plugin.txt"),
        "File written into plugin initial cwd"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn cli_pipe_is_released_when_plugin_panics_while_handling_it() {
    // a plugin that traps while handling a CLI pipe message is not unloaded (its pane just shows
    // the error), so nothing else would ever release the pipe: the `yinz pipe` client would
    // stay blocked until the plugin is unloaded or the session ends. the pipe must be released at
    // crash time.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
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
    let screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ReadCliPipes,
        cache_path,
        plugin_thread_sender,
        client_id
    );
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::CliPipeError,
        server_receiver,
        1
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
    // the plugin's first render is proof that it is loaded and ready for the pipe
    received_screen_instructions.wait_until(
        "the plugin to load and render",
        yinz_test_support::default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::PluginBytes(..)))
                .then_some(())
        },
    );

    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "input_pipe_id".to_owned(),
        name: "panic_while_handling_pipe".to_owned(),
        payload: None,
        plugin: None, // broadcast
        args: None,
        configuration: None,
        floating: None,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        source_pane_id: None,
    });
    received_server_instructions.wait_until_described(
        "the pipe to be released when the plugin crashes while handling it (otherwise the \
         `yinz pipe` client stays blocked until the plugin is unloaded)",
        yinz_test_support::default_timeout(),
        |log| {
            log.iter()
                .any(|i| {
                    matches!(i, ServerInstruction::CliPipeError(pipe_name, _) if pipe_name == "input_pipe_id")
                })
                .then_some(())
        },
        |log| format!("{:#?}", log),
    );
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap();
}

/// A keybind is routed without a pane id, so its `in_place` arrives as a flag rather than as a
/// `pane_id_to_replace`; the host must still ask the screen to replace the client's focused pane.
#[test]
#[serial(plugin_tests)]
pub fn a_keybind_pipe_opened_in_place_asks_the_screen_to_replace_the_focused_pane() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::AddPlugin,
        screen_receiver,
        1,
        &PermissionType::ReadCliPipes,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::KeybindPipe {
        name: "go".to_owned(),
        payload: None,
        plugin: Some(format!("file:{}", *PLUGIN_FIXTURE)),
        args: None,
        configuration: None,
        floating: Some(false),
        should_open_in_place: true,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        plugin_and_client_id: None,
        notification_end: None,
    });
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let placement = received_screen_instructions
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            ScreenInstruction::AddPlugin(
                should_float,
                should_be_open_in_place,
                _,
                _,
                _,
                _,
                _,
                pane_id_to_replace,
                ..,
            ) => Some((*should_float, *should_be_open_in_place, *pane_id_to_replace)),
            _ => None,
        })
        .expect("the pipe loaded the plugin");
    assert_eq!(
        placement,
        (Some(false), true, None),
        "in place, with the focused pane left for the screen to resolve"
    );
}
