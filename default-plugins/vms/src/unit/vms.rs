use super::*;

#[test]
fn conversation_focus_enters_the_host_panel_bar() {
    let mut state = State::default();
    state.take_ring_stop(Some(agent_proto::vms::STOP_TABS));
    assert!(state.tabs_focused);
    assert_eq!(state.bar_focused, [state.own_client_id]);
    state.take_ring_stop(Some(wire::STOP_HISTORY));
    assert!(!state.tabs_focused);
    assert!(state.bar_focused.is_empty());
}

fn state() -> State {
    let mut state = State {
        ..Default::default()
    };
    state.granted = true;
    state.vms.owner = Some(PluginId::new(1));
    state.vms.visible = true;
    state.pane_shown = true;
    state.vms.snapshot = serde_json::from_str(r#"{"vms":[{"session":"one","name":"api","state":"running","vm_id":"physical","panes":[{"id":12,"title":"shell","agent":false}]},{"session":"two","name":"api","state":"running","vm_id":"physical"},{"session":"three","name":"api","state":"running","vm_id":"different"}],"recoveries":[{"path":"/keep/disk","reason":"merge failed","retained_at":123}]}"#).unwrap();
    for lease in &mut state.vms.snapshot.vms {
        lease.entry.worktree = Some("/repo".into());
    }
    state.vms.loaded = true;
    state
}
fn outcome(identity: Identity, operation: Operation, text: &str) -> Outcome {
    Outcome {
        identity,
        operation,
        failed: false,
        at_ms: now_ms(),
        text: text.into(),
    }
}
/// agent's answer to whatever is pending.
fn reply(state: &State, snapshot: Option<Snapshot>, output: &str, error: &str) -> ResultMessage {
    let request = state.vms.pending.clone().expect("a request out");
    ResultMessage {
        request_id: request.request_id,
        operation: request.operation,
        target: request.target,
        snapshot,
        output: output.into(),
        error: error.into(),
    }
}
fn details(state: &State) -> String {
    state.vm_details().lines.join("\n")
}
fn key(bare: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare)
}
#[test]
fn vm_polling_is_single_flight_and_pauses_when_hidden_or_on_agents() {
    let mut state = state();
    state.poll_vms();
    let id = state.vms.pending.as_ref().unwrap().request_id;
    state.poll_vms();
    assert_eq!(state.vms.pending.as_ref().unwrap().request_id, id);
    state.vms.pending = None;
    state.vms.visible = false;
    state.poll_vms();
    assert!(state.vms.pending.is_none());
    state.vms.visible = true;
    state.pane_shown = true;
    state.update(Event::Visible(false));
    state.poll_vms();
    assert!(state.vms.pending.is_none());
    state.update(Event::Visible(true));
    assert!(state.vms.pending.is_some());
}
#[test]
fn vm_reply_identity_and_request_id_protect_the_last_successful_snapshot() {
    let mut state = state();
    state.poll_vms();
    let request = state.vms.pending.clone().unwrap();
    let mut reply = ResultMessage {
        request_id: request.request_id + 1,
        operation: Operation::List,
        target: None,
        snapshot: Some(Snapshot::default()),
        output: String::new(),
        error: String::new(),
    };
    assert!(!state.accept_vm_result(reply.clone()));
    assert_eq!(state.vms.snapshot.vms.len(), 3);
    reply.request_id = request.request_id;
    reply.snapshot = None;
    reply.error = "provider denied".into();
    assert!(state.accept_vm_result(reply));
    assert_eq!(state.vms.snapshot.vms.len(), 3);
    assert_eq!(state.vms.error, "provider denied");
}
#[test]
fn vm_request_timeout_recovers_hidden_views_without_repeating_operations() {
    for operation in [
        Operation::List,
        Operation::Changes,
        Operation::Sync,
        Operation::Close,
    ] {
        let mut state = state();
        let target = (operation != Operation::List).then(|| state.vms.snapshot.vms[0].identity());
        state.vm_request(operation, target.clone());
        let request = state.vms.pending.clone().unwrap();
        let deadline = state.vms.pending_deadline_ms;
        assert!(!state.expire_vm_request(deadline - 1));
        assert_eq!(
            state.vms.pending.as_ref().unwrap().request_id,
            request.request_id
        );
        state.vms.visible = false;
        state.update(Event::Visible(false));
        assert!(state.expire_vm_request(deadline));
        assert!(state.vms.pending.is_none());
        assert!(!state.expire_vm_request(deadline));
        assert_eq!(state.vms.snapshot.vms.len(), 3);
        if let Some(identity) = target {
            // Given up on, not failed: the provider times out on inactivity, and the
            // request may still be running.
            assert_eq!(state.vms.results[0].identity, identity);
            assert!(state.vms.results[0].text.contains("No reply after 60 s"));
            assert!(!state.vms.results[0].failed);
            assert!(!state.vms.has_error());
        } else {
            assert!(state.vms.error.contains("did not answer in 60 s"));
        }
        state.vms.visible = true;
        state.pane_shown = true;
        state.update(Event::Visible(true));
        let fresh = state.vms.pending.clone().unwrap();
        assert_eq!(fresh.operation, Operation::List);
        assert!(fresh.request_id > request.request_id);
        let reply = ResultMessage {
            request_id: request.request_id,
            operation,
            target: request.target,
            snapshot: Some(Snapshot::default()),
            output: "completed; merge warning: kept host copy".into(),
            error: String::new(),
        };
        let mut wrong_target = reply.clone();
        wrong_target.target = None;
        wrong_target.request_id += 10;
        assert!(!state.accept_vm_result(wrong_target));
        assert_eq!(
            state.accept_vm_result(reply.clone()),
            operation != Operation::List
        );
        assert!(!state.accept_vm_result(reply));
        assert!(state.vms.timed_out_operations.is_empty());
        if operation != Operation::List {
            assert_eq!(
                state.vms.results[0].text,
                "completed; merge warning: kept host copy"
            );
        }
        assert_eq!(
            state.vms.pending.as_ref().unwrap().request_id,
            fresh.request_id
        );
        assert_eq!(state.vms.snapshot.vms.len(), 3);
    }
}
#[test]
fn vm_delayed_failure_preserves_a_newer_operation_and_requires_matching_identity() {
    let mut state = state();
    state.vms.visible = false;
    let identity = state.vms.snapshot.vms[0].identity();
    state.vm_request(Operation::Sync, Some(identity.clone()));
    let request = state.vms.pending.clone().unwrap();
    state.expire_vm_request(state.vms.pending_deadline_ms);
    state.vm_request(Operation::Changes, Some(identity.clone()));
    let fresh = state.vms.pending.clone().unwrap();
    let mut reply = ResultMessage {
        request_id: request.request_id,
        operation: Operation::Close,
        target: Some(identity.clone()),
        snapshot: None,
        output: String::new(),
        error: "sync failed; guest disk retained".into(),
    };
    assert!(!state.accept_vm_result(reply.clone()));
    reply.operation = Operation::Sync;
    reply.target.as_mut().unwrap().generation = Some("replacement".into());
    assert!(!state.accept_vm_result(reply.clone()));
    reply.target = Some(identity);
    assert!(state.accept_vm_result(reply.clone()));
    assert_eq!(state.vms.results[0].text, reply.error);
    assert!(state.vms.results[0].failed);
    assert!(
        state.vms.has_error(),
        "a failure nobody was looking at marks the tab"
    );
    assert_eq!(
        state.vms.pending.as_ref().unwrap().request_id,
        fresh.request_id
    );
    assert!(!state.accept_vm_result(reply));
}
#[test]
fn vm_timer_redraws_expiry_and_resumes_visible_polling() {
    let mut state = state();
    state.poll_vms();
    let id = state.vms.pending.as_ref().unwrap().request_id;
    state.vms.pending_deadline_ms = 0;
    assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
    assert!(state.vms.error.contains("did not answer"));
    assert!(state.vms.pending.as_ref().unwrap().request_id > id);
    state.vms.pending_deadline_ms = 0;
    state.vms.visible = false;
    // This retained VM instance expires pending work even while another child is selected.
    assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
    assert!(state.vms.pending.is_none());
}
/// Each question says what `y` does - what a close takes with it, which way a sync goes -
/// and stays on the divider while the details are scrolled to read it.
#[test]
fn vm_confirmations_name_the_lease_and_do_not_follow_a_changed_selection() {
    let mut state = state();
    state.vm_key(key(BareKey::Char('x')));
    let text = details(&state);
    assert!(text.contains("Close api in session one? y/n"));
    assert!(text.contains("Closes 1 pane listed below, then releases this lease."));
    assert!(text.contains("VM keeps running for: two"));
    assert!(!text.contains("three"));
    state.render(9, 20);
    state.vm_key(key(BareKey::PageDown));
    assert!(state.vms.confirming.is_some());
    assert!(state.vms.scroll > 0);
    assert!(state
        .vm_divider_label(20, false)
        .starts_with("y close · n cancel · Close api?"));
    state.vms.selected = 2;
    state.vm_key(key(BareKey::Char('y')));
    assert!(state.vms.pending.is_none());
    state.vm_key(key(BareKey::Char('x')));
    assert!(details(&state).contains("No other session holds this VM: it stops"));
    state.vm_key(key(BareKey::Esc));
    state.vms.details = false;
    state.vm_key(key(BareKey::Char('s')));
    let text = details(&state);
    assert!(text.contains("guest changes into host worktree"));
    assert!(text.contains("nothing is closed"));
    assert!(!text.contains("Remaining leases") && !text.contains("Closes"));
    assert!(state
        .vm_divider_label(20, false)
        .starts_with("y sync · n cancel · Sync api to host?"));
    state.vm_key(key(BareKey::Char('y')));
    assert_eq!(
        state
            .vms
            .pending
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .session,
        "three"
    );
}

