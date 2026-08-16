use super::*;

// -- manual `/tool` invocation ------------------------------------------

/// The call-and-result pair is the whole point: an orphan result makes the
/// next request malformed, and the model would never see what was run. The
/// default state is deliberately unconfigured — this must not need an API.
#[test]
fn a_slash_command_runs_the_tool_and_sends_nothing() {
    let mut state = State::default();
    state.input.set_text("/list_session".to_owned());
    state.submit();

    assert!(matches!(state.status, Status::Idle), "no request fired");
    assert!(!state.owes_reply && state.pending.is_empty());
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[0].tool_calls.len(), 1);
    assert_eq!(state.messages[0].tool_calls[0].name, "list_session");
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_call_id,
        Some(state.messages[0].tool_calls[0].id.clone()),
        "the result answers the call"
    );
}

/// A tool the user typed is shown as prose and still recorded as the raw
/// result the model reads. Driven through `finish_head` rather than through
/// `submit`, because off-wasm `tools::dispatch` answers "tools are
/// unavailable outside zellij" and there is nothing structured to present.
#[test]
fn a_typed_tools_result_is_presented_without_changing_what_is_sent() {
    let mut state = State::default();
    let call = ToolCall {
        id: "manual-0".to_owned(),
        name: "new_pane".to_owned(),
        arguments: "{}".to_owned(),
    };
    state.pending.push_back(call);
    state.manual_head = true;
    state.finish_head(r#"{"opened":true,"pane_id":7}"#);

    let result = state.messages.last().expect("a result was recorded");
    assert_eq!(
        result.content, r#"{"opened":true,"pane_id":7}"#,
        "the model still reads the raw result"
    );
    assert_eq!(
        result.display.as_deref(),
        Some("Opened pane `7`.\n"),
        "the pane shows the prose"
    );
}

/// The model's own calls are context, not answers, and are left alone.
#[test]
fn a_models_tool_result_is_not_presented() {
    let mut state = State::default();
    state.pending.push_back(ToolCall {
        id: "call_1".to_owned(),
        name: "new_pane".to_owned(),
        arguments: "{}".to_owned(),
    });
    state.finish_head(r#"{"opened":true,"pane_id":7}"#);
    assert_eq!(state.messages.last().expect("a result").display, None);
}

/// Typing the command is the confirmation, so the y/n prompt is skipped.
#[test]
fn a_destructive_slash_command_does_not_ask() {
    let mut state = State::default();
    state
        .input
        .set_text("/close_pane pane_id=1 is_plugin=false".to_owned());
    state.submit();

    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(
        state.messages[0].tool_calls[0].arguments, r#"{"is_plugin":false,"pane_id":1}"#,
        "the pairs came out typed, not as strings"
    );
}

#[test]
fn a_malformed_slash_command_is_an_error_not_a_call() {
    let mut state = State::default();
    state.input.set_text("/read_pane 3".to_owned());
    state.submit();

    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].role, Role::Error);
    assert!(state.pending.is_empty());
}

#[test]
fn tab_completes_a_unique_tool_name() {
    let mut state = State::default();
    state.input.set_text("/list_ses".to_owned());
    state.complete_tool_name();
    assert_eq!(state.input.get_text(), "/list_session ");

    // Ambiguous, and ordinary text, both leave the buffer alone.
    state.input.set_text("/new_".to_owned());
    state.complete_tool_name();
    assert_eq!(state.input.get_text(), "/new_");
    state.input.set_text("hello".to_owned());
    assert!(!state.complete_tool_name());
}

#[test]
fn tab_completes_a_mention_from_the_agents_on_screen() {
    let mut state = state_with_panes();
    state.manual_kinds.insert(4, "reviewer".to_owned());

    state.input.set_text("ask @cod".to_owned());
    assert!(state.complete_tool_name());
    assert_eq!(state.input.get_text(), "ask @codex ");
    assert_eq!(state.input.get_cursor_position(), 11);

    // Mid-message: only the token at the cursor completes; the tail stays.
    state.input.set_text("ask @rev about pane 3".to_owned());
    state.input.set_cursor_position(8);
    assert!(state.complete_tool_name());
    assert_eq!(state.input.get_text(), "ask @reviewer  about pane 3");
    assert_eq!(state.input.get_cursor_position(), 14);

    // A bare @ lists every candidate; the buffer is left alone.
    state.input.set_text("@".to_owned());
    assert!(state.complete_tool_name());
    assert_eq!(state.input.get_text(), "@");
    assert_eq!(
        state.messages.last().unwrap().content,
        "@codex  @reviewer  @3  @4",
    );
}

#[test]
fn a_submitted_mention_carries_the_agent_context_with_it() {
    let mut state = ready_state();
    settle(&mut state);
    state.set_panes(terminal_panes(
        &manifest(vec![pane_info(3, "codex", Some("codex"))]),
        &[tab_info(0, 9, "work")],
    ));
    state
        .input
        .set_text("ask @codex to run the tests".to_owned());
    state.submit();

    let user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .unwrap();
    assert_eq!(
        user.content,
        "ask @codex to run the tests\n\n[@codex is agent pane 3 (codex, running) on tab \"work\"]",
    );
}

/// Each list stands alone behind its own command; Tab no longer cycles them,
/// so in a picker it is just another swallowed key.
#[test]
fn slash_commands_open_the_lists_and_tab_stays_put() {
    let mut state = fetching_models();
    assert!(matches!(picker(&state).kind, PickerKind::Models));
    assert!(picker(&state).entries.is_empty(), "still fetching");

    state.picker_key(KeyWithModifier::new(BareKey::Tab));
    assert!(matches!(picker(&state).kind, PickerKind::Models));

    state.picker = None;
    state.run_manual("/history");
    assert!(matches!(picker(&state).kind, PickerKind::Chats(_)));

    state.picker_key(KeyWithModifier::new(BareKey::Tab));
    assert!(matches!(picker(&state).kind, PickerKind::Chats(_)));

    state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier());
    assert!(
        state.picker.is_none(),
        "handing focus to the agents pane dismisses the temporary list"
    );
}

#[test]
fn slash_keys_prints_the_keybinding_reference() {
    let mut state = idle_state();
    assert!(state.run_manual("/keys"));
    let notice = state.messages.last().unwrap();
    assert_eq!(notice.role, Role::Notice);
    assert!(notice.content.contains("Ctrl+l"));
    assert!(notice.content.contains("new chat"));
}
