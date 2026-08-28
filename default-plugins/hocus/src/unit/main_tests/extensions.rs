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
    state.on_extension_registration(7, Some(&payload));
    state
}

#[test]
fn an_extension_call_waits_for_its_registered_owner_and_finishes_normally() {
    let mut state = extension_state("automatic", None);
    state.own_plugin_id = 3;
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
        !state.on_extension_result(8, Some(&payload)),
        "a spoofed owner answered"
    );
    assert!(state.on_extension_result(7, Some(&payload)));
    assert!(matches!(state.status, Status::Idle));
    assert_eq!(tool_results(&state)[0].content, "sunny");
}

#[test]
fn extension_safety_drives_planning_and_confirmation() {
    let read = extension_state("read_only", None);
    assert!(read.tool_is_read_only("weather_forecast"));
    assert!(!read.call_needs_confirmation(&call("1", "weather_forecast", "{}")));

    let automatic = extension_state("automatic", None);
    assert!(!automatic.tool_is_read_only("weather_forecast"));
    assert!(!automatic.call_needs_confirmation(&call("1", "weather_forecast", "{}")));

    let confirming = extension_state("confirm", Some("contacts the weather service"));
    let call = call("1", "weather_forecast", "{}");
    assert!(confirming.call_needs_confirmation(&call));
    assert!(confirming
        .confirmation_detail(&call)
        .iter()
        .any(|line| line.contains("contacts the weather service")));
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