#[test]
fn bare_vms_explain_disk_deletion_and_do_not_offer_worktree_sync() {
    let mut state = state();
    for lease in &mut state.vms.snapshot.vms {
        lease.entry.worktree = None;
    }
    state.vm_key(key(BareKey::Char('x')));
    assert!(details(&state).contains("VM keeps running for: two"));
    assert!(!details(&state).contains("deletes its guest disk"));
    state.vm_key(key(BareKey::Esc));

    state.vms.selected = 2;
    state.vm_key(key(BareKey::Char('x')));
    let text = details(&state);
    assert!(text.contains("deletes its guest disk"));
    assert!(text.contains("copy needed files out before closing"));
    assert!(!text.contains("merges its worktree back"));
    state.vm_key(key(BareKey::Esc));

    for action in ['s', 'c'] {
        state.vm_key(key(BareKey::Char(action)));
        assert!(state.vms.confirming.is_none());
        assert!(state.vms.pending.is_none() && state.vms.queued.is_none());
        assert!(details(&state).contains("No host worktree is linked to this VM"));
    }
    assert!(!state.vm_hints(false).contains(&"s sync"));
    assert!(!state.vm_hints(false).contains(&"c changes"));
    assert!(!state
        .vm_key_sheet()
        .iter()
        .any(|(key, _)| key == "s" || key == "c"));
}

#[test]
fn vm_ring_mouse_offset_and_small_sizes_keep_tabs_reachable() {
    let mut state = state();
    state.render(19, 40);
    state.vms.details = true;
    state.take_ring_stop(Some(wire::STOP_LIST));
    assert!(
        !state.tabs_focused,
        "a tab click gives the keys to its list"
    );
    assert!(!state.vms.details);
    state.on_key(key(BareKey::Tab));
    assert!(state.vms.details);
    state.on_key(key(BareKey::Tab));
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    state.on_key(key(BareKey::Tab).with_shift_modifier());
    assert!(state.vms.details);
    state.on_key(key(BareKey::Tab).with_shift_modifier());
    assert!(!state.vms.details);
    state.on_key(key(BareKey::Tab).with_shift_modifier());
    assert!(state.tabs_focused);
    state.on_key(key(BareKey::Tab));
    assert!(!state.tabs_focused);
    assert!(!state.vms.details);
    state.on_key(key(BareKey::Tab).with_shift_modifier());
    assert!(state.tabs_focused);
    state.on_mouse(Mouse::LeftClick((2) - 1, 0));
    assert_eq!(state.vms.selected, 1);
    assert!(!state.tabs_focused);
    for rows in 1..8 {
        state.render(usize::saturating_sub(rows, 1), 5);
        assert_eq!(state.vms.last_details_rows, 0);
    }
}

/// agent putting the panel away ends the visit here as well: the keys go back to the list
/// and an armed question is dropped rather than taking the first key after `Ctrl e a`.
#[test]
fn the_owner_hiding_the_panel_ends_the_visit() {
    let mut state = state();
    state.render(19, 40);
    state.vm_key(key(BareKey::Char('x')));
    assert!(state.vms.confirming.is_some() && state.vms.details);
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    let hidden = PipeMessage::new(
        PipeSource::Plugin(PluginId::new(1)),
        "vm_panel_visibility",
        &Some("false".into()),
        &None,
        true,
    );
    state.vm_pipe(&hidden);
    assert!(state.vms.confirming.is_none());
    assert!(!state.vms.details);
    assert!(!state.tabs_focused);
}

/// Leaving the tab drops an armed question too, so it cannot wait there holding the
/// list's polling.
#[test]
fn leaving_the_vm_tab_drops_the_armed_question() {
    let mut state = state();
    state.vm_key(key(BareKey::Char('x')));
    assert!(state.vms.confirming.is_some());
    state.update(Event::Visible(false));
    state.update(Event::Visible(true));
    assert!(state.vms.confirming.is_none());
    assert!(state.vms.pending.is_some(), "and the list polls again");
}

/// A click that moves the selection drops a question about the row it left: drawn only
/// on that row, it would otherwise take the next key meant for this one, and hold the
/// polling until then.
#[test]
fn vm_click_on_another_row_drops_the_armed_confirmation() {
    let mut state = state();
    state.render(19, 40);
    state.vm_key(key(BareKey::Char('x')));
    assert!(state.vms.confirming.is_some());
    assert!(state.on_mouse(Mouse::LeftClick((2) - 1, 0)));
    assert_eq!(state.vms.selected, 1);
    assert!(state.vms.confirming.is_none());
    state.on_key(key(BareKey::Down));
    assert_eq!(state.vms.selected, 2);
    state.poll_vms();
    assert!(state.vms.pending.is_some());
}

#[test]
fn vm_mouse_ignores_the_header_and_empty_panes_without_losing_its_place() {
    let mut state = state();
    let identity = state.vms.snapshot.vms[1].identity();
    state.vms.results.push(outcome(
        identity,
        Operation::Changes,
        &"warning\n".repeat(100),
    ));
    state.vms.selected = 1;
    state.vms.details = true;
    state.vms.scroll = 3;
    state.render(19, 40);
    let position = |state: &State| {
        (
            state.vms.selected,
            state.vms.scroll,
            state.vms.details,
            state.vms.pane,
            state.tabs_focused,
        )
    };
    let before = position(&state);
    let dimensions = |state: &State| {
        [
            state.vms.last_list_rows,
            state.vms.last_details_rows,
            state.vms.window,
            state.vms.max_scroll,
        ]
    };
    let visible = dimensions(&state);
    // The wheel scrolls the half under it. This list fits its half, so over it there is
    // nothing to move; over the details, the details scroll.
    state.on_mouse(Mouse::Hover((1) - 1, 39));
    assert!(!state.on_mouse(Mouse::ScrollDown(1)));
    assert_eq!(position(&state), before);
    for row in [state.vms.last_list_rows, 18] {
        let list = (state.vms.window, state.vms.list_scroll);
        state.on_mouse(Mouse::Hover(row as isize, 39));
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        assert_eq!(state.vms.scroll, 4);
        assert_eq!((state.vms.window, state.vms.list_scroll), list);
        assert!(state.on_mouse(Mouse::ScrollUp(1)));
        assert_eq!(position(&state), before);
        assert_eq!((state.vms.window, state.vms.list_scroll), list);
    }
    for (line, column) in [(0, 0), (-1, 0), (20, 0), (1, 40)] {
        state.on_mouse(Mouse::Hover((line) - 1, column));
        assert!(!state.on_mouse(Mouse::ScrollUp(1)));
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        if line != 0 {
            assert!(!state.on_mouse(Mouse::LeftClick((line) - 1, column)));
        }
        assert_eq!(position(&state), before);
    }
    for (rows, cols) in [(1, 40), (0, 40), (20, 0)] {
        state.render(usize::saturating_sub(rows, 1), cols);
        assert_eq!(
            (state.last_rows, state.last_cols),
            (usize::saturating_sub(rows, 1), cols)
        );
        assert_eq!(dimensions(&state), [0; 4]);
        state.mouse_position = None;
        assert!(!state.on_mouse(Mouse::ScrollUp(1)));
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        state.on_mouse(Mouse::Hover((1) - 1, 0));
        assert!(!state.on_mouse(Mouse::ScrollUp(1)));
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        assert!(!state.on_mouse(Mouse::LeftClick((1) - 1, 0)));
        assert_eq!(position(&state), before);

        state.render(19, 40);
        assert_eq!(dimensions(&state), visible);
        assert_eq!(position(&state), before);
    }
}

