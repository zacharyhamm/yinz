use super::*;

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    }
}

fn extension_state(safety: &str, consequence: Option<&str>) -> State {
    let mut state = State::default();
    let path = "/plugins/hocus/weather.lua";
    state.user_plugin_dir = "/plugins".to_owned();
    state
        .extension_files
        .insert("weather".to_owned(), path.to_owned());
    state.extension_instances.insert(
        zellij_tile::prelude::PluginId::new(7),
        ("weather".to_owned(), path.to_owned()),
    );
    let mut tool = serde_json::json!({
        "name": "forecast",
        "description": "Read the forecast.",
        "parameters": { "type": "object", "properties": {} },
        "safety": safety,
    });
    if let Some(consequence) = consequence {
        tool["consequence"] = serde_json::json!(consequence);
    }
    let payload = serde_json::json!({
        "version": crate::extensions::VERSION,
        "namespace": "weather",
        "path": path,
        "tools": [tool],
    })
    .to_string();
    state.on_extension_registration(zellij_tile::prelude::PluginId::new(7), Some(&payload));
    state
}

#[test]
fn an_extension_call_waits_for_its_registered_owner_and_finishes_normally() {
    let mut state = extension_state("automatic", None);
    state.own_plugin_id = zellij_tile::prelude::PluginId::new(3);
    let call = call("call_1", "weather_forecast", r#"{"city":"Chicago"}"#);
    state.pending.push_back(call.clone());

    assert!(!state.run_head(&call));
    assert!(matches!(state.status, Status::AwaitingExtension));
    assert!(tool_results(&state).is_empty());

    let payload = serde_json::json!({
        "version": crate::extensions::VERSION,
        "call_id": "call_1",
        "result": "sunny",
    })
    .to_string();
    assert!(
        !state.on_extension_result(zellij_tile::prelude::PluginId::new(8), Some(&payload)),
        "a spoofed owner answered"
    );
    assert!(state.on_extension_result(zellij_tile::prelude::PluginId::new(7), Some(&payload)));
    assert!(matches!(state.status, Status::Idle));
    assert_eq!(tool_results(&state)[0].content, "sunny");
}

/// The reply is the rpc wire's: a `result` from a handler that returned, an `error` from one
/// that raised or from a name nothing handles - and either way the model is answered.
#[test]
fn an_extension_error_reaches_the_model_named_as_a_failure() {
    let mut state = extension_state("automatic", None);
    let call = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(call.clone());
    assert!(!state.run_head(&call));
    let payload = serde_json::json!({
        "version": crate::extensions::VERSION,
        "call_id": "call_1",
        "error": "no forecast today",
    })
    .to_string();
    assert!(state.on_extension_result(zellij_tile::prelude::PluginId::new(7), Some(&payload)));
    assert_eq!(tool_results(&state)[0].content, "failed: no forecast today");
}

/// A call nobody answers is given up on after `CALL_TIMEOUT_MS`, and the model told why,
/// rather than hocus waiting for a Ctrl-c.
#[test]
fn an_unanswered_extension_call_times_out() {
    let mut state = extension_state("automatic", None);
    let call = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(call.clone());
    assert!(!state.run_head(&call));
    assert!(matches!(state.status, Status::AwaitingExtension));

    state.timer_armed = false;
    state.on_timer(1.0);
    assert!(tool_results(&state).is_empty(), "a second is not a timeout");

    state.timer_armed = false;
    state.on_timer((crate::extensions::CALL_TIMEOUT_MS as f64) / 1000.0);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("did not answer"),
        "{:?}",
        results[0].content
    );
    assert!(matches!(state.status, Status::Idle));
}

/// The results of a parked chat's tools, which `tool_results` only reads off the foreground.
fn parked_tool_results(state: &State, index: usize) -> Vec<&Message> {
    state.parked[index]
        .messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .collect()
}

/// The clock travels with the chat: a call sent by a chat that was then parked is given up on
/// where it waits, and the chat on screen - which sent nothing - hears nothing of it.
#[test]
fn an_extension_call_awaited_by_a_parked_chat_still_times_out() {
    let mut state = extension_state("automatic", None);
    let call = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(call.clone());
    assert!(!state.run_head(&call));
    assert!(matches!(state.status, Status::AwaitingExtension));

    // `/history` moving to another conversation, which leaves an idle chat in front.
    let waiting = state.take_chat();
    state.parked.push(waiting);
    assert!(matches!(state.status, Status::Idle));
    assert!(
        state.extension_call_started_ms.is_none(),
        "the clock went with the chat that sent the call"
    );

    state.timer_armed = false;
    state.on_timer(1.0);
    assert!(
        parked_tool_results(&state, 0).is_empty(),
        "a second is not a timeout"
    );
    assert!(
        state.timer_armed,
        "the chain keeps running for the parked chat's call"
    );

    state.timer_armed = false;
    state.on_timer((crate::extensions::CALL_TIMEOUT_MS as f64) / 1000.0);
    assert!(tool_results(&state).is_empty(), "not this chat's call");
    let results = parked_tool_results(&state, 0);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("did not answer"),
        "{:?}",
        results[0].content
    );
    assert!(matches!(state.parked[0].status, Status::Idle));
}

