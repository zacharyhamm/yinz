use super::*;

/// Mid-planning-turn, as `start_request` would have left it.
fn planning_state() -> State {
    State {
        plan: Plan::Planning,
        ..ready_state()
    }
}

/// A settled chat with a usable config: what an idle hocus looks like to
/// something typed into it. `ready_state` is mid-stream, so a prompt sent
/// against it would queue rather than go out.
fn idle_state() -> State {
    State {
        stream: None,
        reply_at: None,
        messages: Vec::new(),
        ..ready_state()
    }
}

/// A settled plain reply, which is what a finished plan looks like.
fn reply_and_settle(state: &mut State, text: &str) {
    let frame = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n");
    state.on_chunk("hocus-1", frame.as_bytes(), false, None);
    state.on_chunk("hocus-1", b"", true, None);
}

// -- entering the mode --------------------------------------------------

/// `/plan` is a mode switch *and* a question in one. Without the question there
/// is nothing to switch for, so a bare `/plan` is an error rather than a silent
/// change to what the next message will be allowed to do.
#[test]
fn slash_plan_requires_a_prompt() {
    let mut state = idle_state();
    state.input.set_text("/plan".to_owned());
    state.submit();

    assert!(matches!(state.plan, Plan::Off), "the mode did not change");
    assert!(
        state.messages.iter().all(|m| m.tool_calls.is_empty()),
        "no tool was run",
    );
    assert!(
        !state.messages.iter().any(|m| m.role == Role::User),
        "nothing was sent",
    );
    assert_eq!(state.messages.last().unwrap().role, Role::Error);
}

/// The whole point of the command: one submit both enters the mode and asks the
/// question inside it.
#[test]
fn slash_plan_enters_the_mode_and_sends_the_prompt() {
    let mut state = idle_state();
    state
        .input
        .set_text("/plan summarise the render path".to_owned());
    state.submit();

    assert!(state.is_planning());
    assert!(
        state.messages.iter().all(|m| m.tool_calls.is_empty()),
        "`plan` was never taken for a tool",
    );
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .expect("the prompt went out as a user turn");
    assert_eq!(last_user.content, "summarise the render path");
}

/// `/plan on` and `/plan off` were the old toggle. They must not fall through to
/// `parse_command` and be answered "no such tool: plan" — and now that the rest
/// of the line is a prompt, they must not be sent to the model as one-word
/// planning questions either.
#[test]
fn on_and_off_are_errors_not_prompts() {
    let mut state = idle_state();
    for line in ["/plan on", "/plan off"] {
        state.input.set_text(line.to_owned());
        state.submit();
        assert_eq!(
            state.messages.last().unwrap().role,
            Role::Error,
            "{line} was refused",
        );
    }
    assert!(matches!(state.plan, Plan::Off));
    assert!(
        state.messages.iter().all(|m| m.tool_calls.is_empty()),
        "`plan` was never taken for a tool",
    );
    assert!(
        !state.messages.iter().any(|m| m.role == Role::User),
        "neither word reached the model",
    );
}

/// Re-entering is just the next planning turn, which is what makes `/plan` usable
/// as a prefix on a correction. No second "planning mode on".
#[test]
fn slash_plan_while_planning_only_sends() {
    let mut state = State {
        plan: Plan::Planning,
        ..idle_state()
    };
    state
        .input
        .set_text("/plan also check the tab bar".to_owned());
    state.submit();

    assert!(state.is_planning());
    assert!(
        !state
            .messages
            .iter()
            .any(|m| m.content.starts_with("planning mode on")),
        "the mode did not announce itself again",
    );
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .expect("the prompt went out");
    assert_eq!(last_user.content, "also check the tab bar");
}

