use super::*;

#[test]
fn an_unrestricted_job_requires_one_confirmation_when_the_model_creates_it() {
    let mut state = ready_state();
    let frame = tool_call_frame(
        "call_1",
        "schedule",
        r#"{"seconds":60,"prompt":"check","auto_confirm":true}"#,
    );
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(state.jobs.is_empty(), "the job waits for user authority");
    state.confirm_tool();
    assert_eq!(state.jobs.len(), 1);
    assert!(state.jobs[0].auto_confirm);
}

#[test]
fn a_job_fires_only_once_its_interval_has_elapsed() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check pane 3"}"#));
    assert_eq!(state.jobs.len(), 1);

    assert!(!state.on_timer(1.0), "a second in, nothing to do");
    assert!(state.messages.is_empty());

    assert!(state.on_timer(60.0), "now it is due");
    assert_eq!(
        state.messages.first().map(|m| (m.role, m.content.as_str())),
        Some((Role::User, "[scheduled] check pane 3"))
    );
    assert!(state.is_receiving(), "and it sent");
    assert_eq!(state.jobs[0].due_in, 60.0, "rearmed for the next run");
}

#[test]
fn an_unrestricted_job_runs_gated_tools_without_confirmation() {
    let mut state = idle_state();
    state.run_manual(r#"/schedule seconds=5 prompt=check auto_confirm=true"#);
    state.messages.clear();
    assert!(state.on_timer(MIN_JOB_SECONDS));

    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(tool_results(&state).len(), 1);

    state.on_chunk(
        "hocus-2",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n",
        false,
        None,
    );
    state.on_chunk("hocus-2", b"", true, None);
    state.input.set_text("ordinary question".to_owned());
    state.submit();
    let frame = tool_call_frame("call_2", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-3", &frame, false, None);
    state.on_chunk("hocus-3", b"", true, None);

    assert!(matches!(state.status, Status::ConfirmingTool));
}

#[test]
fn an_unrestricted_job_can_create_another_unrestricted_job() {
    let mut state = idle_state();
    state.run_manual(r#"/schedule seconds=5 prompt=check auto_confirm=true"#);
    state.messages.clear();
    assert!(state.on_timer(MIN_JOB_SECONDS));

    let frame = tool_call_frame(
        "call_1",
        "schedule",
        r#"{"seconds":60,"prompt":"more","auto_confirm":true}"#,
    );
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(state.jobs.len(), 2);
    assert!(state.jobs[1].auto_confirm);
}

#[test]
fn a_due_job_waits_for_the_reply_it_would_interrupt() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));
    state.stream = state_streaming(Vec::new(), "hocus-9").stream;

    state.on_timer(90.0);
    assert!(state.messages.is_empty(), "one stream slot, so it holds");
    assert!(state.jobs[0].due_in <= 0.0, "still due");

    state.stream = None;
    assert!(state.on_timer(0.1), "and fires on the next tick");
    assert_eq!(state.messages.first().map(|m| m.role), Some(Role::User));
}

#[test]
fn a_quiet_job_tick_asks_for_no_render() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":600,"prompt":"check"}"#));
    state.timer_armed = true;

    assert!(!state.on_timer(1.0), "nothing changed on screen");
    assert_eq!(state.tick, 0, "and the spinner did not advance");
    assert!(state.timer_armed, "but the countdown keeps running");
}

#[test]
fn an_absurd_interval_is_clamped_and_a_promptless_job_is_refused() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":0.2,"prompt":"check"}"#));
    assert_eq!(state.jobs[0].every, MIN_JOB_SECONDS);

    state.run_local(&schedule_call(r#"{"seconds":60}"#));
    assert_eq!(state.jobs.len(), 1, "no prompt, no job");
}

#[test]
fn jobs_are_cancelled_by_the_tool_by_the_command_and_by_a_new_chat() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"a"}"#));
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"b"}"#));

    assert_eq!(state.unschedule(Some(1)), "cancelled job 1");
    assert_eq!(state.jobs.len(), 1);
    assert_eq!(state.unschedule(Some(1)), "no job 1", "and it stays gone");

    assert!(
        state.run_manual("/stop_jobs"),
        "the slash command clears the rest"
    );
    assert!(state.jobs.is_empty());
    assert!(
        !state.run_manual("/stop_jobs"),
        "and does nothing when there are none"
    );

    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"c"}"#));
    state.new_chat();
    assert!(
        state.jobs.is_empty(),
        "a new chat takes its watchers with it"
    );
}