/// Two chats mid-call each keep their own clock: the older call times out on its own
/// schedule, and the younger one is left waiting until its own time is up.
#[test]
fn two_chats_extension_calls_keep_their_own_clocks() {
    let mut state = extension_state("automatic", None);
    let first = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(first.clone());
    assert!(!state.run_head(&first));
    let waiting = state.take_chat();
    state.parked.push(waiting);

    // Half the timeout later, the foreground chat sends its own call.
    state.timer_armed = false;
    state.on_timer((crate::extensions::CALL_TIMEOUT_MS as f64) / 2000.0);
    let second = call("call_2", "weather_forecast", "{}");
    state.pending.push_back(second.clone());
    assert!(!state.run_head(&second));
    assert!(matches!(state.status, Status::AwaitingExtension));

    // The rest of the first call's timeout: only the first call is given up on.
    state.timer_armed = false;
    state.on_timer((crate::extensions::CALL_TIMEOUT_MS as f64) / 2000.0);
    assert_eq!(parked_tool_results(&state, 0).len(), 1);
    assert!(
        tool_results(&state).is_empty(),
        "the second call has time left"
    );
    assert!(matches!(state.status, Status::AwaitingExtension));

    // And the rest of the second's.
    state.timer_armed = false;
    state.on_timer((crate::extensions::CALL_TIMEOUT_MS as f64) / 2000.0);
    assert_eq!(tool_results(&state).len(), 1);
    assert!(matches!(state.status, Status::Idle));
}

/// The owner leaving answers a parked chat's call as it does the foreground's.
#[test]
fn an_extension_going_away_answers_a_parked_chats_call_too() {
    let mut state = extension_state("automatic", None);
    let call = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(call.clone());
    assert!(!state.run_head(&call));
    let waiting = state.take_chat();
    state.parked.push(waiting);

    state.on_sessions(vec![SessionInfo {
        is_current_session: true,
        plugins: BTreeMap::new(),
        ..SessionInfo::default()
    }]);
    assert!(tool_results(&state).is_empty(), "not this chat's call");
    let results = parked_tool_results(&state, 0);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("went away"),
        "{:?}",
        results[0].content
    );
    assert!(matches!(state.parked[0].status, Status::Idle));
}

/// An extension that unloads mid-call will never answer: the session's next plugin list says
/// its owner is gone, and the call is answered with that.
#[test]
fn an_extension_going_away_answers_the_call_it_was_asked() {
    let mut state = extension_state("automatic", None);
    let call = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(call.clone());
    assert!(!state.run_head(&call));

    state.on_sessions(vec![SessionInfo {
        is_current_session: true,
        plugins: BTreeMap::new(),
        ..SessionInfo::default()
    }]);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("went away"),
        "{:?}",
        results[0].content
    );
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn extension_registration_is_bound_to_the_running_plugin_instance() {
    let path = "/plugins/hocus/weather.lua";
    let mut state = State::default();
    state
        .extension_files
        .insert("weather".to_owned(), path.to_owned());
    state.on_sessions(vec![SessionInfo {
        is_current_session: true,
        plugins: BTreeMap::from([(
            PluginId::new(7),
            PluginInfo {
                location: format!("file:{path}"),
                configuration: BTreeMap::from([
                    ("hocus_extension_name".to_owned(), "weather".to_owned()),
                    ("hocus_extension_path".to_owned(), path.to_owned()),
                ]),
            },
        )]),
        ..SessionInfo::default()
    }]);

    assert_eq!(
        state.extension_instances.get(&PluginId::new(7)),
        Some(&("weather".to_owned(), path.to_owned()))
    );
    assert!(!state.extension_instances.contains_key(&PluginId::new(8)));
}

#[test]
fn extension_results_are_bounded_before_deserialization() {
    let mut state = extension_state("automatic", None);
    let call = call("call_1", "weather_forecast", "{}");
    state.pending.push_back(call.clone());
    assert!(!state.run_head(&call));

    let payload = serde_json::json!({
        "version": crate::extensions::VERSION,
        "call_id": "call_1",
        "result": "x".repeat(crate::extensions::MAX_RESULT_BYTES),
    })
    .to_string();
    assert!(payload.len() > crate::extensions::MAX_RESULT_BYTES);
    assert!(!state.on_extension_result(zellij_tile::prelude::PluginId::new(7), Some(&payload)));
    assert!(matches!(state.status, Status::AwaitingExtension));
    assert!(tool_results(&state).is_empty());
}

#[test]
fn extension_arguments_must_be_json_objects() {
    for arguments in ["null", "[]", r#""Chicago""#] {
        let mut state = extension_state("automatic", None);
        let call = call("call_1", "weather_forecast", arguments);
        state.pending.push_back(call.clone());

        assert!(state.run_head(&call));
        assert!(matches!(state.status, Status::Idle));
        assert!(tool_results(&state)[0]
            .content
            .contains("invalid arguments"));
    }
}