/// Over a list longer than its half the wheel moves the list's window and leaves the
/// selection and the details alone; a click lands on the row under the pointer and hands
/// the window back to the selection.
#[test]
fn the_wheel_scrolls_the_half_under_the_pointer() {
    let mut state = state();
    let lease = state.vms.snapshot.vms[2].clone();
    for index in 0..20 {
        let mut more = lease.clone();
        more.entry.name = format!("vm{index}");
        state.vms.snapshot.vms.push(more);
    }
    state.render(19, 40);
    assert_eq!(state.vms.window, 0);
    state.on_mouse(Mouse::Hover((1) - 1, 0));
    assert!(state.on_mouse(Mouse::ScrollDown(3)));
    state.render(19, 40);
    assert_eq!(state.vms.window, 3);
    assert_eq!((state.vms.selected, state.vms.scroll), (0, 0));
    assert!(state.on_mouse(Mouse::LeftClick((2) - 1, 0)));
    assert_eq!(state.vms.selected, 4);
    assert_eq!(state.vms.list_scroll, None);

    let identity = state.vms.snapshot.vms[4].identity();
    state
        .vms
        .results
        .push(outcome(identity, Operation::Changes, &"line\n".repeat(50)));
    state.render(19, 40);
    for row in [state.vms.last_list_rows, 14] {
        let window = state.vms.window;
        state.on_mouse(Mouse::Hover(row as isize, 0));
        assert!(state.on_mouse(Mouse::ScrollDown(2)));
        assert_eq!(state.vms.scroll, 2);
        assert_eq!((state.vms.window, state.vms.list_scroll), (window, None));
        assert_eq!(state.vms.selected, 4);
        assert!(state.on_mouse(Mouse::ScrollUp(2)));
        assert_eq!(state.vms.scroll, 0);
        assert_eq!((state.vms.window, state.vms.list_scroll), (window, None));
        assert_eq!(state.vms.selected, 4);
    }
}

#[test]
fn vm_pane_navigation_reveals_the_cursor_after_wrapped_titles() {
    let mut state = state();
    let lease = &mut state.vms.snapshot.vms[0];
    lease.panes[0].title = "long pane title ".repeat(20);
    lease.panes.push(vms::Pane {
        id: 13,
        title: "next".into(),
        agent: false,
    });
    state.render(19, 20);
    state.vm_key(key(BareKey::Enter));
    state.render(19, 20);
    let (first, scroll) = (state.vms.pane_rows[0].start, state.vms.scroll);
    assert!(
        scroll <= first && first < scroll + state.vms.last_details_rows,
        "the first pane's first row is in view"
    );
    state.vm_key(key(BareKey::Down));
    state.render(19, 20);
    assert_eq!(state.vms.pane, Some(1));
    let row = state.vms.pane_rows[1].clone();
    let (scroll, rows) = (state.vms.scroll, state.vms.last_details_rows);
    assert!(scroll > 5, "scrolled past the first pane's wrapped title");
    assert!(scroll <= row.start && row.end <= scroll + rows);
}
#[test]
fn vm_list_focus_controls_navigation_and_enter_with_a_retained_pane_cursor() {
    let mut state = state();
    state.render(19, 40);
    state.vm_key(key(BareKey::Enter));
    state.vm_key(key(BareKey::Tab).with_shift_modifier());
    assert_eq!(state.vms.pane, Some(0));
    assert!(!state.vms.details);
    state.vm_key(key(BareKey::Enter));
    assert!(state.vms.details, "Enter from the list must enter details");
    assert_eq!(state.panel_hides, 0);
    state.take_ring_stop(Some(wire::STOP_LIST));
    assert!(!state.vms.details);
    state.vm_key(key(BareKey::Down));
    assert_eq!(state.vms.selected, 1);
    assert!(state.vms.pane.is_none());
    state.vm_key(key(BareKey::Up));
    state.vm_key(key(BareKey::Enter));
    state.vm_key(key(BareKey::Tab).with_shift_modifier());
    state.vm_key(key(BareKey::Down));
    assert_eq!(state.vms.selected, 1);
    assert!(state.vms.pane.is_none());
}

/// Esc goes by which half has the keys - which the divider shows - not by whether a pane
/// cursor happens to be kept: back to the list from the details, and away from the list.
#[test]
fn vm_esc_steps_back_from_details_and_hides_from_the_list() {
    let mut state = state();
    state.render(19, 40);
    state.vm_key(key(BareKey::Tab));
    assert!(state.vms.details && state.vms.pane.is_none());
    state.vm_key(key(BareKey::Esc));
    assert!(!state.vms.details);
    assert_eq!(state.panel_hides, 0);
    state.vm_key(key(BareKey::Esc));
    assert_eq!(state.panel_hides, 1);

    // A lease with no panes: Enter opens its details, and Esc comes back from them.
    state.vms.selected = 1;
    state.render(19, 40);
    state.vm_key(key(BareKey::Enter));
    assert!(state.vms.details);
    state.vm_key(key(BareKey::Esc));
    assert!(!state.vms.details);
    assert_eq!(state.panel_hides, 1);

    // A pane cursor kept across Shift+Tab does not swallow the list's Esc.
    state.vms.selected = 0;
    state.vm_key(key(BareKey::Enter));
    state.vm_key(key(BareKey::Tab).with_shift_modifier());
    assert_eq!(state.vms.pane, Some(0));
    state.vm_key(key(BareKey::Esc));
    assert_eq!(state.panel_hides, 2);
}

/// The first click on a pane line points at it, and a second one goes to it, as a second
/// Enter does - a second click made with the keys already in the details. The click that
/// brings them back, off the tab bar or from another pane, only points, even at the pane
/// already chosen.
#[test]
fn a_click_picks_a_pane_and_a_second_click_goes_to_it() {
    let two_panes = || {
        let mut state = state();
        state.vms.snapshot.vms[0].panes.push(vms::Pane {
            id: 13,
            title: "next".into(),
            agent: true,
        });
        state.render(19, 40);
        state
    };
    // Under the tab bar, the list and the divider - measured again after every render,
    // since what the tab bar has the keys for can move the rows.
    let second =
        |state: &State| (1 + state.vms.last_list_rows + 1 + state.vms.pane_rows[1].start) as isize;
    let mut state = two_panes();
    assert!(state.on_mouse(Mouse::LeftClick((second(&state)) - 1, 0)));
    assert!(state.vms.details);
    assert_eq!(state.vms.pane, Some(1));
    assert_eq!(state.panel_hides, 0);

    state.tabs_focused = true;

    state.bar_focused = vec![state.own_client_id];
    state.render(19, 40);
    assert!(state.on_mouse(Mouse::LeftClick((second(&state)) - 1, 0)));
    assert!(!state.tabs_focused);
    assert_eq!(state.panel_hides, 0, "the click off the bar only points");
    state.render(19, 40);
    assert!(state.on_mouse(Mouse::LeftClick((second(&state)) - 1, 0)));
    assert_eq!(state.panel_hides, 1);

    // Tab from the details on to the panel tab bar, and a click back from there.
    let mut state = two_panes();
    assert!(state.on_mouse(Mouse::LeftClick((second(&state)) - 1, 0)));
    assert!(state.vm_key(key(BareKey::Tab)));
    state.has_keyboard = Some(false);
    state.render(19, 40);
    state.on_mouse(Mouse::LeftClick((second(&state)) - 1, 0));
    assert_eq!(
        state.panel_hides, 0,
        "the click from another pane only points"
    );
    // What the manifest the click makes the host send says.
    state.has_keyboard = Some(true);
    state.render(19, 40);
    assert!(state.on_mouse(Mouse::LeftClick((second(&state)) - 1, 0)));
    assert_eq!(state.panel_hides, 1);
}

#[test]
fn vm_retained_disks_are_informational_and_warnings_are_scrollable_in_full() {
    let mut state = state();
    let identity = state.vms.snapshot.vms[0].identity();
    let warning = (0..100)
        .map(|i| format!("warning {i}: host and guest differ"))
        .collect::<Vec<_>>()
        .join("\n");
    state
        .vms
        .results
        .push(outcome(identity, Operation::Sync, &warning));
    state.render(19, 30);
    assert!(state.vms.max_scroll > 100);
    assert!(details(&state).contains("warning 99:"));
    state.vms.selected = 3;
    let text = details(&state);
    assert!(text.contains("/keep/disk"));
    // A deadline, from the provider's own retention, in a form a person can read.
    assert!(text.contains("Retained at: 1970-01-01 00:02 UTC"));
    assert!(text.contains("Deleted automatically on 1970-01-08 00:02 UTC (7 days after retention)"));
    assert!(!text.contains("Unix seconds"));
    state.render(19, 30);
    assert_eq!(state.vm_divider_label(30, false), "Ctrl-r refresh");
    state.vm_key(key(BareKey::Char('x')));
    assert!(state.vms.confirming.is_none());
    assert!(state.vms.pending.is_none());
}

