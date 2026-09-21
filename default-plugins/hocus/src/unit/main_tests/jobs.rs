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

/// A restored job is exactly the row it was written from, less the countdown,
/// which resumes from the wall clock.
#[test]
fn a_scheduled_job_survives_open_store() {
    let mut state = idle_state();
    state.run_local(&schedule_call(
        r#"{"seconds":60,"prompt":"check pane 3","show":true,"auto_confirm":true}"#,
    ));
    state.run_local(&schedule_call(r#"{"seconds":300,"prompt":"later"}"#));
    let rows = state
        .store()
        .fetch(&FetchRequest::entity(hocus_proto::jobs::ENTITY))
        .expect("fetched");
    assert_eq!(
        hocus_proto::jobs::from_rows(&rows).len(),
        2,
        "written as armed"
    );

    // The next hocus, over the same store.
    let mut restarted = State::default();
    restarted
        .store()
        .apply(
            hocus_proto::jobs::from_rows(&rows)
                .iter()
                .map(|row| hocus_proto::jobs::to_mutation(&restarted.owner(), row))
                .collect(),
        )
        .expect("copied");
    assert!(restarted.jobs.is_empty());
    restarted.open_store();

    assert_eq!(restarted.jobs.len(), 2);
    let job = &restarted.jobs[0];
    assert_eq!(
        (job.id, job.every, job.prompt.as_str()),
        (1, 60.0, "check pane 3")
    );
    assert!(job.show && job.auto_confirm);
    assert!(
        job.due_in > 55.0 && job.due_in <= 60.0,
        "due in {}",
        job.due_in
    );
    assert_eq!(
        restarted.job_seq, 2,
        "the next job id follows the restored ones"
    );
    assert!(restarted.timer_armed, "and the countdown is running");
    assert_eq!(
        restarted.messages.last().map(|m| m.content.as_str()),
        Some("restored 2 scheduled job(s)")
    );

    restarted.open_store();
    assert_eq!(
        restarted.jobs.len(),
        2,
        "opening again restores nothing twice"
    );
}

#[test]
fn unscheduling_and_a_new_chat_forget_the_rows() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"a"}"#));
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"b"}"#));
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"c"}"#));
    let stored = |state: &State| {
        let rows = state
            .store()
            .fetch(&FetchRequest::entity(hocus_proto::jobs::ENTITY))
            .expect("fetched");
        let mut ids: Vec<u64> = hocus_proto::jobs::from_rows(&rows)
            .iter()
            .map(|row| row.job_id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(stored(&state), vec![1, 2, 3]);

    state.unschedule(Some(2));
    assert_eq!(stored(&state), vec![1, 3]);

    // A new chat parks the old one, and the jobs travel with it, still armed.
    state.new_chat();
    assert_eq!(stored(&state), vec![1, 3]);

    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"d"}"#));
    assert_eq!(stored(&state), vec![1, 3, 4]);
    state.run_manual("/stop_jobs");
    assert!(
        stored(&state).is_empty(),
        "every chat's, from whichever chat"
    );

    // Throwing a chat away forgets what it had armed.
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"e"}"#));
    assert_eq!(stored(&state), vec![5]);
    state.reset_chat();
    assert!(stored(&state).is_empty());
}

#[test]
fn a_fired_job_moves_its_due_time_in_the_store() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));
    let due_at = |state: &State| {
        let rows = state
            .store()
            .fetch(&FetchRequest::entity(hocus_proto::jobs::ENTITY))
            .expect("fetched");
        hocus_proto::jobs::from_rows(&rows)[0].next_due_at
    };
    let armed = due_at(&state);
    let now = transitions::now_secs();
    assert!(
        (now + 59..=now + 61).contains(&armed),
        "armed for {armed}, now {now}"
    );

    // Ticks alone write nothing: only the firing does.
    assert!(!state.on_timer(30.0));
    assert_eq!(due_at(&state), armed);

    assert!(state.on_timer(30.0), "due");
    let refired = due_at(&state);
    assert!(
        (now + 59..=now + 62).contains(&refired),
        "rearmed for {refired}, now {now}"
    );
}