#[test]
fn extension_safety_drives_planning_and_confirmation() {
    let read = extension_state("read_only", None);
    assert!(read.tool_is_read_only("weather_forecast"));
    assert!(!read.call_needs_confirmation(&call("1", "weather_forecast", "{}")));

    let automatic = extension_state("automatic", None);
    assert!(!automatic.tool_is_read_only("weather_forecast"));
    assert!(!automatic.call_needs_confirmation(&call("1", "weather_forecast", "{}")));

    let mut confirming = extension_state("confirm", Some("contacts the weather service"));
    let call = call("1", "weather_forecast", r#"{"city":"Chicago"}"#);
    assert!(confirming.call_needs_confirmation(&call));
    confirming.pending.push_back(call);
    confirming.status = Status::ConfirmingTool;
    assert_eq!(
        confirming.confirmation_prompt().unwrap().detail,
        vec![
            ("contacts the weather service".to_owned(), true),
            ("city: Chicago".to_owned(), false),
        ]
    );
}

#[test]
fn extension_confirmation_toasts_bound_source_arguments() {
    let state = extension_state("confirm", Some("writes the forecast extension"));
    let source: Vec<String> = (0..50).map(|n| format!("-- line {n}")).collect();
    let toast = state.confirmation_toast(&call(
        "1",
        "weather_forecast",
        &serde_json::json!({ "source": source.join("\n") }).to_string(),
    ));
    assert!(toast.contains("writes the forecast extension"), "{toast}");
    assert!(toast.contains("30 more lines"), "{toast}");
    assert!(!toast.contains("-- line 49"), "{toast}");
}

#[test]
fn extension_specs_join_normal_requests_and_read_only_specs_join_plans() {
    let mut state = extension_state("read_only", None);
    let read_only = state.extensions.specs(true);
    assert_eq!(read_only.len(), 1);

    state = extension_state("automatic", None);
    assert!(state.extensions.specs(true).is_empty());
    assert_eq!(state.extensions.specs(false).len(), 1);
}

#[test]
fn a_scan_waits_before_loading_files_that_did_not_reregister() {
    let mut state = State {
        user_plugin_dir: "/plugins".to_owned(),
        ..State::default()
    };
    state.on_extension_scan_result(
        Some(0),
        b"/plugins/hocus/weather.lua\n/plugins/ordinary.lua\n",
    );
    assert_eq!(
        state.extension_files.get("weather").map(String::as_str),
        Some("/plugins/hocus/weather.lua")
    );
    assert!(state.extension_scan_due_ms.is_some());
    state.timer_armed = false;
    state.on_timer((PLUGIN_LOAD_MS as f64) / 1000.0);
    assert!(state.extension_scan_due_ms.is_none());
}

/// The answer a `plugin_reload` of an extension gets. Registration is the extension saying what
/// it offers, and the churned todo session read exactly this line as proof its tools worked - so
/// the line has to ask for them to be called.
#[test]
fn a_registration_report_names_the_tools_and_asks_for_them_to_be_called() {
    let path = "/plugins/hocus/weather.lua";
    let mut state = State {
        user_plugin_dir: "/plugins".to_owned(),
        ..State::default()
    };
    state
        .extension_files
        .insert("weather".to_owned(), path.to_owned());
    state.extension_instances.insert(
        zellij_tile::prelude::PluginId::new(7),
        ("weather".to_owned(), path.to_owned()),
    );
    state.pending.push_back(call(
        "call_1",
        "plugin_reload",
        r#"{"name":"hocus/weather"}"#,
    ));
    state.status = Status::AwaitingPluginLoad;
    state.plugin_load = Some(PluginLoadWatch {
        target: path.to_owned(),
        started_ms: 0,
        extension: true,
        headless: false,
        name: "hocus/weather".to_owned(),
    });

    let payload = serde_json::json!({
        "version": crate::extensions::VERSION,
        "namespace": "weather",
        "path": path,
        "tools": [{
            "name": "forecast",
            "description": "Read the forecast.",
            "parameters": { "type": "object", "properties": {} },
            "safety": "read_only",
        }],
    })
    .to_string();
    assert!(state.on_extension_registration(zellij_tile::prelude::PluginId::new(7), Some(&payload)));

    let results = tool_results(&state);
    let answer = results
        .last()
        .expect("the reload was answered")
        .content
        .clone();
    assert!(answer.contains("weather_forecast"), "{answer}");
    assert!(answer.contains("Call each one once"), "{answer}");
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn a_duplicate_scan_revokes_registration_and_finishes_pending_calls() {
    let mut state = extension_state("automatic", None);
    state.user_plugin_dir = "/plugins".into();
    state.extension_instances.insert(
        zellij_tile::prelude::PluginId::new(7),
        ("weather".into(), "/plugins/hocus/weather.lua".into()),
    );
    let request = call("pending", "weather_forecast", "{}");
    state.pending.push_back(request.clone());
    assert!(!state.run_head(&request));
    state.on_extension_scan_result(
        Some(0),
        b"/plugins/hocus/weather.lua\n/plugins/hocus/weather.ts\n",
    );
    assert!(state.extensions.tool("weather_forecast").is_none());
    assert!(state.extension_files.is_empty());
    assert!(tool_results(&state)[0].content.contains("went away"));
}