/// A background listing no longer eats `c`, `s` or `x`: the question arms as ever, and
/// the request waits for the listing to answer - and goes only to a lease still listed.
#[test]
fn vm_keys_arm_and_queue_behind_a_background_list() {
    let mut state = state();
    state.render(19, 40);
    let one = state.vms.snapshot.vms[0].identity();
    state.poll_vms();
    let list = state.vms.pending.clone().unwrap();
    assert_eq!(list.operation, Operation::List);
    assert!(state.vm_key(key(BareKey::Char('x'))));
    assert!(state.vms.confirming.is_some());
    state.vm_key(key(BareKey::Char('y')));
    assert_eq!(
        state.vms.pending.as_ref().unwrap().request_id,
        list.request_id
    );
    assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));
    state.render(19, 40);
    assert_eq!(
        state.vm_divider_label(40, false),
        "Queued: close · one / api"
    );
    assert!(details(&state).contains("Close queued behind the list refresh"));
    let snapshot = state.vms.snapshot.clone();
    assert!(state.accept_vm_result(reply(&state, Some(snapshot.clone()), "", "")));
    let sent = state.vms.pending.clone().unwrap();
    assert_eq!(
        (sent.operation, sent.target),
        (Operation::Close, Some(one.clone()))
    );
    assert!(state.vms.queued.is_none());

    // `c` asks nothing, so it queues straight away.
    state.vms.pending = None;
    state.poll_vms();
    assert!(state.vm_key(key(BareKey::Char('c'))));
    assert!(state.vms.confirming.is_none());
    assert_eq!(state.vms.queued, Some((Operation::Changes, one.clone())));
    let mut gone = snapshot;
    gone.vms.remove(0);
    assert!(state.accept_vm_result(reply(&state, Some(gone), "", "")));
    assert!(
        state.vms.pending.is_none(),
        "a lease that went away is not asked"
    );
    let last = state.vms.results.last().unwrap();
    assert!(last.identity == one && last.failed);
    assert!(last.text.contains("changed while this waited"));
}

/// While a request of the user's own is out the keys that would start another redraw,
/// and the divider names what is running and for how long.
#[test]
fn vm_keys_during_a_user_operation_redraw_and_name_it() {
    let mut state = state();
    let one = state.vms.snapshot.vms[0].identity();
    assert!(state.vm_request(Operation::Sync, Some(one)));
    assert!(state.vm_key(key(BareKey::Char('x'))));
    assert!(state.vms.confirming.is_none());
    state.render(19, 40);
    let label = state.vm_divider_label(40, false);
    assert!(
        label.starts_with("Syncing ") && label.ends_with(" · one / api"),
        "{label}"
    );
    assert!(details(&state).contains("Sync running…"));
    assert!(state.poll_vms(), "the seconds on the divider tick");
}
#[test]
fn vm_background_list_keeps_the_hints() {
    let mut state = state();
    state.poll_vms();
    state.render(19, 40);
    let label = state.vm_divider_label(40, false);
    assert!(label.contains("x close"), "{label}");
    assert!(!state.poll_vms(), "a listing counts nothing");
}
/// The hints drop whole keys from the end rather than cutting one in half.
#[test]
fn vm_hints_drop_whole_segments() {
    let mut state = state();
    state.render(19, 80);
    let segments = [
        "Enter panes",
        "x close",
        "s sync",
        "c changes",
        "Ctrl-r refresh",
        "? keys",
    ];
    for cols in 17..80 {
        let label = state.vm_divider_label(cols, false);
        assert!(
            label
                .split(" · ")
                .all(|segment| segments.contains(&segment)),
            "{cols}: {label}"
        );
    }
    assert_eq!(
        state.vm_divider_label(80, false),
        "Enter panes · x close · s sync · c changes · Ctrl-r refresh"
    );
}

/// The answer to a key is what the details open on, the divider counts what is below
/// them, and host output cannot pass for the pane cursor.
#[test]
fn vm_outcomes_open_the_details_and_the_divider_counts_what_is_below() {
    let mut state = state();
    state.render(11, 22);
    let one = state.vms.snapshot.vms[0].identity();
    state.vms.scroll = 2;
    state.vm_request(Operation::Changes, Some(one.clone()));
    let answer = reply(&state, None, "Guest paths changed since sync: 3", "");
    assert!(state.accept_vm_result(answer));
    state.render(11, 22);
    assert_eq!(state.vms.scroll, 0);
    let (wrapped, _) = state.vm_detail_rows(22);
    let shown = wrapped[..state.vms.last_details_rows].join("\n");
    assert!(shown.contains("Latest changes"), "{shown}");
    assert!(shown.contains("Guest paths changed"), "{shown}");
    let below = state.vms.max_scroll - state.vms.scroll;
    assert!(below > 0);
    let label = state.vm_divider_label(22, false);
    assert!(divider_line(&label, 0, below, 22).ends_with(&format!("↓{below} ")));

    // Output that starts like the pane cursor is not taken for it.
    state.vms.results.clear();
    state
        .vms
        .results
        .push(outcome(one.clone(), Operation::Changes, "> not a pane"));
    state.render(19, 40);
    state.vm_key(key(BareKey::Enter));
    state.render(19, 40);
    let (wrapped, pane_rows) = state.vm_detail_rows(40);
    assert!(wrapped[pane_rows[0].start].starts_with("> terminal 12"));
    assert_eq!(
        state.vms.scroll, 0,
        "the pane and the outcome above it both fit"
    );

    // A new request replaces the old answer rather than sitting under it.
    state.vms.results.clear();
    state.vms.results.push(outcome(
        one.clone(),
        Operation::Sync,
        "Worktree sync completed.",
    ));
    state.vms.pending = None;
    assert!(state.vm_request(Operation::Sync, Some(one)));
    let text = details(&state);
    assert!(text.contains("Sync running…"));
    assert!(!text.contains("Worktree sync completed."));
}

/// A provider that was never configured is unavailable, not stale: the list row gives the
/// reason in the provider's words, the divider stays one line, and the tab gets no `!` -
/// until a listing has loaded and a later one fails.
#[test]
fn an_unconfigured_provider_is_unavailable_not_stale() {
    let mut state = State {
        ..Default::default()
    };
    state.granted = true;
    state.vms.owner = Some(PluginId::new(1));
    state.vms.visible = true;
    state.pane_shown = true;
    state.poll_vms();
    let error = "VM command failed (exit 1): Error: Firecracker is not configured: set \
                 firecracker_provider in your Yinz config to the absolute path of a \
                 provider executable to use VMs\n";
    assert!(state.accept_vm_result(reply(&state, None, "", error)));
    state.render(19, 60);
    assert!(state
        .vm_placeholder()
        .starts_with("Firecracker is not configured: set"));
    let label = state.vm_divider_label(60, false);
    assert_eq!(label, "Unavailable · Ctrl-r retry");
    assert!(!divider_line(&label, 0, 0, 60).contains('\n'));
    assert!(!state.vms.has_error());
    assert!(state.vms.last_details_rows > 0);
    assert!(state
        .vm_detail_rows(60)
        .0
        .join("")
        .starts_with("Unavailable: VM command failed"));

    state.poll_vms();
    let listed: Snapshot =
        serde_json::from_str(r#"{"vms":[{"session":"s","name":"v","state":"running"}]}"#).unwrap();
    assert!(state.accept_vm_result(reply(&state, Some(listed), "", "")));
    assert!(state.vms.error.is_empty());
    state.poll_vms();
    assert!(state.accept_vm_result(reply(&state, None, "", "provider denied\n")));
    assert_eq!(state.vm_divider_label(60, false), "STALE: provider denied");
    assert!(state.vms.has_error());
}

/// Past the deadline, agent answering "busy" is proof the request is alive: the divider
/// says it is still running rather than calling the list stale, and the late answer is
/// still the answer.
#[test]
fn a_busy_reply_after_a_timeout_is_still_running_not_stale() {
    let mut state = state();
    let one = state.vms.snapshot.vms[0].identity();
    state.vm_request(Operation::Sync, Some(one));
    let sync = state.vms.pending.clone().unwrap();
    assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
    assert_eq!(
        state.vms.pending.as_ref().unwrap().operation,
        Operation::List
    );
    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
    assert!(state.vms.error.is_empty());
    assert!(!state.vms.has_error());
    state.render(19, 40);
    let label = state.vm_divider_label(40, false);
    assert!(label.starts_with("Syncing "), "{label}");
    assert!(label.contains(", still running · one / api"), "{label}");
    assert!(details(&state).contains("No reply after 60 s"));
    assert!(state.poll_vms(), "the seconds keep counting");

    let late = ResultMessage {
        request_id: sync.request_id,
        operation: sync.operation,
        target: sync.target,
        snapshot: None,
        output: "Worktree sync completed.".into(),
        error: String::new(),
    };
    assert!(state.accept_vm_result(late));
    assert!(!state.vms.busy);
    let text = details(&state);
    assert!(text.contains("Latest sync (just now):"));
    assert!(text.contains("Worktree sync completed."));
}

/// A failure that arrives while another tab is showing marks the tab; one that arrives
/// in front of the user is shown where it happened and marks nothing.
#[test]
fn an_off_tab_failure_marks_the_tab_until_it_is_seen() {
    let mut state = state();
    let one = state.vms.snapshot.vms[0].identity();
    state.vm_request(Operation::Sync, Some(one.clone()));
    state.update(Event::Visible(false));
    assert!(state.accept_vm_result(reply(&state, None, "", "sync failed")));
    assert!(state.vms.has_error());
    assert!(details(&state).contains("Latest sync (failed, just now):"));
    state.update(Event::Visible(true));
    assert!(!state.vms.has_error());

    state.vms.pending = None;
    state.vm_request(Operation::Close, Some(one));
    assert!(state.accept_vm_result(reply(&state, None, "", "close failed")));
    assert!(!state.vms.has_error());
}

/// Too short to split, the tab still keeps a row for what the divider would say: the
/// error, the latest answer, the keys - and Enter leaves no invisible pane cursor behind.
#[test]
fn a_short_pane_keeps_a_status_row_and_enter_opens_nothing() {
    let mut state = state();
    state.render(6, 40);
    assert_eq!(
        (state.vms.last_list_rows, state.vms.last_details_rows),
        (5, 0)
    );
    assert!(state.vm_divider_label(40, true).starts_with("x close"));
    state.vms.error = "provider denied".into();
    assert_eq!(state.vm_divider_label(40, true), "STALE: provider denied");
    state.vms.error.clear();
    let one = state.vms.snapshot.vms[0].identity();
    state.vms.results.push(outcome(
        one,
        Operation::Changes,
        "Guest paths changed since sync: 3",
    ));
    assert!(state
        .vm_divider_label(40, true)
        .starts_with("Changes check: Guest paths changed"));
    assert!(!state.vm_key(key(BareKey::Enter)));
    assert!(state.vms.pane.is_none() && !state.vms.details);
    assert!(
        !state.on_mouse(Mouse::LeftClick((6) - 1, 0)),
        "the status row is not a half"
    );
    state.vm_key(key(BareKey::Esc));
    assert_eq!(state.panel_hides, 1);
}

/// With nothing to list the tab does not split - no empty half, no ring stop in it - and
/// says how a VM is made; the answer to closing the last one keeps the details, since it
/// has nowhere else to be.
#[test]
fn an_empty_list_does_not_split_and_says_how_vms_are_made() {
    let mut state = state();
    let closed = state.vms.snapshot.vms[0].identity();
    state.vms.snapshot = Snapshot::default();
    state.vms.selected = 0;
    state.render(19, 40);
    assert_eq!(state.vms.last_details_rows, 0);
    assert!(state
        .vm_placeholder()
        .starts_with("No Firecracker VMs. Start one from New agent (Ctrl e n)"));
    state.vm_key(key(BareKey::Tab));
    assert!(!state.vms.details);
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);

    state
        .vms
        .results
        .push(outcome(closed, Operation::Close, "Lease released."));
    state.render(19, 40);
    assert!(state.vms.last_details_rows > 0);
    assert!(state
        .vm_detail_rows(40)
        .0
        .join("\n")
        .contains("Lease released."));
    state.take_ring_stop(Some(wire::STOP_LIST));
    assert_eq!(state.vm_divider_label(40, false), "Ctrl-r refresh");
    // Too short for the details, the status row carries it, naming the lease it was.
    state.render(6, 60);
    assert_eq!(state.vms.last_details_rows, 0);
    assert!(state
        .vm_divider_label(60, true)
        .starts_with("Close of one / api: Lease released."));
}

