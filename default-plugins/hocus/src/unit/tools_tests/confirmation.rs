use super::*;

#[test]
fn confirmation_policy_lists_tools_correctly() {
    for name in [
        "rerun_pane",
        "clear_pane",
        "send_input",
        "agent_send",
        "agent_control",
        // Forgetting a project is the one project tool that takes
        // something away.
        "project_forget",
        "slack_post",
    ] {
        assert!(needs_confirmation(name), "{name} should be confirmed");
    }
    for name in [
        // Answers per call rather than per name: a command over ssh stops, a
        // bare login shell does not. See `call_needs_confirmation`.
        "new_remote_pane",
        "list_session",
        // Listing is always free, including the two that look outward.
        "list_other_sessions",
        "list_orphaned_panes",
        // Pulling something in does not ask: it arrives in the session that
        // asked, so the result is visible the moment it happens, and there is
        // no destination to get wrong. The source can still lose by it — a
        // session that gives away its last tab ends — and that is the accepted
        // price of not stopping, not an omission.
        "warp_pane_here",
        "warp_tab_here",
        "adopt_orphaned_pane",
        "read_pane",
        "new_pane",
        "new_tab",
        "focus",
        "rename",
        "move_pane",
        "swap_panes",
        "resize_pane",
        "toggle_pane",
        "move_panes_to_tab",
        "stack_panes",
        "set_pane_pinned",
        "set_floating_pane_geometry",
        "scroll_pane",
        "web_search",
        "schedule",
        "unschedule",
        "agent_register",
        "agent_list",
        "close_pane",
        "close_tab",
        // A todo tool only writes to a file of the user's own notes.
        "todo_add",
        "todo_list",
        "todo_search",
        "todo_done",
        "todo_edit",
        // As does a note tool.
        "note_add",
        "note_list",
        "note_get",
        "note_edit",
        "note_archive",
        "note_delete",
        "note_link",
        "note_search",
        // Reading Slack is free; only posting stops for a y/n.
        "slack_status",
        "slack_search",
        "slack_read",
        "slack_backfill",
        // Reading the project's own files changes nothing, and the root
        // check is what keeps them the project's own.
        "find_files",
        "search_files",
        "read_file",
        "git_log",
        "git_show",
        "github_read",
        // Finding a line in a plugin is reading it.
        "plugin_search",
        "plugin_outline",
        "plugin_test",
        // Loading a family changes nothing but what the next request advertises.
        LOAD_TOOL,
        "tool_result_read",
    ] {
        assert!(!needs_confirmation(name), "{name} should not be confirmed");
    }
}

/// The confirmation prompt is the only thing standing between the model and
/// a closed pane, so every advertised tool must be in the table above.
#[test]
fn every_advertised_tool_is_covered_by_the_confirmation_table() {
    let specs = specs();
    let names: Vec<&str> = specs
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 121);
    let confirmed = names.iter().filter(|name| needs_confirmation(name)).count();
    assert_eq!(confirmed, 27);
}

