use super::*;
use crate::plugin_map::{PluginMap, RunningPlugin};
use crate::test_env::test_env;
use serde_json::json;
use std::collections::HashSet;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_types::data::{EventType, PermissionType, PipeSource};
use yinz_types::input::layout::RunPluginLocation;

fn plugin(source: &str) -> SteelPlugin {
    SteelPlugin::new(source, Path::new("test.scm"), env(), Arc::default()).unwrap()
}

fn env() -> PluginEnv {
    let mut env = test_env();
    env.plugin_config.path = "test.scm".into();
    env.plugin_config.location = RunPluginLocation::File("test.scm".into());
    env
}

#[test]
fn lifecycle_uses_shared_subscriptions_commands_and_replies() {
    let mut plugin = plugin(
        r#"
        (define text "initial")
        (define (load configuration)
          (yinz-command (hash "Subscribe" (list "CustomMessage")))
          (set! text (string-append
            (hash-ref configuration "prefix")
            (number->string (hash-ref (yinz-command "GetPluginIds") "plugin_id")))))
        (define (update event)
          (set! text (car (hash-ref event "CustomMessage")))
          #t)
        (define (pipe message) (set! text (hash-ref message "payload")) #t)
        (define (render rows columns) text)
        (define (unload) (yinz-command (hash "Unsubscribe" (list "CustomMessage"))))
    "#,
    );
    plugin
        .load(&BTreeMap::from([("prefix".into(), "plugin-".into())]))
        .unwrap();
    assert_eq!(plugin.render(10, 80).unwrap(), b"plugin-1");
    assert!(plugin
        .env()
        .subscriptions
        .lock()
        .unwrap()
        .contains(&EventType::CustomMessage));
    assert!(plugin
        .update(&Event::CustomMessage("event".into(), "payload".into()))
        .unwrap());
    assert_eq!(plugin.render(10, 80).unwrap(), b"event");
    assert!(plugin
        .pipe(&PipeMessage::new(
            PipeSource::Keybind,
            "test",
            &Some("pipe\nframe".into()),
            &None,
            false
        ))
        .unwrap());
    assert_eq!(plugin.render(10, 80).unwrap(), b"pipe\n\rframe");
    let generation = plugin.generation();
    plugin.dispose_with_cleanup(true);
    assert!(!plugin
        .env()
        .subscriptions
        .lock()
        .unwrap()
        .contains(&EventType::CustomMessage));
    assert!(RUNTIMES.with(|runtimes| !runtimes.borrow().contains_key(&generation)));
}

#[test]
fn denied_commands_are_errors_and_host_state_survives_them() {
    let mut plugin = plugin(
        r#"
        (define (load configuration)
          (yinz-command (hash "RenameTabWithId" (list 1 "denied"))))
        (define (render rows columns) (yinz-command "GetYinzVersion"))
    "#,
    );
    let error = plugin.load(&BTreeMap::new()).unwrap_err();
    assert!(format!("{error:#}").contains("ChangeApplicationState"));
    assert!(plugin.env().response_sink.is_none());
    assert!(!plugin.render(10, 80).unwrap().is_empty());
    plugin
        .env_mut()
        .permissions_requested
        .store(true, Ordering::Release);
    let error = plugin.load(&BTreeMap::new()).unwrap_err();
    assert!(format!("{error:#}").contains("PermissionRequestResult"));
    plugin
        .env_mut()
        .set_permissions([PermissionType::ChangeApplicationState].into());
    plugin.load(&BTreeMap::new()).unwrap();
}

#[test]
fn module_initialization_cannot_issue_host_commands() {
    let result = SteelPlugin::new(
        "(yinz-command \"GetPluginIds\")",
        Path::new("test.scm"),
        env(),
        Arc::default(),
    );
    let error = match result {
        Ok(_) => panic!("module initialization acquired host authority"),
        Err(error) => error,
    };
    assert!(format!("{error:#}").contains("unavailable during module initialization"));
}

#[test]
fn replacement_and_disconnect_reject_old_generations() {
    let mut map = PluginMap::default();
    let first = plugin("(define (render rows columns) \"old\")");
    let old = first.generation();
    let id = first.env().plugin_id;
    let client = first.env().client_id;
    let subscriptions = first.env().subscriptions.clone();
    let first = Arc::new(Mutex::new(RunningPlugin::new(first, 10, 80)));
    let previous = map.insert(id, client, first.clone(), subscriptions);
    assert!(previous.is_none());
    assert!(map.contains_generation(id, old));
    let second = plugin("(define (render rows columns) \"new\")");
    let new = second.generation();
    let cancel = second.client_cancel();
    let subscriptions = second.env().subscriptions.clone();
    let previous = map.insert(
        id,
        client,
        Arc::new(Mutex::new(RunningPlugin::new(second, 10, 80))),
        subscriptions,
    );
    previous.unwrap().0.lock().unwrap().dispose();
    assert!(map.remove_matching_instance(id, &first).is_none());
    assert!(!map.contains_generation(id, old));
    assert!(map.contains_generation(id, new));
    assert!(RUNTIMES.with(|runtimes| !runtimes.borrow().contains_key(&old)));
    cancel.store(true, Ordering::Release);
    assert!(!map.contains_generation(id, new));
    assert_eq!(map.remove_disconnected_instances(id, &[]).len(), 1);
}

#[test]
fn cancellation_interrupts_guest_loops_and_skips_unload() {
    let mut env = env();
    let cancel = env.cancel.clone();
    let (sender, receiver) = channels::unbounded();
    env.senders.to_plugin = Some(SenderWithContext::new(sender));
    let mut plugin = SteelPlugin::new(
        r#"
        (define (load configuration)
          (yinz-command (hash "Subscribe" (list "Key")))
          (let loop () (loop)))
        (define (unload) (yinz-command (hash "Subscribe" (list "CustomMessage"))))
    "#,
        Path::new("cancel.scm"),
        env,
        Arc::default(),
    )
    .unwrap();
    let canceller = thread::spawn(move || {
        let (instruction, _) = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            instruction,
            PluginInstruction::PluginSubscribedToEvents(..)
        ));
        cancel.store(true, Ordering::Release);
    });
    assert!(plugin.load(&BTreeMap::new()).is_err());
    canceller.join().unwrap();
    assert!(plugin.env().cancel_tripped.load(Ordering::Acquire));
    plugin.dispose_with_cleanup(true);
    assert!(!plugin
        .env()
        .subscriptions
        .lock()
        .unwrap()
        .contains(&EventType::CustomMessage));
}