/// Without one agent to ask, nothing offers a question that cannot be answered, and the
/// divider says why and what fixes it.
#[test]
fn vm_actions_without_an_owner_say_why() {
    let mut state = state();
    state.vms.owner = None;
    state.render(19, 40);
    assert!(state.vm_key(key(BareKey::Char('x'))));
    assert!(state.vms.confirming.is_none());
    assert_eq!(state.vm_divider_label(40, false), NO_OWNER_DIVIDER);
    assert!(state.vm_key(key(BareKey::Char('c'))));
    assert!(state.vm_key(key(BareKey::Char('r')).with_ctrl_modifier()));
    state.vm_key(key(BareKey::Char('y')));
    assert!(state.vms.pending.is_none() && state.vms.queued.is_none());
}

/// A fresh tab says what it is waiting on, and once a listing has loaded, the background
/// polls that follow do not flash "Loading" over an empty list.
#[test]
fn a_fresh_vm_tab_waits_for_agent_then_loads() {
    let mut state = State {
        ..Default::default()
    };
    assert_eq!(state.vm_placeholder(), NO_OWNER);
    state.granted = true;
    state.vms.owner = Some(PluginId::new(1));
    assert_eq!(state.vm_placeholder(), "Waiting for agent…");
    state.vms.visible = true;
    state.pane_shown = true;
    state.poll_vms();
    assert_eq!(state.vm_placeholder(), "Loading VMs…");
    assert!(state.accept_vm_result(reply(&state, Some(Snapshot::default()), "", "")));
    state.poll_vms();
    assert!(state.vms.pending.is_some());
    assert!(state.vm_placeholder().starts_with("No Firecracker VMs."));
    // Ctrl-r lists straight away, whatever agent last said about the panel.
    state.vms.pending = None;
    state.vms.visible = false;
    assert!(state.vm_key(key(BareKey::Char('r')).with_ctrl_modifier()));
    assert!(state.vms.pending.is_some());
}

/// The divider marks the details as having the keys - a `▸` as well as its colour - after
/// Tab into them, and only while a key pressed now would reach them: not while the panel's
/// tab bar has the keys, and not while another pane does.
#[test]
fn tab_into_the_details_marks_the_divider() {
    let mut state = state();
    state.render(19, 40);
    let drawn = |state: &State| {
        divider_text(
            &state.vm_divider_label(40, false),
            0,
            0,
            state.vm_details_have_the_keys(),
            40,
        )
        .content()
        .to_owned()
    };
    assert!(drawn(&state).starts_with("─   "), "{}", drawn(&state));
    assert!(state.vm_key(key(BareKey::Tab)));
    assert!(state.vms.details);
    assert!(drawn(&state).starts_with("─ ▸ "), "{}", drawn(&state));
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    assert!(drawn(&state).starts_with("─   "), "the bar has them");
    state.tabs_focused = false;
    state.bar_focused = vec![];
    state.own_pane_changed(true, true);
    state.own_pane_changed(true, false);
    assert!(drawn(&state).starts_with("─   "), "another pane has them");
    assert!(
        state.vms.details,
        "the stop is kept for the keys to come back to"
    );
}

/// The `?` sheet names every key the tab answers in the mode it is in.
#[test]
fn the_vm_key_sheet_follows_the_mode() {
    let mut state = state();
    state.render(19, 40);
    let keys = |state: &State| {
        state
            .vm_key_sheet()
            .into_iter()
            .map(|(keys, _)| keys)
            .collect::<Vec<_>>()
    };
    let what = |state: &State, key: &str| {
        state
            .vm_key_sheet()
            .into_iter()
            .find(|(keys, _)| keys == key)
            .map(|(_, what)| what)
    };
    let list = keys(&state);
    for key in [
        "↑↓ j k",
        "Enter",
        "Esc",
        "x",
        "s",
        "c",
        "PgUp PgDn",
        "Ctrl-r",
        "Tab",
        "Shift+Tab",
        "click",
        "wheel",
    ] {
        assert!(list.iter().any(|keys| keys == key), "{key}");
    }
    assert_eq!(what(&state, "Esc").as_deref(), Some("hide the panel"));
    state.vm_key(key(BareKey::Enter));
    assert_eq!(what(&state, "Esc").as_deref(), Some("back to the list"));
    assert_eq!(
        what(&state, "Enter").as_deref(),
        Some("go to the chosen pane")
    );
    // An armed question stays armed under the sheet, which lists its answers first.
    state.on_key(key(BareKey::Char('x')));
    assert!(state.on_key(key(BareKey::Char('?'))));
    assert_eq!(state.key_sheet_scroll, Some(0));
    assert!(state.vms.confirming.is_some());
    assert_eq!(state.key_sheet()[0].0, "y");
    state.on_key(key(BareKey::Esc));
    assert_eq!(state.key_sheet_scroll, None, "Esc shut the sheet");
    assert!(state.vms.confirming.is_some(), "and left the question");
    state.on_key(key(BareKey::Char('?')));
    state.on_key(key(BareKey::Char('n')));
    assert_eq!(state.key_sheet_scroll, None);
    assert!(state.vms.confirming.is_none());
    // `y` through the sheet answers the question.
    state.on_key(key(BareKey::Char('x')));
    state.on_key(key(BareKey::Char('?')));
    state.on_key(key(BareKey::Char('y')));
    assert_eq!(state.key_sheet_scroll, None);
    assert!(
        state
            .vms
            .pending
            .as_ref()
            .is_some_and(|pending| pending.operation == Operation::Close)
            || state
                .vms
                .queued
                .as_ref()
                .is_some_and(|(operation, _)| *operation == Operation::Close)
    );
}

