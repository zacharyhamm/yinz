use super::*;

/// Mid-planning-turn, as `start_request` would have left it.
fn planning_state() -> State {
    State {
        plan: Plan::Planning,
        ..ready_state()
    }
}

/// A settled plain reply, which is what a finished plan looks like.
fn reply_and_settle(state: &mut State, text: &str) {
    let frame = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n");
    state.on_chunk("hocus-1", frame.as_bytes(), false, None);
    state.on_chunk("hocus-1", b"", true, None);
}

// -- the toggle ---------------------------------------------------------

/// `/plan` drives the plugin, not the conversation: nothing is sent, and it is
/// not run as a tool named `plan`.
#[test]
fn slash_plan_toggles_the_mode_without_sending_anything() {
    let mut state = State::default();
    state.input.set_text("/plan".to_owned());
    state.submit();

    assert!(state.is_planning());
    assert!(state.stream.is_none(), "nothing was sent");
    assert!(
        state.messages.iter().all(|m| m.tool_calls.is_empty()),
        "no tool was run",
    );
    assert_eq!(state.messages.last().unwrap().role, Role::Notice);

    state.input.set_text("/plan".to_owned());
    state.submit();
    assert!(matches!(state.plan, Plan::Off), "the second one leaves");
}

/// `/plan off` must not fall through to `parse_command` and be answered with
/// "no such tool: plan".
#[test]
fn the_explicit_forms_are_idempotent() {
    let mut state = State::default();
    for line in ["/plan on", "/plan on", "/plan off", "/plan off"] {
        state.input.set_text(line.to_owned());
        state.submit();
    }
    assert!(matches!(state.plan, Plan::Off));
    assert!(
        state.messages.iter().all(|m| m.tool_calls.is_empty()),
        "`plan` was never taken for a tool",
    );
}

/// Turning the mode on while a destructive call waits on a y/n would otherwise
/// leave that call armed inside the mode that exists to disarm it.
#[test]
fn switching_to_planning_declines_a_pending_call() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert!(matches!(state.status, Status::ConfirmingTool));

    state.input.set_text("/plan".to_owned());
    state.submit();

    assert!(state.is_planning());
    assert!(!matches!(state.status, Status::ConfirmingTool));
    let answered = tool_results(&state);
    assert_eq!(answered.len(), 1, "the call the model made was answered");
    assert_eq!(answered[0].content, "declined by the user");
}

/// Declining the pending call empties the queue, and an emptied queue starts the
/// model's follow-up request. That request must already be a planning one — the
/// mode has to be set before the call is answered, not after.
#[test]
fn the_follow_up_after_that_decline_is_already_a_planning_request() {
    // A two-call batch parked on the first one's y/n, as `pump_tools` leaves it.
    let calls: Vec<ToolCall> = [("call_1", "clear_pane", 1), ("call_2", "close_pane", 2)]
        .into_iter()
        .map(|(id, name, pane)| ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: format!(r#"{{"pane_id":{pane},"is_plugin":false}}"#),
        })
        .collect();
    let mut state = State {
        stream: None,
        reply_at: None,
        status: Status::ConfirmingTool,
        owes_reply: true,
        ..ready_state()
    };
    state.messages.push(Message::tool_calls(calls.clone()));
    state.pending.extend(calls);

    state.input.set_text("/plan".to_owned());
    state.submit();

    assert!(state.is_planning());
    let results = tool_results(&state);
    assert_eq!(results.len(), 2, "every call the model made was answered");
    assert!(
        results[1]
            .content
            .starts_with("refused: planning mode is on"),
        "the rest of the batch met the gate rather than running: {}",
        results[1].content,
    );
}

// -- enforcement --------------------------------------------------------

/// The tool array is the real gate, but a model can name a tool it was never
/// offered. It gets a result saying no, rather than the pane being closed.
#[test]
fn a_mutating_call_is_refused_while_planning() {
    let mut state = planning_state();
    let frame = tool_call_frame("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .content
            .starts_with("refused: planning mode is on"),
        "{}",
        results[0].content,
    );
    assert!(
        !matches!(state.status, Status::ConfirmingTool),
        "it was refused, not put up for a y/n",
    );
}

/// Typing a command is the user acting, not the model slipping the mode.
#[test]
fn a_typed_tool_still_runs_while_planning() {
    let mut state = State {
        plan: Plan::Planning,
        ..State::default()
    };
    state
        .input
        .set_text("/close_pane pane_id=1 is_plugin=false".to_owned());
    state.submit();

    assert_eq!(state.messages[0].tool_calls.len(), 1);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        !results[0].content.starts_with("refused:"),
        "{}",
        results[0].content,
    );
}

// -- approve to execute -------------------------------------------------

#[test]
fn a_finished_plan_asks_to_be_carried_out() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "First I would do this.");

    assert!(state.awaiting_plan());
    assert!(state.confirmation_prompt().is_some(), "the prompt is up");
    assert!(
        !state.is_in_flight(),
        "nothing is running, so nothing may queue behind it",
    );
}