#[test]
fn invalid_commands_are_rejected_before_dispatch() {
    let mut env = env();
    assert!(dispatch(&mut env, json!({"Subscribe": ["NotAnEvent"]})).is_err());
    assert!(env.subscriptions.lock().unwrap().is_empty());
    assert!(env.response_sink.is_none());
}

#[test]
fn notification_history_waits_for_a_busy_screen() {
    let mut env = env();
    env.set_permissions(HashSet::from([PermissionType::ReadApplicationState]));
    let (sender, receiver) = channels::unbounded();
    env.senders.to_screen = Some(SenderWithContext::new(sender));
    let (finished, completion) = std::sync::mpsc::channel();
    let screen = thread::spawn(move || {
        let (instruction, _) = receiver
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        let yinz_bus::screen::ScreenInstruction::ListNotifications { response_channel } =
            instruction
        else {
            panic!("expected notification history request");
        };
        // Reattachment can exceed the old 100ms deadline; the query must keep waiting.
        assert!(matches!(
            completion.recv_timeout(Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        response_channel.send(vec![]).unwrap();
    });
    let result = dispatch(&mut env, json!("ListNotifications"));
    let _ = finished.send(());
    screen.join().unwrap();
    assert_eq!(result.unwrap(), json!([]));
}

#[test]
fn runtime_is_bound_to_its_executor_thread() {
    let plugin = plugin("(define (render rows columns) \"frame\")");
    let mut plugin = thread::spawn(move || {
        let mut plugin = plugin;
        let error = plugin.render(10, 80).unwrap_err();
        assert!(format!("{error:#}").contains("pinned executor"));
        plugin
    })
    .join()
    .unwrap();
    assert_eq!(plugin.render(10, 80).unwrap(), b"frame");
}

#[test]
fn migrated_builtins_load_through_the_real_host_dispatcher() {
    for &name in yinz_runtime::consts::BUILTIN_PLUGIN_NAMES {
        let mut env = env();
        env.plugin_config = yinz_config::plugins::PluginConfig::from_run_plugin(
            &yinz_types::input::layout::RunPlugin {
                location: RunPluginLocation::Yinz(yinz_types::data::PluginTag::new(name)),
                ..Default::default()
            },
        )
        .unwrap();
        env.data_owner = url::Url::from(&env.plugin_config.location);
        let mut plugin = SteelPlugin::new(
            yinz_steel_builtins::builtin_steel_plugin(name).unwrap(),
            Path::new(name),
            env,
            Arc::default(),
        )
        .unwrap_or_else(|error| panic!("{name} initialization: {error:#}"));
        plugin
            .load(&BTreeMap::new())
            .unwrap_or_else(|error| panic!("{name} load: {error:#}"));
        plugin
            .render(20, 80)
            .unwrap_or_else(|error| panic!("{name} render: {error:#}"));
        plugin.dispose_with_cleanup(true);
    }
}

#[test]
fn callback_context_preserves_and_restores_the_acting_client() {
    let mut env = env();
    let original = env.client_id;
    let reply = dispatch(
        &mut env,
        json!({"__yinz_runtime": {
            "client": 9, "command": "GetPluginIds", "permission_id": null
        }}),
    )
    .unwrap();
    assert_eq!(reply["client_id"], 9);
    assert_eq!(env.client_id, original);
    assert!(dispatch(
        &mut env,
        json!({"__yinz_runtime": {
            "client": 9, "command": {"Subscribe": ["invalid"]}, "permission_id": null
        }})
    )
    .is_err());
    assert_eq!(env.client_id, original);
}

#[test]
fn session_repl_keeps_definitions_and_rejects_non_cli_evaluation() {
    use yinz_bus::ServerInstruction;
    let mut env = env();
    env.plugin_config = yinz_config::plugins::PluginConfig::from_run_plugin(
        &yinz_types::input::layout::RunPlugin {
            location: RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("repl")),
            ..Default::default()
        },
    )
    .unwrap();
    let (sender, receiver) = channels::unbounded();
    env.senders.to_server = Some(SenderWithContext::new(sender));
    let mut plugin = SteelPlugin::new("", Path::new("repl"), env, Arc::default()).unwrap();
    let evaluate = |plugin: &mut SteelPlugin, source: &str| {
        plugin
            .pipe(&PipeMessage::new(
                PipeSource::Cli("repl-test".into()),
                "__yinz_repl_eval",
                &Some(source.into()),
                &None,
                true,
            ))
            .unwrap();
        let (instruction, _) = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        let ServerInstruction::CliPipeOutput(pipe, output) = instruction else {
            panic!("unexpected REPL response: {instruction:?}");
        };
        assert_eq!(pipe, "repl-test");
        serde_json::from_str::<Value>(&output).unwrap()
    };
    assert!(evaluate(&mut plugin, "(define answer 40)")
        .get("error")
        .is_none());
    assert_eq!(
        evaluate(&mut plugin, "(+ answer 2)")["values"],
        json!(["42"])
    );
    plugin
        .pipe(&PipeMessage::new(
            PipeSource::Plugin(yinz_types::internal_ids::PluginId::new(9)),
            "__yinz_repl_eval",
            &Some("(define forbidden 99)".into()),
            &None,
            true,
        ))
        .unwrap();
    assert!(receiver.try_recv().is_err());
    assert!(evaluate(&mut plugin, "forbidden")["error"].is_string());
    assert_eq!(
        evaluate(&mut plugin, "(+ answer 3)")["values"],
        json!(["43"])
    );
    assert!(plugin
        .env()
        .input_pipes_to_unblock
        .lock()
        .unwrap()
        .contains("repl-test"));
}

#[test]
fn unload_drains_replies_already_queued_for_its_generation() {
    use yinz_bus::background_jobs::BackgroundJob;
    use yinz_bus::screen::ScreenInstruction;
    let (jobs, requests) = channels::unbounded();
    let (notifications, queued) = channels::unbounded();
    let (screen, observed) = channels::unbounded();
    let mut env = env();
    env.set_permissions([PermissionType::RunCommands].into());
    env.senders.to_background_jobs = Some(SenderWithContext::new(jobs));
    env.senders.to_plugin = Some(SenderWithContext::new(notifications));
    env.senders.to_screen = Some(SenderWithContext::new(screen));
    let senders = env.senders.clone();
    let mut plugin = SteelPlugin::new(
        r#"
      (define pending #f)
      (define (load config)
        (set! pending (yinz-run-command (hash "command" (list "/usr/bin/true")))))
      (define (unload)
        (yinz-then pending
          (lambda (reply)
            (unless (= (hash-ref reply "exit-code") 0) (error "command failed"))
            (yinz-command (hash "SetSelectable" #f)))))
    "#,
        Path::new("cleanup.scm"),
        env,
        Arc::default(),
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let (id, client, generation, context) = loop {
        if let BackgroundJob::RunCommand(id, client, _, _, _, _, context, _, _, generation) =
            requests.recv_timeout(Duration::from_secs(10)).unwrap().0
        {
            break (id, client, generation.unwrap(), context);
        }
    };
    senders
        .send_to_plugin(PluginInstruction::for_plugin_event(
            id,
            client,
            Some(generation),
            Event::RunCommandResult(Some(0), vec![], vec![], context),
        ))
        .unwrap();
    let reply = loop {
        if let PluginInstruction::PluginReply(_, _, _, Event::RunCommandResult(..), reply) =
            queued.recv_timeout(Duration::from_secs(10)).unwrap().0
        {
            break reply;
        }
    };
    plugin.dispose_with_cleanup(true);
    assert!(observed
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ScreenInstruction::SetSelectable(_, false))));
    assert!(reply.claim().is_none());
}

#[test]
fn unload_deadline_bounds_both_pending_deferreds_and_guest_loops() {
    for body in ["(yinz-deferred)", "(let loop () (loop))"] {
        let mut plugin = plugin(&format!("(define (unload) {body})"));
        let started = std::time::Instant::now();
        let generation = plugin.generation();
        plugin.dispose_with_cleanup(true);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "unload was not bounded"
        );
        assert!(RUNTIMES.with(|runtimes| !runtimes.borrow().contains_key(&generation)));
    }
}

#[test]
fn steel_diagnostics_are_drained_without_becoming_rendered_content() {
    let mut plugin = plugin(
        r#"
        (yinz-log "initialization diagnostic")
        (define (load config) (yinz-log "load diagnostic"))
        (define (render rows columns)
          (yinz-log "render diagnostic")
          (display "visible frame"))
        "#,
    );
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(plugin.render(10, 80).unwrap(), b"visible frame");
    assert!(plugin
        .with_runtime(false, |runtime| runtime.take_logs())
        .unwrap()
        .is_empty());
    plugin.dispose_with_cleanup(true);
}

#[test]
fn steel_load_schedules_its_initial_render_while_permissions_are_unanswered() {
    let (sender, receiver) = channels::unbounded();
    let mut env = env();
    env.senders.to_plugin = Some(SenderWithContext::new(sender));
    let mut plugin = SteelPlugin::new(
        r#"
        (define (load config)
          (yinz-command (hash "RequestPluginPermissions" '("PluginData")))
          (yinz-request-render))
        (define (render rows columns) "waiting for permission")
        "#,
        Path::new("test.scm"),
        env,
        Arc::default(),
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(receiver.try_iter().filter(|(instruction, _)| matches!(instruction,
        PluginInstruction::PluginTick(id, client, generation)
            if *id == plugin.env().plugin_id && *client == plugin.env().client_id && *generation == plugin.generation()
    )).count(), 1);
    assert!(plugin.tick().unwrap());
    assert!(!plugin.tick().unwrap());
    assert_eq!(plugin.render(2, 40).unwrap(), b"waiting for permission");
    plugin.dispose_with_cleanup(true);
}

#[test]
fn cell_pixel_size_dispatch_uses_acting_client_without_permissions() {
    use yinz_bus::screen::ScreenInstruction;
    use yinz_types::pane_size::SizeInPixels;

    let mut env = env();
    let owner = env.client_id;
    let (sender, receiver) = channels::unbounded();
    env.senders.to_screen = Some(SenderWithContext::new(sender));
    let responder = std::thread::spawn(move || {
        for size in [
            Some(SizeInPixels {
                width: 9,
                height: 18,
            }),
            None,
        ] {
            let (instruction, _) = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            let ScreenInstruction::GetCellPixelSize {
                client_id,
                response_channel,
            } = instruction
            else {
                panic!("unexpected instruction: {instruction:?}");
            };
            assert_eq!(client_id, yinz_types::internal_ids::ClientId::new(9));
            response_channel.send(size).unwrap();
        }
    });
    for expected in [json!({"width": 9, "height": 18}), Value::Null] {
        let result = dispatch(
            &mut env,
            json!({"__yinz_runtime": {
                "client": 9, "command": "GetCellPixelSize", "permission_id": null
            }}),
        )
        .unwrap();
        assert_eq!(result, expected);
        assert_eq!(env.client_id, owner);
    }
    responder.join().unwrap();
}