/// The sheet names only keys that would do something now: none of a lease's on a retained
/// disk, no details' on a pane with none drawn, nothing that asks agent without one agent
/// to ask - and says what fixes that instead.
#[test]
fn the_vm_key_sheet_lists_only_keys_that_act() {
    let keys = |state: &State| {
        state
            .vm_key_sheet()
            .into_iter()
            .map(|(keys, _)| keys)
            .collect::<Vec<_>>()
    };
    let what = |state: &State, key: &str| {
        state
            .vm_key_sheet()
            .into_iter()
            .find(|(keys, _)| keys == key)
            .map(|(_, what)| what)
    };
    let lists = |state: &State, key: &str| keys(state).iter().any(|keys| keys == key);

    let mut disk = state();
    disk.render(19, 40);
    assert!(what(&disk, "c").is_some_and(|what| what.starts_with("count ")));
    disk.vms.selected = 3;
    for (name, bare) in [
        ("Enter", BareKey::Enter),
        ("x", BareKey::Char('x')),
        ("s", BareKey::Char('s')),
        ("c", BareKey::Char('c')),
    ] {
        assert!(!lists(&disk, name), "{name}");
        assert!(!disk.vm_key(key(bare)), "{name}");
    }
    assert!(disk.vms.confirming.is_none() && disk.vms.pending.is_none());

    let mut empty = state();
    empty.vms.snapshot = Snapshot::default();
    empty.vms.selected = 0;
    empty.render(19, 40);
    assert_eq!(empty.vms.last_details_rows, 0);
    for name in ["Enter", "x", "s", "c"] {
        assert!(!lists(&empty, name), "{name}");
    }
    assert!(!keys(&empty).iter().any(|keys| keys.contains("PgUp")));
    assert_eq!(
        what(&empty, "Tab").as_deref(),
        Some("on to the panel tab bar")
    );

    let mut ownerless = state();
    ownerless.vms.owner = None;
    ownerless.render(19, 40);
    for name in ["x", "s", "c", "Ctrl-r"] {
        assert!(!lists(&ownerless, name), "{name}");
    }
    assert!(lists(&ownerless, "Enter"));
    assert!(!lists(&ownerless, "Ctrl e c"));
    assert!(ownerless.vm_key(key(BareKey::Char('x'))));
    assert!(ownerless.vms.confirming.is_none());
}

/// However narrow the divider, a key on it is whole or not there - one too wide for the
/// row gives way to the next rather than being cut - and where the first key leaves room,
/// the row ends by saying `?` has the rest.
#[test]
fn vm_divider_never_cuts_a_key_at_narrow_widths() {
    for cols in [22, 24, 28, 34] {
        for rows in [10, 12, 14] {
            let mut state = state();
            for id in 13..18 {
                state.vms.snapshot.vms[0].panes.push(vms::Pane {
                    id,
                    title: format!("pane {id}"),
                    agent: false,
                });
            }
            state.render(usize::saturating_sub(rows, 1), cols);
            assert!(
                state.vms.max_scroll > 0,
                "{cols}x{rows}: a readout is drawn"
            );
            let check = |state: &State| {
                let label = state.vm_divider_label(cols, false);
                let hints = state.vm_hints(false);
                assert!(!label.is_empty(), "{cols}x{rows}");
                for piece in label.split(" · ") {
                    assert!(
                        piece == "? keys" || hints.contains(&piece),
                        "{cols}x{rows}: {label}"
                    );
                }
                let (scroll, max) = (state.vms.scroll, state.vms.max_scroll);
                let marked = format!("▸ {label}");
                let drawn = divider_line(&marked, scroll, max - scroll, cols);
                assert!(drawn.contains(&marked), "{cols}x{rows}: {drawn}");
                assert!(drawn.width() <= cols, "{cols}x{rows}: {drawn}");
                label
            };
            let list = check(&state);
            // At 34 the readout leaves room for `Enter panes · ? keys`; at 28 it leaves 17
            // or 18 columns, where `Enter panes` is all there is room for.
            if cols == 34 {
                assert!(list.ends_with("? keys"), "{rows}: {list}");
            }
            state.vm_key(key(BareKey::Enter));
            state.render(usize::saturating_sub(rows, 1), cols);
            check(&state);
            for _ in 0..6 {
                state.vm_key(key(BareKey::Down));
                state.render(usize::saturating_sub(rows, 1), cols);
                check(&state);
            }
        }
    }
}

/// Set up the way the timeout tests need it: a sync of lease 0 given up on, with the
/// listing the expiry sent still out.
fn expired_sync(state: &mut State) -> Request {
    state.render(19, 40);
    let one = state.vms.snapshot.vms[0].identity();
    assert!(state.vm_request(Operation::Sync, Some(one)));
    let sync = state.vms.pending.clone().unwrap();
    assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
    assert_eq!(
        state.vms.pending.as_ref().map(|pending| pending.operation),
        Some(Operation::List)
    );
    sync
}

/// A listing given up on is most likely still running in agent, which refuses anything else
/// as busy. A close confirmed behind it waits for a listing agent answers - it used to go
/// out at the expiry, be refused, and be recorded as a failed close, lost - and a busy
/// refusal of a request that did go out puts it back in the queue rather than failing it.
#[test]
fn a_request_behind_an_expired_listing_waits_for_one_that_answers() {
    let mut state = state();
    state.render(19, 40);
    let one = state.vms.snapshot.vms[0].identity();
    let out = |state: &State| state.vms.pending.as_ref().map(|pending| pending.operation);
    state.poll_vms();
    assert_eq!(out(&state), Some(Operation::List));
    state.vm_key(key(BareKey::Char('x')));
    state.vm_key(key(BareKey::Char('y')));
    assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));

    assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
    assert!(state.vms.list_expired);
    assert_eq!(out(&state), None, "not sent into the listing still running");
    assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));
    // Refused while agent still runs the first listing: still waiting, nothing failed.
    state.poll_vms();
    assert_eq!(out(&state), Some(Operation::List));
    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
    assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));
    assert!(state.vms.results.is_empty());
    // A key pressed now waits the same way.
    state.vms.queued = None;
    state.vm_key(key(BareKey::Char('x')));
    state.vm_key(key(BareKey::Char('y')));
    assert_eq!(out(&state), None);
    assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));

    // A listing agent answers sends it.
    state.poll_vms();
    let snapshot = state.vms.snapshot.clone();
    assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
    assert!(!state.vms.list_expired);
    assert_eq!(out(&state), Some(Operation::Close));
    assert!(state.vms.queued.is_none());
    // Were agent busy after all, the close is not a failure: it waits again.
    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
    assert!(
        state.vms.results.is_empty(),
        "a busy refusal is not a failed close"
    );
    assert_eq!(state.vms.queued, Some((Operation::Close, one)));
}

/// An armed close or sync question does not hold the list's polling: it names the panes and
/// the sharing the list says, which used to freeze under it for as long as the keys were in
/// the conversation.
#[test]
fn an_armed_question_keeps_the_list_polling() {
    let mut state = state();
    state.render(19, 40);
    assert!(state.vm_key(key(BareKey::Char('x'))));
    assert!(state.vms.confirming.is_some());
    state.poll_vms();
    assert_eq!(
        state.vms.pending.as_ref().map(|pending| pending.operation),
        Some(Operation::List)
    );
    let snapshot = state.vms.snapshot.clone();
    assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
    assert!(
        state.vms.confirming.is_some(),
        "a listing that keeps the lease keeps the question"
    );
    assert!(state.vm_key(key(BareKey::Char('y'))));
    assert_eq!(
        state.vms.pending.as_ref().map(|pending| pending.operation),
        Some(Operation::Close)
    );
}

/// agent's late answer to `request`.
fn late(request: &Request, output: &str) -> ResultMessage {
    ResultMessage {
        request_id: request.request_id,
        operation: request.operation,
        target: request.target.clone(),
        snapshot: None,
        output: output.into(),
        error: String::new(),
    }
}

/// While agent says a request given up on is still running, `c`, `s` and `x` are held as
/// they are while one is out: sent, they would only be refused. The late reply lets them
/// go again.
#[test]
fn keys_are_held_while_a_timed_out_request_is_busy() {
    let mut state = state();
    let sync = expired_sync(&mut state);
    let one = state.vms.snapshot.vms[0].identity();
    assert!(state.vm_key(key(BareKey::Char('c'))));
    assert_eq!(state.vms.queued, Some((Operation::Changes, one)));
    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
    assert!(state.vms.busy);
    assert!(state.vms.pending.is_none());
    assert!(state.vms.queued.is_some(), "not sent into a known refusal");

    state.vms.queued = None;
    for c in ['c', 's', 'x'] {
        assert!(state.vm_key(key(BareKey::Char(c))));
        assert!(state.vms.pending.is_none(), "{c}");
        assert!(state.vms.confirming.is_none(), "{c}");
    }
    assert!(details(&state).contains("No reply after 60 s"));
    state.render(19, 40);
    let label = state.vm_divider_label(40, false);
    assert!(label.starts_with("Syncing "), "{label}");

    assert!(state.accept_vm_result(late(&sync, "Worktree sync completed.")));
    let snapshot = state.vms.snapshot.clone();
    assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
    assert!(!state.vms.busy);
    assert!(state.vm_key(key(BareKey::Char('c'))));
    assert_eq!(
        state.vms.pending.as_ref().map(|pending| pending.operation),
        Some(Operation::Changes)
    );
}