/// The stall this exists to remove: a scheduled run that reaches a gated tool
/// used to park on `y`/`n` with every other job queued behind it.
#[test]
fn a_scheduled_run_declines_a_confirmation_instead_of_parking() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":5,"prompt":"tidy up"}"#));
    state.run_local(&schedule_call(r#"{"seconds":5,"prompt":"and again"}"#));
    state.messages.clear();
    assert!(state.on_timer(MIN_JOB_SECONDS));
    assert_eq!(
        state.scheduled_run,
        Some(ScheduledRun {
            job_id: 1,
            auto_confirm: false
        })
    );

    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(
        !matches!(state.status, Status::ConfirmingTool),
        "nobody is asked"
    );
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .content
            .starts_with("declined: `clear_pane` needs confirmation"),
        "the model is told why: {}",
        results[0].content
    );
    assert!(state.is_receiving(), "and the run goes on to report back");

    // The run ends, and the next job is not held behind the first.
    state.on_chunk(
        "hocus-2",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n",
        false,
        None,
    );
    state.on_chunk("hocus-2", b"", true, None);
    assert!(state.scheduled_run.is_none());
    assert!(state.on_timer(0.1), "the second job fires");
    assert!(state
        .messages
        .iter()
        .any(|m| m.content == "[scheduled] and again"));
}

/// The job ids the store holds, sorted.
fn stored_job_ids(state: &State) -> Vec<u64> {
    let rows = state
        .store()
        .fetch(&FetchRequest::entity(hocus_proto::jobs::ENTITY))
        .expect("fetched");
    let mut ids: Vec<u64> = hocus_proto::jobs::from_rows(&rows)
        .iter()
        .map(|row| row.job_id)
        .collect();
    ids.sort();
    ids
}

/// A row for a job armed in chat `chat_id`, as the last hocus would have left it.
fn held_row(job_id: u64, chat_id: Option<u64>) -> hocus_proto::jobs::JobRow {
    hocus_proto::jobs::JobRow {
        job_id,
        chat_id,
        every: 60.0,
        next_due_at: crate::transitions::now_secs() + 60,
        prompt: format!("job {job_id}"),
        show: false,
        auto_confirm: false,
    }
}

/// A job armed in a chat that is not open at the next start waits for that
/// chat rather than firing into whichever one is on screen.
#[test]
fn a_restored_job_waits_for_its_chat() {
    let mut state = State::default();
    state
        .store()
        .apply(vec![
            hocus_proto::jobs::to_mutation(&state.owner(), &held_row(1, Some(3))),
            hocus_proto::jobs::to_mutation(&state.owner(), &held_row(2, Some(3))),
            hocus_proto::jobs::to_mutation(&state.owner(), &held_row(3, None)),
        ])
        .expect("written");
    state.open_store();

    assert_eq!(
        state.jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![3],
        "only the job of a chat that was never saved lands in the foreground"
    );
    assert_eq!(
        state
            .held_jobs
            .get(&3)
            .map(|jobs| jobs.iter().map(|job| job.id).collect::<Vec<_>>()),
        Some(vec![1, 2]),
        "the others wait under their chat"
    );
    assert_eq!(state.job_seq, 3);
    assert_eq!(state.job_count(), 1, "held jobs are not counting down");
    assert_eq!(state.total_job_count(), 3);
    assert!(
        state.live_chat_ids().contains(&3),
        "and /history marks their chat as live"
    );
    let notices: Vec<&str> = state.messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        notices,
        vec![
            "restored 1 scheduled job(s)",
            "2 scheduled job(s) wait in archived chats: /history reopens one with its jobs"
        ]
    );

    state.open_store();
    assert_eq!(
        state.total_job_count(),
        3,
        "opening again holds nothing twice"
    );
}