/// The prompt half of `/plan` is an ordinary message, so it waits its turn behind
/// an arriving reply rather than opening a second stream.
#[test]
fn a_plan_prompt_queues_behind_an_arriving_reply() {
    let mut state = ready_state();
    state
        .input
        .set_text("/plan work out an approach".to_owned());
    state.submit();

    assert!(state.is_planning(), "the mode is on straight away");
    assert_eq!(
        state.asks.iter().map(Ask::shown).collect::<Vec<_>>(),
        vec!["work out an approach"],
        "the question queued",
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

    state
        .input
        .set_text("/plan work out an approach".to_owned());
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

    state
        .input
        .set_text("/plan work out an approach".to_owned());
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

/// The plan is drawn as a box with the answer written along its bottom border,
/// so the prompt has to say which kind of box it is and which keys go on it.
/// The body is deliberately empty: a plan's body is the reply already in the
/// transcript, which the renderer lifts into the frame.
#[test]
fn a_waiting_plan_asks_through_a_plan_box() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "the plan");

    let prompt = state.confirmation_prompt().expect("the prompt is up");

    assert_eq!(prompt.kind, PromptKind::Plan);
    assert_eq!(prompt.title, "PLAN");
    assert!(prompt.detail.is_empty(), "the reply itself is the body");
    let keys: Vec<&str> = prompt.keys.iter().map(|(key, _)| *key).collect();
    assert_eq!(keys, vec!["/execute", "n", "Esc"]);
}

#[test]
fn help_does_not_displace_a_waiting_plan() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "the plan");
    state.input.set_text("/help".to_owned());

    state.submit();

    assert!(state.awaiting_plan());
    assert_eq!(state.messages.last().unwrap().role, Role::Notice);
    assert!(state.messages.last().unwrap().content.contains("/execute"));
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
fn execute_leaves_the_mode_and_carries_the_plan_out() {
    let mut state = planning_state();
    state.tool_rounds = 4;
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("/execute".to_owned());
    state.submit();

    assert!(matches!(state.plan, Plan::Off));
    assert_eq!(state.tool_rounds, 0, "execution gets its own budget");
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .expect("the approval is a user turn");
    assert_eq!(
        last_user.content,
        "Approved. Carry out the plan you just wrote."
    );
}

/// Trailing text is an amendment to the approval, not a turn of its own — the
/// model must not be handed "carry it out" and "but not like that" separately.
#[test]
fn execute_carries_a_trailing_amendment() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state
        .input
        .set_text("/execute skip the migration step".to_owned());
    state.submit();

    assert!(matches!(state.plan, Plan::Off));
    let users: Vec<&str> = state
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(
        users.last().copied(),
        Some("Approved. Carry out the plan you just wrote. skip the migration step"),
    );
}

/// `--clear` carries the plan out in a chat of its own: the planning chat is
/// parked with its research, and the executor is handed the plan and only the
/// plan, as the one user turn a fresh transcript starts from.
#[test]
fn execute_clear_carries_the_plan_into_a_new_tab() {
    let mut state = planning_state();
    state.tool_rounds = 4;
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("/execute --clear".to_owned());
    state.submit();

    assert_eq!(
        state.parked.len(),
        1,
        "the planning chat was parked, not lost"
    );
    let planned = &state.parked[0];
    assert!(
        matches!(planned.plan, Plan::Off),
        "the parked chat left the mode"
    );
    assert!(
        planned.messages.iter().any(|m| m.content == "The plan."),
        "the plan stayed where it was written"
    );
    assert_eq!(
        planned.messages.last().map(|m| m.content.as_str()),
        Some("planning mode off — carrying out the plan in a new chat"),
    );

    assert!(matches!(state.plan, Plan::Off));
    assert_eq!(state.tool_rounds, 0, "execution gets its own budget");
    assert!(
        state.is_streaming(),
        "the request went out from the new chat"
    );
    let users: Vec<&str> = state
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(
        users,
        vec!["Approved. Carry out this plan:\n\nThe plan."],
        "the fresh transcript is the plan and nothing else"
    );
    assert!(
        !state.messages.iter().any(|m| m.content == "hi"),
        "the planning prompt did not come along"
    );
}

/// An amendment typed after `--clear` follows the plan it amends.
#[test]
fn execute_clear_carries_a_trailing_amendment() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state
        .input
        .set_text("/execute --clear skip the migration step".to_owned());
    state.submit();

    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .expect("the approval is a user turn");
    assert_eq!(
        last_user.content,
        "Approved. Carry out this plan:\n\nThe plan.\n\nskip the migration step"
    );
}