/// A request queued while agent is still busy waits out the refusals, and goes once the
/// late reply is in and a listing answers - `c` pressed behind the listing, and a `y` armed
/// just before agent said it was busy.
#[test]
fn a_request_queued_during_busy_goes_out_after_the_late_reply() {
    for (press, operation) in [
        (&['c'][..], Operation::Changes),
        (&['x', 'y'][..], Operation::Close),
    ] {
        let mut state = state();
        let sync = expired_sync(&mut state);
        let (arm, answer) = press.split_at(press.len() - 1);
        for &c in arm {
            state.vm_key(key(BareKey::Char(c)));
        }
        if !arm.is_empty() {
            assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        }
        state.vm_key(key(BareKey::Char(answer[0])));
        if arm.is_empty() {
            assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        }
        assert!(state.vms.busy);
        assert!(state.vms.pending.is_none(), "{operation:?}");
        assert_eq!(
            state.vms.queued.as_ref().map(|(queued, _)| *queued),
            Some(operation)
        );
        // Still busy at the next tick: still queued.
        state.poll_vms();
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        assert!(state.vms.queued.is_some());

        assert!(state.accept_vm_result(late(&sync, "Worktree sync completed.")));
        let snapshot = state.vms.snapshot.clone();
        assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
        assert_eq!(
            state.vms.pending.as_ref().map(|pending| pending.operation),
            Some(operation)
        );
        assert!(state.vms.queued.is_none());
    }
}

/// The same queue with the tab out of sight when the late reply comes - the panel put
/// away, or another tab chosen: the request still goes once agent has finished, behind a
/// listing that checks its lease, rather than at the next visit to the tab, which can be
/// hours after the user confirmed it.
#[test]
fn a_request_queued_during_busy_goes_out_after_the_late_reply_while_hidden() {
    for leave in ["hide", "switch"] {
        for (press, operation) in [
            (&['c'][..], Operation::Changes),
            (&['x', 'y'][..], Operation::Close),
        ] {
            let mut state = state();
            let sync = expired_sync(&mut state);
            let (arm, answer) = press.split_at(press.len() - 1);
            for &c in arm {
                state.vm_key(key(BareKey::Char(c)));
            }
            if !arm.is_empty() {
                assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
            }
            state.vm_key(key(BareKey::Char(answer[0])));
            if arm.is_empty() {
                assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
            }
            assert!(state.vms.busy, "{leave} {operation:?}");
            assert_eq!(
                state.vms.queued.as_ref().map(|(queued, _)| *queued),
                Some(operation)
            );
            match leave {
                "hide" => {
                    state.vms.visible = false;
                    state.end_visit();
                },
                _ => {
                    state.update(Event::Visible(false));
                },
            }
            // Still busy: the tick sends nothing out of sight, and the request waits.
            state.poll_vms();
            assert!(state.vms.pending.is_none(), "{leave} {operation:?}");
            assert!(state.vms.queued.is_some());

            assert!(state.accept_vm_result(late(&sync, "Worktree sync completed.")));
            assert_eq!(
                state.vms.pending.as_ref().map(|pending| pending.operation),
                Some(Operation::List),
                "{leave} {operation:?}: the listing the queued request waits on"
            );
            let snapshot = state.vms.snapshot.clone();
            assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
            assert_eq!(
                state.vms.pending.as_ref().map(|pending| pending.operation),
                Some(operation),
                "{leave}"
            );
            assert!(state.vms.queued.is_none());
            // Its answer is taken out of sight too, and nothing more goes out after it.
            assert!(state.accept_vm_result(reply(&state, None, "done", "")));
            assert!(state.vms.pending.is_none(), "{leave} {operation:?}");
            state.poll_vms();
            assert!(state.vms.pending.is_none(), "{leave} {operation:?}");
        }
    }
}

/// At the width a divider has in a pane at its default half, what each label cannot do
/// without is what shows - the answer keys, the seconds, the key that fixes things -
/// however long the VM's name.
#[test]
fn vm_divider_keeps_keys_and_seconds_at_the_default_half() {
    let drawn = |state: &State| {
        let label = state.vm_divider_label(24, false);
        divider_text(&label, 0, 5, false, 24).content().to_owned()
    };
    for name in ["9cc0eea7664ed86b", "yinz-agent-18a7b3c4d5e6f708-1"] {
        let mut state = state();
        state.vms.snapshot.vms[0].entry.name = name.into();
        state.render(19, 24);
        state.vm_key(key(BareKey::Char('x')));
        assert!(drawn(&state).contains("y close"), "{}", drawn(&state));
        state.vm_key(key(BareKey::Char('n')));
        state.vm_key(key(BareKey::Char('s')));
        assert!(drawn(&state).contains("y sync"), "{}", drawn(&state));
        state.vm_key(key(BareKey::Char('n')));

        let identity = state.vms.snapshot.vms[0].identity();
        assert!(state.vm_request(Operation::Sync, Some(identity.clone())));
        state.vms.pending_started_ms = now_ms() - 12_000;
        assert!(drawn(&state).contains("12s"), "{}", drawn(&state));
        state.vms.pending = None;

        state.vms.queued = Some((Operation::Close, identity));
        assert!(drawn(&state).contains("Queued"), "{}", drawn(&state));
        state.vms.queued = None;

        state.vms.owner = None;
        assert!(
            drawn(&state).contains("VM command run"),
            "{}",
            drawn(&state)
        );
    }
}

/// A lease's row leads with the VM's name, so rows of one session's VMs differ where they
/// start, and a generated name too long for the row keeps the end that tells it apart.
#[test]
fn vm_rows_name_the_vm_and_fit_beside_the_marker() {
    let mut state = state();
    state.vms.snapshot = serde_json::from_str(
        r#"{"vms":[{"session":"one","name":"api","state":"running"},{"session":"one","name":"web","state":"running"}]}"#,
    )
    .unwrap();
    assert_eq!(
        state.vm_rows(24),
        ["api / one  running  —", "web / one  running  —"]
    );

    state.vms.snapshot = serde_json::from_str(
        r#"{"vms":[{"session":"brave-lemur","name":"yinz-agent-186a4f3b2c1d0e9f-1","state":"running"},{"session":"brave-lemur","name":"yinz-agent-186a4f3b2c1d0e9f-2","state":"running"}]}"#,
    )
    .unwrap();
    for cols in 12..80 {
        let rows = state.vm_rows(cols);
        for row in &rows {
            assert!(2 + row.width() <= cols, "{cols}: {row}");
        }
        if cols >= 13 {
            assert_ne!(rows[0], rows[1], "{cols}");
        }
    }
    assert!(details(&state).starts_with("VM: yinz-agent-186a4f3b2c1d0e9f-1 · session brave-lemur"));
}

/// Too short to split, the status row gives a failure's reason rather than the CLI's
/// `VM command failed (exit N):` in front of it; the details keep every word.
#[test]
fn a_short_pane_status_row_names_a_failures_reason() {
    for error in [
        "VM command failed (exit 1): Error: guest agent unreachable",
        "VM command failed (exit 1): syncing 3 files\nError: guest agent unreachable",
    ] {
        let mut state = state();
        state.render(6, 60);
        assert_eq!(state.vms.last_details_rows, 0);
        let one = state.vms.snapshot.vms[0].identity();
        assert!(state.vm_request(Operation::Sync, Some(one)));
        assert!(state.accept_vm_result(reply(&state, None, "", error)));
        let label = state.vm_divider_label(60, true);
        assert!(
            label.starts_with("Sync failed: guest agent unreachable"),
            "{label}"
        );
        assert!(details(&state).contains(error));
    }
}

/// A reply is taken only from the agent a request went to, so a change of owner says, for
/// each request left out, that its answer will not come here - not a failure, since it may
/// have run - rather than leaving a promise that the result will replace the line.
#[test]
fn a_listing_stuck_past_the_deadline_is_not_called_another_request() {
    let mut state = state();
    state.poll_vms();
    assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
    for _ in 0..2 {
        state.poll_vms();
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        state.render(19, 40);
        assert_eq!(
            state.vm_divider_label(40, false),
            "STALE: VM listing did not answer in 60 s; retrying."
        );
        assert!(!state
            .vm_detail_rows(200)
            .0
            .join("\n")
            .contains("Another VM request"));
        assert!(state.vms.has_error());
    }
    state.poll_vms();
    let snapshot = state.vms.snapshot.clone();
    assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
    assert!(state.vms.error.is_empty());
    state.poll_vms();
    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
    assert_eq!(state.vms.error, vms::BUSY);
}