/// The invariant the name table cannot state on its own: whatever decides to
/// stop must also be able to say why.
///
/// `every_confirmed_tool_says_what_it_will_do` walks the *name* table, so it has
/// never covered the tools that answer per call — `schedule` was already outside
/// it, and `new_remote_pane` has just joined. This walks the decision the tool
/// loop actually makes instead, which is the one that has to match.
#[test]
fn whatever_stops_for_a_confirmation_can_say_what_it_will_do() {
    let mut calls = vec![
        call(
            "new_remote_pane",
            r#"{"remote_host":"box","command":"htop"}"#,
        ),
        call("new_remote_pane", r#"{"remote_host":"box"}"#),
        call("new_remote_pane", r#"{"remote_host":"box","command":""}"#),
        call("new_remote_pane", "not json at all"),
        // `new_pane` answers per call the same way: a named command stops, a bare
        // pane does not, and a call nobody can read stops.
        call("new_pane", r#"{"command":"htop"}"#),
        call("new_pane", r#"{"command":["bash","-lc","id"]}"#),
        call("new_pane", r#"{"command":""}"#),
        call("new_pane", r#"{"command":"","args":["htop"]}"#),
        call("new_pane", r#"{"command":["","htop"]}"#),
        call("new_pane", "not json at all"),
        call("schedule", r#"{"prompt":"check ci","auto_confirm":true}"#),
        call("schedule", r#"{"prompt":"check ci"}"#),
    ];
    // Every other tool, with nothing to go on but its name.
    calls.extend(names().iter().map(|name| call(name, "{}")));
    for tool_call in &calls {
        assert_eq!(
            call_needs_confirmation(tool_call),
            consequence(tool_call).is_some(),
            "{} {} disagrees about whether it stops",
            tool_call.name,
            tool_call.arguments,
        );
    }
}

/// Arguments the model got wrong are not a bare shell — they are a call nobody
/// can read, and the one case that most needs a human to look at it.
#[test]
fn a_remote_pane_whose_arguments_do_not_parse_still_stops() {
    assert!(call_needs_confirmation(&call(
        "new_remote_pane",
        "not json at all"
    )));
}

/// The same guard as the confirmation table above, one layer along: a tool
/// that stops to ask and then cannot say what it will do is a prompt with
/// nothing in it but the model's JSON.
#[test]
fn every_confirmed_tool_says_what_it_will_do() {
    for name in names().iter().filter(|name| needs_confirmation(name)) {
        let sentence = consequence(&call(name, "{}"));
        assert!(sentence.is_some(), "{name} should say what it will do");
        assert!(
            !sentence.expect("checked").trim().is_empty(),
            "{name}'s consequence should not be blank"
        );
    }
}

#[test]
fn a_tool_that_never_asks_has_no_consequence() {
    assert_eq!(consequence(&call("list_session", "{}")), None);
}

/// `schedule` is not in the confirmation table, but asking for standing
/// permission stops `pump_tools` all the same - and only then.
#[test]
fn scheduling_only_says_anything_when_it_asks_for_standing_permission() {
    assert_eq!(
        consequence(&call(
            "schedule",
            r#"{"prompt":"check ci","auto_confirm":true}"#
        )),
        Some("lets every run of this job use tools without stopping to ask again".into())
    );
    assert_eq!(
        consequence(&call("schedule", r#"{"prompt":"check ci"}"#)),
        None
    );
}

#[test]
fn a_consequence_names_the_pane_it_will_act_on() {
    assert_eq!(
        consequence(&call("send_input", r#"{"pane_id":3,"text":"hi"}"#)),
        Some("types into pane 3, as if you had typed it yourself".into())
    );
}

#[test]
fn a_remote_pane_consequence_names_the_host() {
    assert_eq!(
        consequence(&call(
            "new_remote_pane",
            r#"{"remote_host":"box","command":"htop"}"#
        )),
        Some("runs a command on box over ssh, opening a new pane connected to it".into())
    );
}

/// A bare login shell never stops, so it has nothing to say — the same shape as
/// `scheduling_only_says_anything_when_it_asks_for_standing_permission`.
#[test]
fn a_remote_pane_only_says_anything_when_it_names_a_command() {
    assert!(!call_needs_confirmation(&call(
        "new_remote_pane",
        r#"{"remote_host":"box"}"#
    )));
    assert_eq!(
        consequence(&call("new_remote_pane", r#"{"remote_host":"box"}"#)),
        None
    );
    // An empty command is a command in form only: it reaches the far machine as
    // a login shell, so it must not stop as though it were one.
    assert!(!call_needs_confirmation(&call(
        "new_remote_pane",
        r#"{"remote_host":"box","command":""}"#
    )));
    assert_eq!(
        consequence(&call(
            "new_remote_pane",
            r#"{"remote_host":"box","command":""}"#
        )),
        None
    );
    assert!(call_needs_confirmation(&call(
        "new_remote_pane",
        r#"{"remote_host":"box","command":"htop"}"#
    )));
}

/// A local `new_pane` that names a command runs a model-chosen program on this
/// machine, so it must stop for a y/n — leaving it unconfirmed was a
/// prompt-injection path straight to local execution. A bare pane (a shell the
/// user drives) still does not stop, exactly like `new_remote_pane`.
#[test]
fn a_local_pane_stops_only_when_it_names_a_command() {
    // Bare pane: no command, no prompt.
    assert!(!call_needs_confirmation(&call("new_pane", "{}")));
    assert_eq!(consequence(&call("new_pane", "{}")), None);
    // An empty command is a shell in form only — treat it as the bare case.
    assert!(!call_needs_confirmation(&call(
        "new_pane",
        r#"{"command":""}"#
    )));
    assert_eq!(consequence(&call("new_pane", r#"{"command":""}"#)), None);
    // A named command stops and says what it will run.
    assert!(call_needs_confirmation(&call(
        "new_pane",
        r#"{"command":"htop"}"#
    )));
    assert_eq!(
        consequence(&call("new_pane", r#"{"command":"htop"}"#)),
        Some("opens a new pane running `htop` on this machine".into())
    );
    // The whole argv sent as one array is the other accepted shape.
    assert!(call_needs_confirmation(&call(
        "new_pane",
        r#"{"command":["bash","-lc","id"]}"#
    )));
    assert_eq!(
        consequence(&call("new_pane", r#"{"command":["bash","-lc","id"]}"#)),
        Some("opens a new pane running `bash` on this machine".into())
    );
}

/// A leading empty word is skipped, not read as the program: the prompt names
/// the program that will actually run. The dispatcher reads the same
/// `program_and_args`, so a call confirmed as `htop` cannot open a bare shell.
#[test]
fn a_leading_empty_word_does_not_hide_the_command_from_the_prompt() {
    for arguments in [
        r#"{"command":"","args":["htop"]}"#,
        r#"{"command":["","htop"]}"#,
    ] {
        assert!(
            call_needs_confirmation(&call("new_pane", arguments)),
            "{arguments}"
        );
        assert_eq!(
            consequence(&call("new_pane", arguments)),
            Some("opens a new pane running `htop` on this machine".into()),
            "{arguments}"
        );
    }
}

/// Arguments the model got wrong are not a bare pane — they are a call nobody
/// can read, the one case that most needs a human to look at it.
#[test]
fn a_local_pane_whose_arguments_do_not_parse_still_stops() {
    assert!(call_needs_confirmation(&call(
        "new_pane",
        "not json at all"
    )));
}

/// `agent_control`'s argument is a verb, and interrupting is not restarting.
#[test]
fn agent_control_says_which_of_its_verbs_is_about_to_run() {
    let restart = consequence(&call(
        "agent_control",
        r#"{"pane_id":7,"action":"restart"}"#,
    ));
    assert_eq!(
        restart,
        Some("restarts the agent in pane 7, losing its conversation".into())
    );
    let close = consequence(&call("agent_control", r#"{"pane_id":7,"action":"close"}"#));
    assert_eq!(
        close,
        Some("closes pane 7 and the agent running in it".into())
    );
}

/// Arguments the model got wrong are exactly when a prompt must not go quiet.
#[test]
fn a_consequence_survives_arguments_that_do_not_parse() {
    let sentence = consequence(&call("send_input", "not json at all"));
    assert_eq!(
        sentence,
        Some("types into a pane, as if you had typed it yourself".into())
    );
}

#[test]
fn arguments_are_one_readable_line_each_rather_than_json() {
    assert_eq!(
        argument_lines(&call("send_input", r#"{"pane_id":3,"text":"git push"}"#)),
        vec!["pane_id: 3", "text: git push"]
    );
}

#[test]
fn multiline_arguments_show_every_line_as_a_block() {
    assert_eq!(
        argument_lines(&call(
            "send_input",
            r#"{"pane_id":3,"text":"git push\nwhen done\n"}"#,
        )),
        vec!["pane_id: 3", "text:", "  git push", "  when done", "  "]
    );
}

#[test]
fn an_empty_argument_object_contributes_no_lines() {
    assert!(argument_lines(&call("clear_pane", "{}")).is_empty());
    assert!(argument_lines(&call("clear_pane", "")).is_empty());
}

#[test]
fn arguments_that_do_not_parse_are_shown_verbatim_rather_than_dropped() {
    assert_eq!(
        argument_lines(&call("send_input", "not json at all")),
        vec!["not json at all"]
    );
}

#[test]
fn the_confirmation_block_names_the_call_what_it_does_and_what_it_was_given() {
    assert_eq!(
        confirmation_detail(&call("clear_pane", r#"{"pane_id":4}"#)),
        vec![
            "clear_pane",
            "  throws away pane 4's scrollback; it cannot be scrolled back",
            "  pane_id: 4",
        ]
    );
}

/// A toast is read from a pane that has nothing to do with hocus, so it has
/// to say who is asking before it says what for.
#[test]
fn the_toast_says_who_is_asking_before_the_block() {
    let toast = confirmation_toast(&call("clear_pane", r#"{"pane_id":4}"#));
    let mut lines = toast.lines();
    assert_eq!(lines.next(), Some(TOAST_HEADLINE));
    assert_eq!(lines.next(), Some("clear_pane"));
    assert!(toast.ends_with("  pane_id: 4"), "got {toast:?}");
}

// -- naming the destination ---------------------------------------------

/// `is_plugin` decides which pane `pane_id` even means, so the target has to
/// read it rather than assume a terminal. Pointing at terminal 4 when the call
/// is about plugin 4 would mark the wrong pane and reveal the wrong tab.
#[test]
fn the_target_respects_which_kind_of_pane_the_id_is() {
    assert_eq!(
        confirmation_target(&call("send_input", r#"{"pane_id":4,"is_plugin":false}"#)),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(4)))
    );
    assert_eq!(
        confirmation_target(&call("clear_pane", r#"{"pane_id":4,"is_plugin":true}"#)),
        Some(PaneId::Plugin(zellij_tile::prelude::PluginId::new(4)))
    );
}

/// The agent tools address a terminal and don't take `is_plugin` at all —
/// `agent_host_call` rewrites them into primitives with `is_plugin: false`.
#[test]
fn an_agent_call_targets_its_terminal_without_being_told() {
    assert_eq!(
        confirmation_target(&call("agent_send", r#"{"pane_id":9,"text":"hi"}"#)),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(9)))
    );
    assert_eq!(
        confirmation_target(&call("agent_control", r#"{"pane_id":9,"action":"close"}"#)),
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(9)))
    );
}

/// Every confirmable tool that does *not* name a pane in this session: it acts
/// on a channel, a project, a machine of its own, so there is nothing local for
/// the session to go and show.
#[test]
fn a_tool_with_no_local_pane_has_no_target() {
    for (name, arguments) in [
        ("slack_post", r#"{"channel":"general"}"#),
        ("project_forget", r#"{"name":"thing"}"#),
        ("agent_spawn", r#"{"command":"claude"}"#),
        ("new_remote_pane", r#"{"remote_host":"box"}"#),
    ] {
        assert_eq!(
            confirmation_target(&call(name, arguments)),
            None,
            "{name} names no pane in this session"
        );
    }
}

/// Same rule as `consequence`: the prompt still goes up on a call the model got
/// wrong, it just goes up without a destination to point at.
#[test]
fn a_target_is_absent_rather_than_guessed_when_the_arguments_are_wrong() {
    assert_eq!(confirmation_target(&call("send_input", "not json")), None);
    // Parses, but `with_pane` would refuse it too: which pane is unanswered.
    assert_eq!(
        confirmation_target(&call("send_input", r#"{"pane_id":4}"#)),
        None
    );
    assert_eq!(
        confirmation_target(&call("send_input", r#"{"text":"hi"}"#)),
        None
    );
}

/// Read from the pane, with no sidebar in sight, the label is all there is —
/// so it has to name who is asking and that an answer is owed.
#[test]
fn the_label_says_whose_question_the_pane_is_wearing() {
    assert_eq!(
        indicator_label(&call("send_input", r#"{"pane_id":4}"#)),
        "hocus: send_input?"
    );
}

/// The git half of `agent_spawn` is the half that writes to the user's repository,
/// so the sentence they are asked to agree to has to name it. A confirmation
/// reading "opens a new pane running `claude`" for a call that is about to add a
/// worktree is a confirmation of the wrong thing.
#[test]
fn spawning_an_agent_on_a_branch_says_where_the_branch_and_the_worktree_go() {
    let worktree = consequence(&call(
        "agent_spawn",
        r#"{"agent":"claude","branch":"fix-parser","prompt":"fix the parser"}"#,
    ))
    .unwrap();
    assert!(worktree.contains("running `claude`"), "{worktree}");
    assert!(worktree.contains(".worktrees/fix-parser"), "{worktree}");
    assert!(worktree.contains("`main`"), "{worktree}");

    let in_place = consequence(&call(
        "agent_spawn",
        r#"{"agent":"codex","branch":"topic","base":"develop","worktree":false,"prompt":"go"}"#,
    ))
    .unwrap();
    assert!(in_place.contains("switching this checkout"), "{in_place}");
    assert!(in_place.contains("`topic`"), "{in_place}");
    assert!(in_place.contains("`develop`"), "{in_place}");
    assert!(!in_place.contains("worktree"), "{in_place}");
}

/// Two things the sentence has to promise correctly now that the host decides them:
/// whether a taken name is joined or stepped around, and whether the agent will be
/// loose in the checkout the user is working in.
#[test]
fn the_sentence_says_whether_a_name_is_joined_and_whether_the_agent_is_left_loose() {
    let fresh = consequence(&call(
        "agent_spawn",
        r#"{"agent":"claude","branch":"fix-parser","prompt":"fix the parser"}"#,
    ))
    .unwrap();
    assert!(fresh.contains("the next free name"), "{fresh}");

    let reusing = consequence(&call(
        "agent_spawn",
        r#"{"agent":"claude","branch":"fix-parser","reuse":true,"prompt":"fix the parser"}"#,
    ))
    .unwrap();
    assert!(reusing.contains("existing or new"), "{reusing}");
    assert!(!reusing.contains("the next free name"), "{reusing}");

    let loose = consequence(&call(
        "agent_spawn",
        r#"{"agent":"claude","allow_outside_worktree":true,"prompt":"go"}"#,
    ))
    .unwrap();
    assert!(
        loose.contains("in the current checkout rather than a worktree of its own"),
        "{loose}"
    );
}

/// `cursor` is the one agent that is not its own program, and the sentence names
/// the program — telling someone you are about to run `cursor` when you are about
/// to run `cursor-agent` is naming the editor, not the agent.
#[test]
fn the_consequence_names_the_program_that_will_actually_run() {
    let sentence =
        consequence(&call("agent_spawn", r#"{"agent":"cursor","prompt":"go"}"#)).unwrap();
    assert!(sentence.contains("`cursor-agent`"), "{sentence}");
}

/// No git arguments, no git clause: `agent_spawn` still has its old, narrower
/// consequence for the call it always took.
#[test]
fn spawning_without_a_branch_promises_no_repository_changes() {
    let sentence = consequence(&call("agent_spawn", r#"{"command":"zsh","prompt":"go"}"#)).unwrap();
    assert_eq!(sentence, "opens a new pane running `zsh`");
}

/// Spawning with no prompt leaves the pane for a human to type into, the same
/// shape as a bare `new_pane` — it does not stop, and has nothing to say.
#[test]
fn spawning_without_a_prompt_does_not_stop() {
    assert!(!call_needs_confirmation(&call(
        "agent_spawn",
        r#"{"agent":"claude","branch":"fix-parser"}"#
    )));
    assert_eq!(
        consequence(&call(
            "agent_spawn",
            r#"{"agent":"claude","branch":"fix-parser"}"#
        )),
        None
    );
    // Blank counts as absent.
    assert!(!call_needs_confirmation(&call(
        "agent_spawn",
        r#"{"command":"zsh","prompt":"   "}"#
    )));
}

/// A prompt hands the new agent an instruction to act on immediately — that is
/// the confirmable part, regardless of any other argument.
#[test]
fn spawning_with_a_prompt_stops() {
    assert!(call_needs_confirmation(&call(
        "agent_spawn",
        r#"{"command":"zsh","prompt":"go"}"#
    )));
}

/// Arguments the model got wrong are not a bare spawn — they are a call nobody
/// can read, the one case that most needs a human to look at it.
#[test]
fn an_agent_spawn_whose_arguments_do_not_parse_still_stops() {
    assert!(call_needs_confirmation(&call(
        "agent_spawn",
        "not json at all"
    )));
}