/// `--clearly` is a note, not the flag: only the whole word clears.
#[test]
fn execute_clear_is_a_whole_word() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("/execute --clearly".to_owned());
    state.submit();

    assert!(state.parked.is_empty(), "nothing was parked");
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .expect("the approval is a user turn");
    assert_eq!(
        last_user.content,
        "Approved. Carry out the plan you just wrote. --clearly"
    );
}

/// The flag changes where the plan is carried out, not whether there is one.
#[test]
fn execute_clear_without_a_waiting_plan_is_the_same_error() {
    for plan in [Plan::Planning, Plan::Off] {
        let mut state = State {
            plan,
            ..idle_state()
        };
        state.input.set_text("/execute --clear".to_owned());
        state.submit();

        assert!(state.parked.is_empty(), "nothing was parked");
        assert_eq!(state.messages.last().unwrap().role, Role::Error);
    }
}

/// Mid-research there is no plan to carry out yet, and outside the mode there was
/// never one. Both are errors rather than a request going out on their own.
#[test]
fn execute_without_a_waiting_plan_is_an_error() {
    for (plan, planning) in [(Plan::Planning, true), (Plan::Off, false)] {
        let mut state = State {
            plan,
            ..idle_state()
        };
        state.input.set_text("/execute".to_owned());
        state.submit();

        assert_eq!(
            state.plan_mode_on(),
            planning,
            "the mode was left exactly as it was",
        );
        assert_eq!(state.messages.last().unwrap().role, Role::Error);
        assert!(
            !state.messages.iter().any(|m| m.role == Role::User),
            "nothing was sent",
        );
    }
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

/// `y` no longer answers a plan — `/execute` does — so it is always typing,
/// whether or not the box already holds a correction.
#[test]
fn y_is_typed_rather_than_read_as_approval() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");

    state.on_key(KeyWithModifier::new(BareKey::Char('y')));
    assert!(
        state.awaiting_plan(),
        "the plan is still waiting, not approved out from under the user",
    );
    assert_eq!(state.input.get_text(), "y", "even on an empty box");

    state.input.set_text("you should reconsider".to_owned());
    state.on_key(KeyWithModifier::new(BareKey::Char('y')));
    assert!(state.awaiting_plan());
    assert_eq!(
        state.input.get_text(),
        "you should reconsidery",
        "the keystroke reached the editor rather than being swallowed"
    );
}

/// `n` still keeps refining, and still only on an empty box: with a correction
/// half typed it is a letter, not an answer.
#[test]
fn n_answers_the_plan_only_on_an_empty_box() {
    let mut state = planning_state();
    reply_and_settle(&mut state, "The plan.");
    state.input.set_text("no, because".to_owned());
    state.on_key(KeyWithModifier::new(BareKey::Char('n')));
    assert!(state.awaiting_plan(), "the letter was typed, not an answer");

    state.input.set_text(String::new());
    state.on_key(KeyWithModifier::new(BareKey::Char('n')));
    assert!(state.is_planning(), "back to refining");
    assert!(!state.awaiting_plan());
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

// -- skills ---------------------------------------------------------------

/// A skill is typed text as far as the mode is concerned: inside it, a
/// planning turn; at a waiting plan, the answer to it.
#[test]
fn a_skill_is_a_planning_turn_while_planning_and_answers_a_waiting_plan() {
    let mut state = planning_state();
    settle(&mut state);
    load_skill(
        &mut state,
        skills::Source::User,
        "bench",
        "Run the benchmarks.",
    );
    state.input.set_text("/bench".to_owned());
    state.submit();
    assert!(state.is_planning());
    assert!(state.is_receiving());

    let mut state = idle_state();
    state.plan = Plan::AwaitingApproval;
    load_skill(
        &mut state,
        skills::Source::User,
        "bench",
        "Run the benchmarks.",
    );
    state.input.set_text("/bench again".to_owned());
    state.submit();
    assert!(state.is_planning(), "answered, not left waiting");
    assert!(!state.awaiting_plan());
    assert!(state.is_receiving());
}