/// While the panel's tab bar has the keys the divider names the bar's, on the row it has
/// anyway, and the list and details keep every row they had.
#[test]
fn the_bar_hint_takes_the_vm_divider_not_a_row() {
    let mut state = state();
    state.render(11, 60);
    let halves = |state: &State| (state.vms.last_list_rows, state.vms.last_details_rows);
    let unfocused = halves(&state);
    assert!(!state.vm_divider_label(60, false).contains("←/→ choose"));
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    state.render(11, 60);
    assert_eq!(halves(&state), unfocused);
    let label = state.vm_divider_label(60, false);
    assert!(label.contains("←/→ choose"), "{label}");
    state.tabs_focused = false;
    state.bar_focused = vec![];
    state.render(11, 60);
    assert_eq!(halves(&state), unfocused);
    assert!(!state.vm_divider_label(60, false).contains("←/→ choose"));

    // With no agent to ask, the bar's keys still come first while it has them.
    state.vms.owner = None;
    assert_eq!(state.vm_divider_label(60, false), NO_OWNER_DIVIDER);
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    let label = state.vm_divider_label(60, false);
    assert!(label.contains("←/→ choose"), "{label}");
}

/// A status on the divider - a request queued, running or still running after its
/// timeout, or a listing that failed - still leads it while the bar has the keys, and the
/// bar's keys follow it: the divider is the one row on the tab that names keys, so the
/// status taking all of it left the bar's named nowhere.
#[test]
fn a_vm_status_keeps_the_bar_hint_while_the_bar_has_the_keys() {
    for case in ["stale", "empty", "queued", "pending", "still running"] {
        let mut state = state();
        state.render(11, 60);
        let one = state.vms.snapshot.vms[0].identity();
        match case {
            "stale" => state.vms.error = "provider denied".into(),
            "empty" => {
                state.vms.snapshot = Snapshot::default();
                state.vms.error = "provider denied".into();
            },
            "queued" => state.vms.queued = Some((Operation::Sync, one)),
            "pending" => assert!(state.vm_request(Operation::Sync, Some(one))),
            _ => {
                expired_sync(&mut state);
                assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
                assert!(state.vms.busy);
            },
        }
        let own = state.vm_divider_label(60, false);
        let expected = match case {
            "stale" => "STALE: provider denied",
            "empty" => "STALE · Ctrl-r retry",
            "queued" => "Queued: ",
            "pending" => "Syncing ",
            _ => ", still running",
        };
        assert!(own.contains(expected), "{case}: {own}");
        assert!(!own.contains("←/→ choose"), "{case}: {own}");

        // A running request's seconds can tick between two labels, so those are compared
        // by the status's text rather than whole.
        let timeless = matches!(case, "stale" | "empty" | "queued");
        state.tabs_focused = true;
        state.bar_focused = vec![state.own_client_id];
        let label = state.vm_divider_label(60, false);
        let bar = label.find("←/→ choose");
        assert!(
            bar.is_some_and(|at| label[..at].contains(expected)),
            "{case}: {label}"
        );
        assert!(!timeless || label.starts_with(&own), "{case}: {label}");
        state.render(11, 60);
        assert_eq!(
            state.footer_hint(),
            None,
            "{case}: said once, on the divider"
        );
        state.tabs_focused = false;
        state.bar_focused = vec![];
        let back = state.vm_divider_label(60, false);
        assert!(!back.contains("←/→ choose"), "{case}: {back}");
        assert!(!timeless || back == own, "{case}: {back}");
    }
}

/// A quiet list - nothing listed and nothing to say under it - has no divider, so while the
/// tab bar has the keys it gets the one row that names them, and gives it back after. With
/// no agent to ask, the placeholder says so and the row still names the bar's keys, rather
/// than the same complaint twice.
#[test]
fn a_quiet_vm_list_gives_the_bar_hint_a_row_while_the_bar_has_the_keys() {
    for owner in [Some(PluginId::new(1)), None] {
        let mut state = State {
            ..Default::default()
        };
        state.vms.owner = owner;
        state.render(11, 60);
        assert_eq!(state.vms.last_list_rows, 11, "{owner:?}");
        assert_eq!(state.vms.last_details_rows, 0);
        state.tabs_focused = true;
        state.bar_focused = vec![state.own_client_id];
        state.render(11, 60);
        assert_eq!(state.vms.last_list_rows, 10, "{owner:?}");
        let label = state.vm_divider_label(60, true);
        assert!(label.contains("←/→ choose"), "{owner:?}: {label}");
        assert_eq!(
            state.footer_hint(),
            None,
            "said once, not on a footer as well"
        );
        state.tabs_focused = false;
        state.bar_focused = vec![];
        state.render(11, 60);
        assert_eq!(state.vms.last_list_rows, 11, "{owner:?}");
    }
}

#[test]
fn a_whole_panel_show_does_not_poll_an_inactive_vm_child() {
    let mut state = state();
    state.pane_shown = false;
    state.vms.visible = false;
    let show = PipeMessage::new(
        PipeSource::Plugin(PluginId::new(1)),
        "vm_panel_visibility",
        &Some("true".into()),
        &None,
        true,
    );
    state.vm_pipe(&show);
    state.poll_vms();
    assert!(!state.vms.visible);
    assert!(state.vms.pending.is_none());
}

#[test]
fn queued_bar_keys_and_paste_stay_out_of_content() {
    let mut state = State::default();
    state.take_ring_stop(Some(agent_proto::vms::STOP_TABS));
    for key in [
        BareKey::Right,
        BareKey::Right,
        BareKey::Left,
        BareKey::Down,
        BareKey::Char('j'),
        BareKey::Char('/'),
        BareKey::Char('?'),
    ] {
        assert!(state.on_key(KeyWithModifier::new(key)));
        assert!(state.tabs_focused);
    }
    assert!(!state.on_paste("queued paste"));
    assert!(state.tabs_focused);
    for key in [BareKey::Enter, BareKey::Tab, BareKey::Esc] {
        state.take_ring_stop(Some(agent_proto::vms::STOP_TABS));
        assert!(state.on_key(KeyWithModifier::new(key)));
        assert!(!state.tabs_focused);
    }
}

#[test]
fn execution_rejects_replaced_leases_and_preserves_correlated_failures() {
    let snapshot: Snapshot = serde_json::from_str(
        r#"{"vms":[{"session":"work","name":"api","vm_id":"old","state":"running"}]}"#,
    )
    .unwrap();
    let request = Request {
        request_id: 7,
        operation: Operation::Close,
        target: Some(snapshot.vms[0].identity()),
    };
    let mut state = State {
        granted: true,
        ..Default::default()
    };
    state.vms.snapshot = snapshot;
    state.vms.pending = Some(request.clone());
    state.vm_panel_pending = Some(request);
    let mut context = BTreeMap::from([
        ("vm_panel".into(), "wrong".into()),
        ("vm_panel_validate".into(), "true".into()),
    ]);
    assert!(!state.on_vm_panel_result(Some(0), b"{}", b"", &context));
    assert!(state.vm_panel_pending.is_some());
    context.insert("vm_panel".into(), "7".into());
    assert!(state.on_vm_panel_result(
        Some(0),
        br#"{"vms":[{"session":"work","name":"api","vm_id":"replacement","state":"running"}]}"#,
        b"",
        &context
    ));
    assert!(state.vm_panel_pending.is_none());
    assert_eq!(
        state.vms.snapshot.vms[0].entry.vm_id.as_deref(),
        Some("old")
    );
    assert!(state.vms.results[0].failed);
    assert!(state.vms.results[0].text.contains("changed or disappeared"));
}

#[test]
fn direct_runner_reports_permissions_and_keeps_its_inflight_request() {
    let mut state = state();
    let request = Request {
        request_id: 2,
        operation: Operation::List,
        target: None,
    };
    state.granted = false;
    state.vms.pending = Some(request.clone());
    state.execute_vm_request(request.clone());
    assert!(state.vms.pending.is_none());
    assert!(state.vms.error.contains("Permission"));
    state.granted = true;
    state.vms.pending = Some(request.clone());
    state.execute_vm_request(request.clone());
    assert_eq!(state.vm_panel_pending.as_ref().unwrap().request_id, 2);
    let later = Request {
        request_id: 3,
        ..request
    };
    state.vms.pending = Some(later.clone());
    state.execute_vm_request(later);
    assert_eq!(state.vm_panel_pending.as_ref().unwrap().request_id, 2);
    assert_eq!(state.vms.error, vms::BUSY);
    state.granted = false;
    state.vms.pending = state.vm_panel_pending.clone();
    assert!(state.on_vm_panel_result(
        Some(0),
        b"{}",
        b"",
        &BTreeMap::from([("vm_panel".into(), "2".into())])
    ));
    assert!(state.vm_panel_pending.is_none());
    assert!(state.vms.pending.is_none());
    assert!(state.vms.error.contains("revoked"));
}