/// A turn that ended in tool calls is still researching.
#[test]
fn a_turn_that_ended_in_tool_calls_keeps_planning() {
    let mut state = planning_state();
    let frame = tool_call_frame("call_1", "read_file", r#"{"path":"x"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(state.is_planning());
    assert!(!state.awaiting_plan());
}

/// The approval has to reach the model as a user turn: a notice is dropped on
/// the way out, which would leave two assistant turns in a row and nothing
/// asking for the second.
#[test]
fn y_leaves_the_mode_and_carries_the_plan_out() {
    let mut state = planning_state();
    state.tool_rounds = 4;
    reply_and_settle(&mut state, "The plan.");
    state.approve_plan();

    assert!(matches!(state.plan, Plan::Off));
    assert_eq!(state.tool_rounds, 0, "execution gets its own budget");
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .expect("the approval is a user turn");
    assert!(last_user.content.contains("Carry out the plan"));
}

#[test]
fn n_keeps_planning() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.decline_plan();

    assert!(state.is_planning(), "still planning");
    assert!(!state.awaiting_plan());
    assert_eq!(state.messages.last().unwrap().role, Role::Notice);
}

/// Typing at a plan is how you correct one, so the question stops being asked
/// and the text goes out as the next planning turn — never as `queued: …`.
#[test]
fn typing_at_a_plan_answers_it_rather_than_queueing() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("do it the other way".to_owned());
    state.submit();

    assert!(state.asks.is_empty(), "nothing was queued");
    assert!(state.is_planning(), "still planning, with the correction");
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .unwrap();
    assert_eq!(last_user.content, "do it the other way");
}

/// Esc only ever puts hocus away. A plan waiting for an answer is not an answer,
/// so it is still waiting when the pane comes back - the same treatment a tool
/// call waiting on y/n gets.
#[test]
fn esc_hides_and_leaves_the_plan_waiting() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.on_key(KeyWithModifier::new(BareKey::Esc));

    assert!(state.awaiting_plan(), "the approval is still outstanding");
    assert!(
        !state.messages.iter().any(|m| m.content == "tools aborted"),
        "there was nothing to abort",
    );
}

/// Ctrl-c is the interrupt, and a waiting plan is the only outstanding question
/// while one waits - so it answers that rather than falling through to a tool
/// queue that is empty.
#[test]
fn ctrl_c_declines_the_plan_and_aborts_nothing() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.on_key(KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier());

    assert!(state.is_planning(), "back to planning, not carrying it out");
    assert!(
        !state.messages.iter().any(|m| m.content == "tools aborted"),
        "there was nothing to abort",
    );
}

/// A job coming due while a plan waits must start its own turn rather than be
/// mistaken for an approval of the plan.
#[test]
fn a_job_firing_does_not_approve_the_plan() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.jobs.push(Job {
        id: 1,
        every: 5.0,
        due_in: 0.0,
        prompt: "check".to_owned(),
        show: false,
        auto_confirm: false,
    });
    state.tick_jobs(1.0);

    assert!(state.is_planning(), "the job runs inside the mode");
    assert!(!state.awaiting_plan());
    assert!(
        !state
            .messages
            .iter()
            .any(|m| m.content.contains("Carry out the plan")),
        "the plan was not approved by a timer",
    );
}

// -- where the mode lives -----------------------------------------------

#[test]
fn a_new_chat_leaves_planning_mode() {
    let mut state = planning_state();
    state.new_chat();
    assert!(matches!(state.plan, Plan::Off));
}

/// A chat parked mid-plan is still owed an answer, so it must not be archived
/// and dropped — and it comes back planning.
#[test]
fn a_parked_chat_keeps_its_mode() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");

    let parked = state.take_chat();
    assert!(parked.is_live(), "a plan awaiting an answer is still live");
    state.put_chat(parked);
    assert!(state.awaiting_plan(), "and it comes back asking");
}

/// A `y`/`n` typed as the start of a correction must reach the input box, not
/// be read as an answer to the approval prompt.
#[test]
fn ynot_composing_feedback_is_not_approval() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("you should reconsider".to_owned());

    state.on_key(KeyWithModifier::new(BareKey::Char('y')));

    assert!(
        state.awaiting_plan(),
        "the plan is still waiting, not approved out from under the user",
    );
    assert_eq!(
        state.input.get_text(),
        "you should reconsidery",
        "the keystroke reached the editor rather than being swallowed"
    );
}

/// Pressing Enter on an empty box while a plan awaits approval must leave the
/// prompt up, not silently drop back into planning mode.
#[test]
fn empty_enter_does_not_dismiss_the_approval_prompt() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("   ".to_owned());

    state.submit();

    assert!(
        state.awaiting_plan(),
        "an empty submission is not a correction and must not clear the prompt",
    );
}

/// A chat that is merely `Planning` — no approval pending yet — must stay
/// live too, or switching away from it loses the mode entirely.
#[test]
fn a_parked_planning_chat_keeps_its_mode_too() {
    let state = planning_state();

    let chat = Chat {
        messages: state.messages.clone(),
        plan: state.plan,
        ..Chat::default()
    };
    assert!(
        chat.is_live(),
        "a chat still in planning mode is owed the rest of its plan",
    );
}

/// `/plan` and the five `plugin_*` tools share a prefix, so completion must not
/// silently pick one of them for the other.
#[test]
fn plan_completes_without_colliding_with_the_plugin_tools() {
    let mut state = State::default();
    state.input.set_text("/pla".to_owned());
    state.complete_tool_name();
    assert_eq!(
        state.input.get_text(),
        "/plan ",
        "an exact prefix completes"
    );

    let mut state = State::default();
    state.input.set_text("/pl".to_owned());
    state.complete_tool_name();
    assert_eq!(state.input.get_text(), "/pl", "an ambiguous one is listed");
    let listed = state.messages.last().expect("the matches were listed");
    assert!(listed.content.contains("/plan"), "{}", listed.content);
    assert!(
        listed.content.contains("/plugin_read"),
        "{}",
        listed.content
    );
}