/// `/history` opening the chat is what hands it its jobs back, counting down
/// from there.
#[test]
fn reopening_a_chat_brings_its_jobs_with_it() {
    let mut state = idle_state();
    state.messages = vec![Message::user("watch this"), Message::assistant("ok")];
    state.flush_save();
    let saved = state.chat_id.expect("the flush allocated an id");
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"a"}"#));
    assert_eq!(stored_job_ids(&state), vec![1]);

    // The next hocus: the chat is in the archive and nowhere else, its job
    // in the store with the chat's id on it.
    state.new_chat();
    state.parked.retain(|chat| chat.chat_id != Some(saved));
    state.chat_tab = 0;
    state.timer_armed = false;
    state.restore_persisted();
    assert!(state.jobs.is_empty());
    assert_eq!(state.held_jobs.get(&saved).map(Vec::len), Some(1));
    assert!(!state.timer_armed, "nothing is counting a held job down");

    state.switch_to_chat(saved);
    assert_eq!(state.chat_id, Some(saved));
    assert_eq!(
        state.jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![1],
        "the job is the reopened chat's again"
    );
    assert!(state.held_jobs.is_empty());
    assert!(state.timer_armed, "and counting down");
    assert_eq!(
        state.messages.last().map(|m| m.content.as_str()),
        Some("restored 1 scheduled job(s)"),
        "said in the chat it belongs to"
    );
    assert_eq!(stored_job_ids(&state), vec![1], "still armed in the store");
}

/// Closing a chat's tab stops its jobs for good, in the store as in memory;
/// the row does not come back at the next start as a job nobody remembers.
#[test]
fn closing_a_chat_tab_forgets_its_jobs() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"a"}"#));
    state.new_chat();
    assert_eq!(state.chat_tab_count(), 2, "the job kept its chat open");
    assert_eq!(state.parked[0].jobs.len(), 1);
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"b"}"#));
    assert_eq!(stored_job_ids(&state), vec![1, 2]);

    state.close_tab(0);
    assert_eq!(state.chat_tab_count(), 1);
    assert_eq!(
        stored_job_ids(&state),
        vec![2],
        "the closed chat's job is gone, the foreground's stays"
    );
}

/// The kill switch reaches jobs held for chats that are not open, which no
/// tab could otherwise get at.
#[test]
fn stop_jobs_clears_held_jobs_too() {
    let mut state = idle_state();
    state
        .store()
        .apply(vec![
            hocus_proto::jobs::to_mutation(&state.owner(), &held_row(7, Some(3))),
            hocus_proto::jobs::to_mutation(&state.owner(), &held_row(8, Some(4))),
        ])
        .expect("written");
    state.open_store();
    assert_eq!(state.held_jobs.len(), 2);
    assert_eq!(state.job_count(), 0);

    assert!(
        state.run_manual("/stop_jobs"),
        "there was something to stop"
    );
    assert!(state.held_jobs.is_empty());
    assert!(stored_job_ids(&state).is_empty());
    assert_eq!(
        state.messages.last().map(|m| m.content.as_str()),
        Some("cancelled 2 scheduled job(s)")
    );

    // And one by id.
    state
        .store()
        .apply(vec![hocus_proto::jobs::to_mutation(
            &state.owner(),
            &held_row(9, Some(3)),
        )])
        .expect("written");
    state.restore_persisted();
    assert_eq!(state.unschedule(Some(9)), "cancelled job 9");
    assert!(
        state.held_jobs.is_empty(),
        "an emptied chat is dropped from the map"
    );
    assert!(stored_job_ids(&state).is_empty());
}

/// A job that fires with `show: true` reveals the container.
#[test]
fn a_job_that_fires_with_show_brings_the_panel_back() {
    let mut state = idle_state();
    state.run_local(&schedule_call(
        r#"{"seconds":60,"prompt":"check pane 3","show":true}"#,
    ));

    state.hide_panel();
    assert!(state.hidden);

    assert!(state.on_timer(60.0), "the job is due");

    assert!(!state.hidden, "the panel is up, and hocus says so");
}
