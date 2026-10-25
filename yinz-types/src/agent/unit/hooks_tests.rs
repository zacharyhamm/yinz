use super::*;

#[test]
fn worker_hooks_only_broadcast_without_a_parent_state_change() {
    for kind in ["claude", "codex", "muse"] {
        for name in [
            "SubagentStart",
            "SubagentStop",
            "Stop",
            "SessionEnd",
            "SessionStart",
        ] {
            let mut event = event(name);
            if !matches!(name, "SubagentStart" | "SubagentStop") {
                event.agent_id = Some("worker".into());
                event.source = Some("startup".into());
            }
            assert!(matches!(hook_outcome(kind, &event), HookOutcome::Nothing));
            let actions = crate::input::actions::hook_report_actions(
                kind,
                crate::pane_id::PaneId::Terminal(crate::internal_ids::TerminalId::new(9)),
                &event,
                serde_json::to_string(&event).unwrap(),
            );
            assert!(
                matches!(actions.as_slice(), [crate::input::actions::Action::AgentEvent(event)]
                    if event.hook == name)
            );
        }
    }
}

#[test]
fn the_hook_settings_wire_every_event_to_the_command() {
    let settings = claude_hook_settings("yinz action agent-report --hook-json");
    let hooks = settings["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), CLAUDE_HOOK_EVENTS.len());
    for event in CLAUDE_HOOK_EVENTS {
        let handler = &hooks[*event][0]["hooks"][0];
        assert_eq!(handler["type"], "command", "{event}");
        assert_eq!(handler["command"], "yinz action agent-report --hook-json");
        assert_eq!(handler["timeout"], CLAUDE_HOOK_TIMEOUT_SECONDS);
        assert!(
            hooks[*event][0].get("matcher").is_none(),
            "{event} fires on every occurrence"
        );
    }

    let command = "yinz action agent-report --kind codex --hook-json";
    let codex = codex_hook_settings(command);
    let hooks = codex["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), CODEX_HOOK_EVENTS.len());
    for event in CODEX_HOOK_EVENTS {
        assert_eq!(hooks[*event][0]["hooks"][0]["command"], command, "{event}");
        assert_eq!(
            hooks[*event][0]["hooks"][0]["timeout"], CODEX_HOOK_TIMEOUT_SECONDS,
            "{event}"
        );
    }
    assert!(hooks.get("Interrupt").is_some());
    assert!(hooks.get("Notification").is_none());
}

#[test]
fn the_muse_settings_wire_its_events_and_not_the_llm_firehose() {
    let command = "/root/.config/muse/yinz/yinz-hook.sh";
    let settings = muse_hook_settings(command);
    let hooks = settings["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), MUSE_HOOK_EVENTS.len());
    for event in MUSE_HOOK_EVENTS {
        let handler = &hooks[*event][0]["hooks"][0];
        assert_eq!(handler["type"], "command", "{event}");
        assert_eq!(handler["command"], command, "{event}");
        assert_eq!(handler["timeout"], MUSE_HOOK_TIMEOUT_SECONDS, "{event}");
        assert!(
            hooks[*event][0].get("matcher").is_none(),
            "{event} fires on every occurrence"
        );
    }
    assert!(hooks.get("Notification").is_some());
    assert!(hooks.get("PreLLMCall").is_none());
    assert!(hooks.get("PostLLMCall").is_none());
}

#[test]
fn an_env_wrapper_is_looked_through_for_the_agents_name() {
    let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        agent_kind_of(Path::new("/usr/bin/claude"), &[]).as_deref(),
        Some("claude")
    );
    assert_eq!(
        agent_kind_of(
            Path::new("env"),
            &args(&["FOO=1", "BAR=x=y", "claude", "--model", "opus"])
        )
        .as_deref(),
        Some("claude")
    );
    assert_eq!(
        agent_kind_of(
            Path::new("/usr/bin/env"),
            &args(&["-i", "-u", "HOME", "codex"])
        )
        .as_deref(),
        Some("codex")
    );
    assert_eq!(agent_kind_of(Path::new("env"), &args(&["FOO=1"])), None);
}

#[test]
fn a_agent_launcher_is_looked_through_for_the_agents_name() {
    let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    for launcher in ["codex", "muse"] {
        assert_eq!(
            agent_kind_of(
                Path::new("yinz"),
                &args(&[
                    "agent",
                    launcher,
                    "--command",
                    launcher,
                    "--",
                    "resume",
                    "--last"
                ])
            )
            .as_deref(),
            Some(launcher)
        );
        // The bare form runs the launcher's own default program.
        assert_eq!(
            agent_kind_of(
                Path::new("yinz"),
                &args(&["agent", launcher, "--", "--model", "x"])
            )
            .as_deref(),
            Some(launcher)
        );
        // Anything else under `agent` is maintenance, not an agent pane.
        assert_eq!(
            agent_kind_of(Path::new("yinz"), &args(&["agent", "hooks", "install"])).as_deref(),
            Some("yinz")
        );
    }
}

#[test]
fn palette_launchers_keep_provider_identity_with_default_and_explicit_commands() {
    for (launcher, provider) in [("claude", "claude"), ("codex-untracked", "codex")] {
        for arguments in [
            vec!["agent", launcher, "--", "prompt"],
            vec!["agent", launcher, "--command", provider, "--", "prompt"],
        ] {
            let args: Vec<String> = arguments.into_iter().map(str::to_owned).collect();
            assert_eq!(
                kind_from_program_and_args(Path::new("yinz"), &args).as_deref(),
                Some(provider)
            );
        }
    }
}

#[test]
fn a_sandboxed_pane_reports_the_command_inside_its_launcher_or_bwrap() {
    let args =
        |words: &[&str]| -> Vec<String> { words.iter().map(|word| (*word).to_owned()).collect() };
    let inner = args(&["env", "YINZ_AGENT_PANE=1", "claude", "--", "prompt"]);
    let behind = |prefix: &[&str]| [args(prefix), inner.clone()].concat();
    for argv in [
        // The launcher's own name is whatever this binary was installed as.
        behind(&["/opt/yinz-dev", BUBBLEWRAP_RUN_FLAG, "/project", "--"]),
        behind(&["bwrap", "--unshare-all", "--chdir", "/project", "--"]),
        behind(&["/usr/bin/bwrap", "--"]),
    ] {
        assert_eq!(bubblewrap_inner_argv(&argv), Some(inner.as_slice()));
    }
    for argv in [
        args(&["claude", "--", "prompt"]),
        args(&["bwrap", "--unshare-all"]),
        args(&["bwrap", "--"]),
        args(&["yinz", BUBBLEWRAP_RUN_FLAG, "/project", "claude"]),
    ] {
        assert_eq!(bubblewrap_inner_argv(&argv), None, "{argv:?}");
    }
}

#[test]
fn a_command_line_names_an_agent_by_its_first_word_or_behind_an_interpreter() {
    for (command, kind) in [
        ("claude", "claude"),
        ("claude-code --resume", "claude"),
        ("/usr/local/bin/Codex --resume", "codex"),
        ("/Users/x/.local/bin/muse-bin-0.1.0-R708.1 --resume", "muse"),
        ("node /opt/bin/gemini", "gemini"),
        ("pi-agent", "pi"),
        ("yinz agent codex --command codex -- resume --last", "codex"),
        ("yinz agent muse --command muse -- resume --last", "muse"),
    ] {
        assert_eq!(
            kind_from_command(command).as_deref(),
            Some(kind),
            "{command}"
        );
    }
    for command in [
        "zsh",
        "pipenv run pi",
        "git commit -m 'ask claude'",
        "yinz hooks install",
        "",
    ] {
        assert_eq!(kind_from_command(command), None, "{command}");
    }
}

#[test]
fn a_program_and_its_arguments_name_an_agent_through_env_and_an_interpreter() {
    let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        kind_from_program_and_args(Path::new("/usr/bin/claude"), &args(&["--model", "opus"]))
            .as_deref(),
        Some("claude")
    );
    assert_eq!(
        kind_from_program_and_args(Path::new("env"), &args(&["FOO=1", "codex", "--resume"]))
            .as_deref(),
        Some("codex")
    );
    assert_eq!(
        kind_from_program_and_args(
            Path::new("/usr/bin/env"),
            &args(&["node", "/opt/bin/gemini"])
        )
        .as_deref(),
        Some("gemini")
    );
    for (list, kind) in [
        (&["-C", "/repo", "node", "/opt/bin/claude"][..], "claude"),
        (&["-u", "HOME", "node", "/opt/bin/claude"][..], "claude"),
        (
            &["--chdir", "/repo", "python3", "-u", "/opt/bin/codex"][..],
            "codex",
        ),
        (
            &["--unset", "HOME", "node", "/opt/bin/gemini"][..],
            "gemini",
        ),
    ] {
        assert_eq!(
            kind_from_program_and_args(Path::new("env"), &args(list)).as_deref(),
            Some(kind),
            "env {list:?}"
        );
    }
    for (program_path, list) in [
        (
            "node",
            &["--max-old-space-size=4096", "/opt/bin/claude"][..],
        ),
        ("uvx", &["pi"]),
        ("uv", &["run", "pi"]),
        ("bun", &["run", "claude"]),
        ("deno", &["run", "--allow-all", "/opt/bin/claude.js"]),
    ] {
        assert!(
            kind_from_program_and_args(Path::new(program_path), &args(list)).is_some(),
            "{program_path} {list:?}"
        );
    }
    // Only the script counts behind an interpreter: this decides where a pane goes.
    for (program_path, list) in [
        ("python3", &["gemini_test.py", "--api", "gemini"][..]),
        ("node", &["build.js", "--preset", "claude-3"]),
        ("uv", &["run", "x", "--with", "pi"]),
        ("uv", &["--directory", "/work/claude", "run", "pytest"]),
        ("uv", &["--directory", "/work/project", "run", "claude"]),
        ("uv", &["run", "--with", "claude", "pytest"]),
        ("uv", &["--directory"]),
        ("uv", &["run", "run", "claude"]),
        ("node", &["--require", "/work/claude", "build.js"]),
        ("node", &["--unknown=value", "claude"]),
        ("node", &["-e", "claude"]),
        ("python3", &["-c", "claude"]),
        ("python3", &["-m", "claude"]),
        ("python3", &["run", "claude"]),
        ("node", &["python3", "claude"]),
        ("node", &["FOO=1", "claude"]),
        ("node", &[]),
        ("/bin/zsh", &["-c", "claude"]),
        ("env", &["FOO=1"]),
    ] {
        assert_eq!(
            kind_from_program_and_args(Path::new(program_path), &args(list)),
            None,
            "{program_path} {list:?}"
        );
    }
}

#[test]
fn codex_bootstrap_marker_survives_pruning_and_provider_hooks_omit_it() {
    for ready in [true, false] {
        let hook = HookEvent {
            codex_hooks_ready: Some(ready),
            ..event("SessionStart")
        };
        let pruned: HookEvent =
            serde_json::from_value(serde_json::to_value(hook.pruned_to(0)).unwrap()).unwrap();
        assert_eq!(pruned.codex_hooks_ready, Some(ready));
    }
    assert!(serde_json::to_value(event("SessionStart"))
        .unwrap()
        .get("_yinz_codex_hooks_ready")
        .is_none());
}

#[test]
fn muse_bootstrap_marker_survives_pruning_and_provider_hooks_omit_it() {
    for ready in [true, false] {
        let hook = HookEvent {
            muse_hooks_ready: Some(ready),
            ..event("SessionStart")
        };
        let pruned: HookEvent =
            serde_json::from_value(serde_json::to_value(hook.pruned_to(0)).unwrap()).unwrap();
        assert_eq!(pruned.muse_hooks_ready, Some(ready));
    }
    assert!(serde_json::to_value(event("SessionStart"))
        .unwrap()
        .get("_yinz_muse_hooks_ready")
        .is_none());
}

fn event(name: &str) -> HookEvent {
    HookEvent {
        hook_event_name: name.to_owned(),
        session_id: "s1".to_owned(),
        cwd: Some("/work".to_owned()),
        ..Default::default()
    }
}

fn notification(kind: &str) -> HookEvent {
    HookEvent {
        notification_type: Some(kind.to_owned()),
        ..event("Notification")
    }
}

#[test]
fn each_hook_event_names_the_state_it_means() {
    let start = |source: &str| HookEvent {
        source: Some(source.to_owned()),
        ..event("SessionStart")
    };
    assert_eq!(hook_state(&start("startup")), Some(AgentState::Idle));
    assert_eq!(hook_state(&start("resume")), Some(AgentState::Idle));
    let clear = start("clear");
    assert_eq!(hook_state(&clear), Some(AgentState::Idle));
    let HookOutcome::Reported(agent) = hook_outcome("claude", &clear) else {
        panic!("clear must restore the agent record");
    };
    assert_eq!(agent.state, AgentState::Idle);
    assert_eq!(agent.source, AgentSource::Reported);
    assert_eq!(hook_state(&start("compact")), None);
    assert_eq!(hook_state(&start("unknown")), None);
    assert_eq!(hook_state(&event("SessionStart")), None);
    assert_eq!(hook_state(&event("Stop")), Some(AgentState::Idle));
    assert_eq!(hook_state(&event("Interrupt")), Some(AgentState::Idle));
    assert_eq!(
        hook_state(&event("UserPromptSubmit")),
        Some(AgentState::Busy)
    );
    assert_eq!(hook_state(&event("PreToolUse")), Some(AgentState::Busy));
    assert_eq!(hook_state(&event("PostToolUse")), Some(AgentState::Busy));
    assert_eq!(
        hook_state(&event("PermissionRequest")),
        Some(AgentState::Waiting)
    );
    assert_eq!(hook_state(&notification("permission_prompt")), None);
    assert_eq!(
        hook_state(&notification("agent_needs_input")),
        Some(AgentState::Waiting)
    );
    assert_eq!(hook_state(&notification("idle_prompt")), None);
    assert_eq!(hook_state(&notification("quota_auto_resume_fired")), None);
    assert_eq!(hook_state(&event(HOOK_SESSION_END)), None);
    assert_eq!(hook_state(&event("SomethingNewInAYearsTime")), None);

    // The nine events the table grew by. A turn that died on an API error ended; a
    // subagent, a compaction and a refused tool call are all the agent working; an MCP
    // form is a person being asked.
    assert_eq!(hook_state(&event("StopFailure")), Some(AgentState::Idle));
    for busy in [
        "PostToolUseFailure",
        "PermissionDenied",
        "ElicitationResult",
        "PreCompact",
        "PostCompact",
    ] {
        assert_eq!(hook_state(&event(busy)), Some(AgentState::Busy), "{busy}");
    }
    assert_eq!(hook_state(&event("Elicitation")), Some(AgentState::Waiting));
    // The one notification that reads like a turn ending and is not: Claude Code raises
    // it for a *background* job this session started, from the same payload the
    // foreground would use. A pane whose agent launched one and worked on would be
    // marked idle mid-turn by it. `Stop` is the turn ending, and says so earlier.
    assert_eq!(hook_state(&notification("agent_completed")), None);

    // The two tools that *are* the question. Every other tool starting is work.
    let tool = |name: &str, tool: &str| HookEvent {
        tool_name: Some(tool.to_owned()),
        ..event(name)
    };
    for asking in ["AskUserQuestion", "ExitPlanMode"] {
        assert_eq!(
            hook_state(&tool("PreToolUse", asking)),
            Some(AgentState::Waiting),
            "{asking}"
        );
        assert_eq!(
            hook_state(&tool("PostToolUse", asking)),
            Some(AgentState::Busy),
            "{asking} finishing is work again"
        );
    }
    assert_eq!(
        hook_state(&tool("PreToolUse", "Bash")),
        Some(AgentState::Busy)
    );

    // Display events carry prose without changing state. SessionEnd ends the record.
    for wired in CLAUDE_HOOK_EVENTS {
        if matches!(
            *wired,
            HOOK_SESSION_END | "Notification" | "MessageDisplay" | "SubagentStart" | "SubagentStop"
        ) {
            continue;
        }
        let said = match *wired {
            "SessionStart" => hook_state(&start("startup")),
            "PreToolUse" | "PostToolUse" => hook_state(&tool(wired, "Bash")),
            _ => hook_state(&event(wired)),
        };
        assert!(said.is_some(), "{wired} is wired and says nothing");
    }
}

/// Other providers use the permission mode to distinguish automatic approval.
/// Direct questions and elicitation still need a person under every mode.
#[test]
fn a_wait_the_mode_answers_needs_nobody() {
    let request = |tool: Option<&str>, mode: Option<&str>| HookEvent {
        permission_mode: mode.map(str::to_owned),
        tool_name: tool.map(str::to_owned),
        ..event("PermissionRequest")
    };
    assert!(waiting_needs_a_person(&request(Some("Bash"), None)));
    assert!(waiting_needs_a_person(&request(
        Some("Bash"),
        Some("default")
    )));
    assert!(waiting_needs_a_person(&request(Some("Bash"), Some("plan"))));
    for unattended in UNATTENDED_MODES {
        assert!(
            !waiting_needs_a_person(&request(Some("Bash"), Some(unattended))),
            "{unattended} answers every prompt"
        );
    }

    // `acceptEdits` accepts *edits*. A `Bash` prompt under it blocks exactly as it would
    // under `default`, and the mode alone cannot tell the two apart.
    for tool in AUTO_ACCEPTED_EDIT_TOOLS {
        assert!(
            !waiting_needs_a_person(&request(Some(tool), Some("acceptEdits"))),
            "{tool} is accepted without anybody"
        );
    }
    for tool in ["Bash", "WebFetch", "mcp__github__create_pull_request"] {
        assert!(
            waiting_needs_a_person(&request(Some(tool), Some("acceptEdits"))),
            "{tool} still stops and waits"
        );
    }
    // No tool named is treated as attended: a toast too many costs a line, and one too
    // few costs a blocked agent nobody hears about.
    assert!(waiting_needs_a_person(&request(None, Some("acceptEdits"))));

    // Direct questions and elicitation notifications are always waits. None is a
    // permission prompt, so no mode answers any of them.
    for mode in ["acceptEdits", "bypassPermissions", "dontAsk", "auto"] {
        for tool in WAITING_TOOLS {
            let asking = HookEvent {
                tool_name: Some((*tool).to_owned()),
                permission_mode: Some(mode.to_owned()),
                ..event("PreToolUse")
            };
            assert!(
                waiting_needs_a_person(&asking),
                "{tool} under {mode} is still a question"
            );
        }
        let elicitation = HookEvent {
            permission_mode: Some(mode.to_owned()),
            ..event("Elicitation")
        };
        assert!(waiting_needs_a_person(&elicitation), "{mode}");
        for kind in WAITING_NOTIFICATIONS {
            let notification = HookEvent {
                permission_mode: Some(mode.to_owned()),
                ..notification(kind)
            };
            assert_eq!(hook_state(&notification), Some(AgentState::Waiting));
        }
        for kind in ["permission_prompt", "worker_permission_prompt"] {
            assert_eq!(hook_state(&notification(kind)), None);
        }
    }
}

#[test]
fn claude_permission_evaluation_stays_busy_until_a_prompt_notification() {
    for mode in [
        None,
        Some("plan"),
        Some("default"),
        Some("acceptEdits"),
        Some("auto"),
        Some("dontAsk"),
        Some("bypassPermissions"),
    ] {
        let request = HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(json!({"command": "git status"})),
            permission_mode: mode.map(str::to_owned),
            ..event("PermissionRequest")
        };
        let HookOutcome::Reported(info) = hook_outcome("claude", &request) else {
            panic!("permission evaluation reports work");
        };
        assert_eq!(info.state, AgentState::Busy);
        assert_eq!(info.detail.as_deref(), Some("Bash git"));
        assert_eq!(info.waiting_on, None);
        assert!(!info.wait_needs_a_person);
        let raw = serde_json::to_string(&request).unwrap();
        let broadcast = AgentEvent::from_hook(
            "claude",
            crate::internal_ids::TerminalId::new(7),
            &request,
            raw.clone(),
        );
        assert_eq!(broadcast.state, Some(info.state));
        assert_eq!(broadcast.detail, info.detail);
        assert_eq!(broadcast.raw, raw);

        for tool in WAITING_TOOLS {
            let asking = HookEvent {
                tool_name: Some((*tool).to_owned()),
                ..request.clone()
            };
            assert_eq!(
                hook_state_for(
                    "claude",
                    &HookEvent {
                        hook_event_name: "PreToolUse".to_owned(),
                        ..asking.clone()
                    }
                ),
                Some(AgentState::Waiting)
            );
            assert_eq!(hook_outcome("claude", &asking), HookOutcome::Nothing);
            assert_eq!(
                hook_state_for(
                    "claude",
                    &HookEvent {
                        hook_event_name: "PostToolUse".to_owned(),
                        ..asking
                    }
                ),
                Some(AgentState::Busy)
            );
        }
        for kind in ["permission_prompt", "worker_permission_prompt"] {
            let prompt = HookEvent {
                permission_mode: mode.map(str::to_owned),
                message: Some("Allow Bash?".to_owned()),
                ..notification(kind)
            };
            for worker in [None, Some("worker")] {
                let prompt = HookEvent {
                    agent_id: worker.map(str::to_owned),
                    ..prompt.clone()
                };
                let expected = (kind == "permission_prompt").then_some(AgentState::Waiting);
                if worker.is_none() && expected.is_some() {
                    let HookOutcome::Reported(info) = hook_outcome("claude", &prompt) else {
                        panic!("a prompt notification reports waiting");
                    };
                    assert_eq!(info.state, AgentState::Waiting);
                    assert_eq!(info.waiting_on.as_deref(), Some("Allow Bash?"));
                    assert!(info.wait_needs_a_person);
                } else {
                    assert_eq!(hook_outcome("claude", &prompt), HookOutcome::Nothing);
                }
                assert_eq!(hook_state_for("claude", &prompt), expected);
                for provider in ["codex", "muse", "opencode"] {
                    assert_eq!(hook_state_for(provider, &prompt), None);
                }
                let broadcast = AgentEvent::from_hook(
                    "claude",
                    crate::internal_ids::TerminalId::new(7),
                    &prompt,
                    serde_json::to_string(&prompt).unwrap(),
                );
                assert_eq!(broadcast.state, expected);
            }
        }
    }
}

/// What a record says the agent is blocked on. Every source is text the agent or an MCP
/// server wrote, so all of it is cleaned and cut.
#[test]
fn a_wait_names_what_it_is_waiting_for() {
    let ask = HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(serde_json::json!({
            "questions": [{ "question": "Which library\nfor dates?", "header": "Library" }]
        })),
        ..event("PreToolUse")
    };
    assert_eq!(
        waiting_detail(&ask).as_deref(),
        Some("Which library for dates?"),
        "a line, not a word: the newline collapses to a space"
    );
    let headerless = HookEvent {
        tool_input: Some(serde_json::json!({ "questions": [{ "header": "Library" }] })),
        ..ask.clone()
    };
    assert_eq!(waiting_detail(&headerless).as_deref(), Some("Library"));
    let shapeless = HookEvent {
        tool_input: Some(serde_json::json!({ "nothing": "we know" })),
        ..ask
    };
    assert_eq!(
        waiting_detail(&shapeless).as_deref(),
        Some("a question"),
        "a reshaped tool_input costs the text, not the fact that a question was asked"
    );

    let plan = HookEvent {
        tool_name: Some("ExitPlanMode".to_owned()),
        ..event("PreToolUse")
    };
    assert_eq!(waiting_detail(&plan).as_deref(), Some("approve the plan"));

    let permission = HookEvent {
        tool_name: Some("Bash".to_owned()),
        tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
        ..event("PermissionRequest")
    };
    assert_eq!(waiting_detail(&permission).as_deref(), Some("Bash rm"));

    let elicitation = HookEvent {
        message: Some("Which account should I use?".to_owned()),
        ..event("Elicitation")
    };
    assert_eq!(
        waiting_detail(&elicitation).as_deref(),
        Some("Which account should I use?")
    );

    // Nothing that is not a wait names one.
    assert_eq!(waiting_detail(&event("PostToolUse")), None);
    assert_eq!(waiting_detail(&event("Stop")), None);

    // A long question is cut to what a toast holds, and the cut is marked.
    let long = HookEvent {
        message: Some("q ".repeat(200)),
        ..event("Notification")
    };
    let line = waiting_detail(&long).expect("a line");
    assert_eq!(line.chars().count(), WAITING_DETAIL_CHARS);
    assert!(line.ends_with('…'));
}

#[test]
fn a_reported_record_is_complete_and_an_end_is_an_end() {
    let pre = HookEvent {
        tool_name: Some("Bash".to_owned()),
        tool_input: Some(serde_json::json!({ "command": "cargo build --release" })),
        ..event("PreToolUse")
    };
    assert_eq!(
        hook_outcome("claude", &pre),
        HookOutcome::Reported(AgentInfo {
            kind: "claude".to_owned(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: Some("Bash cargo".to_owned()),
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: Some("/work".to_owned()),
            session_id: Some("s1".to_owned()),
            run_id: None,
        })
    );
    // Permission evaluation keeps the tool hint without announcing a user wait.
    let permission = HookEvent {
        tool_name: Some("Bash".to_owned()),
        tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
        ..event("PermissionRequest")
    };
    match hook_outcome("claude", &permission) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.state, AgentState::Busy);
            assert_eq!(info.detail.as_deref(), Some("Bash rm"));
            assert_eq!(info.waiting_on, None);
        },
        other => panic!("{other:?}"),
    }
    assert_eq!(
        hook_outcome("claude", &event(HOOK_SESSION_END)),
        HookOutcome::Ended
    );
    assert_eq!(
        hook_outcome("claude", &notification("auth_success")),
        HookOutcome::Nothing
    );
    match hook_outcome("claude", &event("PostToolUse")) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.detail, None, "a finished tool clears the hint")
        },
        other => panic!("{other:?}"),
    }

    // Codex's question, which is a tool call and not an event of its own.
    let question = HookEvent {
        tool_name: Some("request_user_input".to_owned()),
        tool_input: Some(serde_json::json!({
            "questions": [{"question": "Which history should we keep?"}]
        })),
        ..event("PreToolUse")
    };
    match hook_outcome("codex", &question) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.kind, "codex");
            assert_eq!(info.state, AgentState::Waiting);
            assert_eq!(
                info.waiting_on.as_deref(),
                Some("Which history should we keep?")
            );
        },
        other => panic!("{other:?}"),
    }
    // And read the same way whoever asks, because every road that reads an event reads
    // this one function: a kind-keyed answer would reach the record and not the feed.
    assert_eq!(hook_state(&question), Some(AgentState::Waiting));
}

/// A Muse session's feed, as the host parses it: startup is idle, a submitted turn
/// is busy, a plain `Stop` ends it (Muse has no blocking-hook continuation to wait
/// out), and the translated approval notification waits on a person. The payloads
/// are the shapes captured from `muse exec --provider echo`, with the ids blanked.
#[test]
fn a_muse_feed_reads_as_the_lifecycle_it_is() {
    let start = HookEvent {
        source: Some("startup".to_owned()),
        ..event("SessionStart")
    };
    match hook_outcome("muse", &start) {
        HookOutcome::Reported(info) => assert_eq!(info.state, AgentState::Idle),
        other => panic!("{other:?}"),
    }
    match hook_outcome("muse", &event("UserPromptSubmit")) {
        HookOutcome::Reported(info) => assert_eq!(info.state, AgentState::Busy),
        other => panic!("{other:?}"),
    }
    // A snake_case tool still names the row; only its argument hint is lost.
    let pre = HookEvent {
        tool_name: Some("read_skill".to_owned()),
        ..event("PreToolUse")
    };
    match hook_outcome("muse", &pre) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.state, AgentState::Busy);
            assert_eq!(info.detail.as_deref(), Some("read_skill"));
        },
        other => panic!("{other:?}"),
    }
    match hook_outcome("muse", &event("Stop")) {
        HookOutcome::Reported(info) => assert_eq!(info.state, AgentState::Idle),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        hook_outcome("muse", &event(HOOK_SESSION_END)),
        HookOutcome::Ended
    );
    // The translated approval notification: a wait, on a person, carrying the
    // sentence Muse would have shown.
    let approval = HookEvent {
        message: Some("Allow `cargo build`?".to_owned()),
        ..event("Elicitation")
    };
    match hook_outcome("muse", &approval) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.state, AgentState::Waiting);
            assert!(info.wait_needs_a_person);
            assert_eq!(info.waiting_on.as_deref(), Some("Allow `cargo build`?"));
        },
        other => panic!("{other:?}"),
    }
}

/// The record carries [`waiting_needs_a_person`]'s answer, because the pane that draws the
/// waiting bell is a long way from the event that decided it - and the event is gone by
/// the time the pane sees anything. Only a wait carries one, for the same reason only a
/// wait carries `waiting_on`: a record that has moved on has no wait for anybody to
/// answer.
#[test]
fn a_record_says_whether_its_wait_wants_a_person() {
    let request = |tool: &str, mode: &str| HookEvent {
        permission_mode: Some(mode.to_owned()),
        tool_name: Some(tool.to_owned()),
        ..event("PermissionRequest")
    };
    let reported = |event: &HookEvent| match hook_outcome("claude", event) {
        HookOutcome::Reported(info) => info,
        other => panic!("{other:?}"),
    };

    // Automatic approval is busy on every surface.
    let accepted = reported(&request("Edit", "acceptEdits"));
    assert_eq!(accepted.state, AgentState::Busy);
    assert_eq!(accepted.waiting_on, None);
    assert!(!accepted.wait_needs_a_person);

    for kind in ["claude", "codex", "muse"] {
        for mode in [None, Some("default"), Some("plan"), Some("unknown-mode")] {
            let event = HookEvent {
                permission_mode: mode.map(str::to_owned),
                ..request("Bash", "default")
            };
            let HookOutcome::Reported(info) = hook_outcome(kind, &event) else {
                panic!("manual permission must report");
            };
            let expected = if kind == "claude" {
                AgentState::Busy
            } else {
                AgentState::Waiting
            };
            assert_eq!(info.state, expected);
            assert_eq!(info.wait_needs_a_person, kind != "claude");
        }
    }

    // Claude still evaluates Bash permissions under acceptEdits.
    let blocked = reported(&request("Bash", "acceptEdits"));
    assert_eq!(blocked.state, AgentState::Busy);
    assert!(!blocked.wait_needs_a_person);

    // A question addressed to a person is one under every mode there is.
    for mode in ["acceptEdits", "bypassPermissions", "dontAsk", "auto"] {
        let asking = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            permission_mode: Some(mode.to_owned()),
            ..event("PreToolUse")
        };
        assert!(reported(&asking).wait_needs_a_person, "{mode}");
    }

    // And nothing that is not a wait carries one, whatever the mode around it.
    for name in ["PreToolUse", "PostToolUse", "Stop"] {
        let busy = HookEvent {
            permission_mode: Some("default".to_owned()),
            ..event(name)
        };
        let info = reported(&busy);
        assert_ne!(info.state, AgentState::Waiting);
        assert!(!info.wait_needs_a_person, "{name}");
    }
}

/// A record that says `idle` and carries a question is a record that disagrees with
/// itself, and every reader of it — the row, the toast, `agent_list` — reads both halves.
/// The only source of a wait's text is [`waiting_detail`], which will answer for an
/// `Elicitation` or a `Notification` whatever the state around it turned out to be.
#[test]
fn only_a_wait_carries_what_it_is_waiting_on() {
    let elicitation = HookEvent {
        message: Some("Which account?".to_owned()),
        ..event("Elicitation")
    };
    match hook_outcome("claude", &elicitation) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.state, AgentState::Waiting);
            assert_eq!(info.waiting_on.as_deref(), Some("Which account?"));
        },
        other => panic!("{other:?}"),
    }
    // The same text on an event that is not a wait. `ElicitationResult` is the answer
    // coming back, which is the agent working again.
    let answered = HookEvent {
        message: Some("Which account?".to_owned()),
        ..event("ElicitationResult")
    };
    match hook_outcome("claude", &answered) {
        HookOutcome::Reported(info) => {
            assert_eq!(info.state, AgentState::Busy);
            assert_eq!(info.waiting_on, None);
        },
        other => panic!("{other:?}"),
    }
}

/// The broadcast is copied into every running plugin's memory, twice per tool call,
/// inside a hook with five seconds to live — so a `Write` of a large file must not
/// travel whole. What a reader takes from `tool_input` is short and must survive.
#[test]
fn a_pruned_event_keeps_what_is_read_and_drops_the_rest() {
    let huge = "x".repeat(2 * 1024 * 1024);
    let write = HookEvent {
        tool_name: Some("Write".to_owned()),
        tool_input: Some(serde_json::json!({
            // Deliberately first, the way Claude Code's own schema orders it: cutting
            // every string rather than stopping at a byte count is what keeps the read
            // key alive behind the enormous one.
            "content": huge,
            "file_path": "/work/src/main.rs",
        })),
        last_assistant_message: Some(huge.clone()),
        prompt: Some(huge.clone()),
        ..event("PreToolUse")
    };

    let (pruned, _) = write.broadcast(serde_json::to_string(&write).unwrap());
    let json = serde_json::to_string(pruned.as_ref()).expect("a pruned event serializes");
    assert!(
        json.len() <= HOOK_BROADCAST_MAX_BYTES,
        "a pruned event fits the cap, got {}",
        json.len()
    );
    // Every reading still works off the pruned event, which is the whole point of
    // cutting the strings rather than dropping the fields.
    assert_eq!(hook_detail(&pruned).as_deref(), Some("Write main.rs"));
    assert_eq!(hook_state(&pruned), hook_state(&write));
    assert!(pruned
        .last_assistant_message
        .as_ref()
        .is_some_and(|message| message.len() > 8 * 1024));

    // The other reader of `tool_input`, whose question is nested two levels down.
    let asking = HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(serde_json::json!({
            "questions": [{ "question": "Which library?", "header": "Library" }],
            "context": huge,
        })),
        ..event("PreToolUse")
    };
    assert_eq!(
        waiting_detail(&asking.pruned()).as_deref(),
        Some("Which library?")
    );

    // An event already inside the cap travels unchanged, which is nearly all of them —
    // `hook_report_actions` does not even call this for one.
    let small = HookEvent {
        tool_name: Some("Bash".to_owned()),
        tool_input: Some(serde_json::json!({ "command": "cargo test" })),
        ..event("PreToolUse")
    };
    assert_eq!(small.pruned(), small);
}

/// The prune is a cap in bytes, whatever alphabet the agent was speaking.
///
/// Every string cap here is in characters, and a character is up to four bytes. A plan or
/// a set of questions written in Japanese, beside three prose fields at their own caps,
/// is what a single prune could not hold: it went out over the cap the prune exists to
/// enforce, into every plugin's memory, which is the whole cost the cap is about.
#[test]
fn a_broadcast_is_bounded_whatever_alphabet_it_is_in() {
    // Three bytes a character, and every one of them under a key a history reads at
    // length or in a prose field with the largest cap.
    let long = "計".repeat(PRUNED_HISTORY_STRING_CHARS);
    let questions: Vec<serde_json::Value> = (0..8)
        .map(|_| serde_json::json!({ "question": long.clone(), "header": long.clone() }))
        .collect();
    let asking = HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(serde_json::json!({ "questions": questions })),
        last_assistant_message: Some(long.clone()),
        message: Some(long.clone()),
        prompt: Some(long.clone()),
        ..event("PreToolUse")
    };

    let json = serde_json::to_string(&asking).expect("the event serializes");
    assert!(json.len() > HOOK_BROADCAST_MAX_BYTES);
    let payload = asking.broadcast_json(json);
    assert!(
        payload.len() <= HOOK_BROADCAST_MAX_BYTES,
        "the broadcast is bounded, got {}",
        payload.len()
    );
    // Still an event, and still the same event: what a reader takes from it is a name and
    // a first question, both of which fit whatever the budget came down to.
    let arrived: HookEvent =
        serde_json::from_str(&payload).expect("a pruned event is still an event");
    assert_eq!(hook_state(&arrived), hook_state(&asking));
    assert_eq!(arrived.tool_name.as_deref(), Some("AskUserQuestion"));
    assert!(waiting_detail(&arrived).is_some());
}

/// The typed `prompt` and `last_message` are bounded the way `raw` is, and say what it
/// says: a multi-megabyte final message must not reach every plugin's memory through the
/// typed field after the prune kept it out of the JSON.
#[test]
fn a_typed_events_prose_is_bounded_like_its_raw_json() {
    let huge = "x".repeat(2 * 1024 * 1024);
    let stop = HookEvent {
        last_assistant_message: Some(huge.clone()),
        prompt: Some(huge),
        ..event("Stop")
    };
    let json = serde_json::to_string(&stop).expect("the event serializes");
    let typed = AgentEvent::from_hook(
        "claude",
        crate::internal_ids::TerminalId::new(7),
        &stop,
        json,
    );
    assert!(typed.raw.len() <= HOOK_BROADCAST_MAX_BYTES);
    let arrived: HookEvent = serde_json::from_str(&typed.raw).expect("still an event");
    assert_eq!(typed.last_message, arrived.last_assistant_message);
    assert_eq!(typed.prompt, arrived.prompt);
    assert!(typed
        .last_message
        .as_ref()
        .is_some_and(|message| message.len() > 8 * 1024));
    assert!(typed
        .prompt
        .as_ref()
        .is_some_and(|prompt| prompt.len() > 8 * 1024));

    // Under the cap, whole - nearly every event.
    let small = HookEvent {
        last_assistant_message: Some("Ran the tests; they pass.".to_owned()),
        ..event("Stop")
    };
    let json = serde_json::to_string(&small).expect("the event serializes");
    let typed = AgentEvent::from_hook(
        "claude",
        crate::internal_ids::TerminalId::new(7),
        &small,
        json.clone(),
    );
    assert_eq!(typed.raw, json);
    assert_eq!(typed.last_message, small.last_assistant_message);
    assert_eq!(typed.prompt, None);
}

/// An event that already fits travels exactly as it arrived.
#[test]
fn a_broadcast_within_the_cap_is_untouched() {
    let event = HookEvent {
        tool_name: Some("Read".to_owned()),
        tool_input: Some(serde_json::json!({ "file_path": "/work/src/main.rs" })),
        ..event("PreToolUse")
    };
    let json = serde_json::to_string(&event).expect("the event serializes");
    assert_eq!(event.broadcast_json(json.clone()), json);
}

#[test]
fn hook_json_boundaries_and_full_prompt_identity() {
    for size in [
        HOOK_BROADCAST_MAX_BYTES - 1,
        HOOK_BROADCAST_MAX_BYTES,
        HOOK_BROADCAST_MAX_BYTES + 1,
    ] {
        let mut value =
            json!({"hook_event_name":"UserPromptSubmit", "prompt":"", "turn_id":"turn-1"});
        let overhead = value.to_string().len();
        value["prompt"] = Value::String("x".repeat(size - overhead));
        let raw = value.to_string();
        assert_eq!(raw.len(), size);
        let hook: HookEvent = serde_json::from_str(&raw).unwrap();
        let (_, bounded) = hook.broadcast(raw.clone());
        assert!(bounded.len() <= HOOK_BROADCAST_MAX_BYTES);
        if size <= HOOK_BROADCAST_MAX_BYTES {
            assert_eq!(bounded, raw);
        }
    }
    let metadata = HookEvent {
        session_id: "\u{0001}".repeat(HOOK_BROADCAST_MAX_BYTES),
        ..event("Stop")
    };
    assert!(
        metadata
            .broadcast_json(serde_json::to_string(&metadata).unwrap())
            .len()
            <= HOOK_BROADCAST_MAX_BYTES
    );
    for text in ["x", "計🙂", "\\\"\n\u{0001}"] {
        let prompt = text.repeat(HOOK_BROADCAST_MAX_BYTES);
        let hook = HookEvent {
            prompt: Some(prompt.clone()),
            turn_id: Some("turn-1".into()),
            prompt_id: Some("prompt-1".into()),
            claude_turn_complete: Some(true),
            claude_background_tasks: (0..40).map(|task| format!("task-{task}")).collect(),
            agy_turn_complete: Some(true),
            message: Some(prompt.clone()),
            last_assistant_message: Some(prompt.clone()),
            tool_input: Some(json!({"plan":prompt, "content":"x".repeat(300000)})),
            ..event("UserPromptSubmit")
        };
        let (bounded, raw) = hook.broadcast(serde_json::to_string(&hook).unwrap());
        assert!(raw.len() <= HOOK_BROADCAST_MAX_BYTES);
        assert!(bounded.prompt.as_ref().unwrap().len() > 8192);
        assert_eq!(bounded.turn_id, hook.turn_id);
        assert_eq!(bounded.prompt_id, hook.prompt_id);
        assert_eq!(bounded.claude_turn_complete, hook.claude_turn_complete);
        assert_eq!(
            bounded.claude_background_tasks,
            hook.claude_background_tasks[..MAX_BACKGROUND_TASKS]
        );
        assert_eq!(bounded.agy_turn_complete, hook.agy_turn_complete);
        assert_eq!(
            bounded.prompt_sha256,
            Some(crate::agent::control::prompt_sha256(&prompt))
        );
        let arrived: HookEvent = serde_json::from_str(&raw).unwrap();
        assert_eq!(&arrived, bounded.as_ref());
        // A remote broker can prune before forwarding to the host.
        // A second prune must keep the digest of the original, complete prompt.
        let mut forwarded: Value = serde_json::from_str(&raw).unwrap();
        forwarded["message"] = Value::String("message".repeat(1000));
        let forwarded_hook: HookEvent = serde_json::from_value(forwarded.clone()).unwrap();
        let (repruned, forwarded_raw) = forwarded_hook.broadcast(forwarded.to_string());
        assert!(forwarded_raw.len() <= HOOK_BROADCAST_MAX_BYTES);
        assert_eq!(repruned.prompt_sha256, bounded.prompt_sha256);
        assert_eq!(repruned.turn_id, bounded.turn_id);
        let budget = bounded.prompt.as_ref().unwrap().len();
        assert!(
            serde_json::to_string(
                &HookEvent {
                    prompt_sha256: bounded.prompt_sha256.clone(),
                    ..hook
                }
                .pruned_to(budget + 4)
            )
            .unwrap()
            .len()
                > HOOK_BROADCAST_MAX_BYTES
        );
    }
}

/// A cut would drop a collapsed paste's closer and leave its opener in the prompt.
#[test]
fn an_oversize_collapsed_paste_is_unwrapped_before_it_is_cut() {
    let body = "x\n".repeat(HOOK_BROADCAST_MAX_BYTES);
    let prompt = format!(
        "{} \n\n<pasted_content id=\"8c9e\">\n{body}\n</pasted_content id=\"8c9e\">\n",
        crate::agent::control::PASTE_LEAD_IN
    );
    let hook = HookEvent {
        prompt: Some(prompt.clone()),
        ..event("UserPromptSubmit")
    };
    let (bounded, raw) = hook.broadcast(serde_json::to_string(&hook).unwrap());
    assert!(raw.len() <= HOOK_BROADCAST_MAX_BYTES);
    let kept = bounded.prompt.as_deref().unwrap();
    assert!(!kept.contains("pasted_content"));
    let submitted = crate::agent::control::submitted_prompt(kept);
    assert!(submitted.len() > 8192);
    assert!(body.starts_with(submitted.as_ref()));
    assert_eq!(
        bounded.prompt_sha256,
        Some(crate::agent::control::prompt_sha256(&prompt))
    );
}

/// A submitted plan survives a prune, where a passing argument does not.
///
/// The regression the key-aware cut exists for: a plan is `tool_input.plan`, and cutting
/// every string in `tool_input` to a record's budget turned a long plan into 256
/// characters on its way to the run history. The prune only runs above
/// [`HOOK_BROADCAST_MAX_BYTES`], which a big plan and a big anything-else together reach
/// easily.
#[test]
fn a_prune_keeps_a_plan_at_length_and_still_cuts_an_argument() {
    let plan = "step. ".repeat(600);
    let event = HookEvent {
        tool_name: Some("ExitPlanMode".to_owned()),
        tool_input: Some(serde_json::json!({
            "plan": plan,
            "context": "y".repeat(2 * 1024 * 1024),
        })),
        ..event("PreToolUse")
    };
    let pruned = event.pruned();
    let input = pruned.tool_input.expect("the input survives");
    assert_eq!(
        input.get("plan").and_then(|plan| plan.as_str()),
        Some(plan.as_str()),
        "a plan of this length is kept whole"
    );
    assert_eq!(
        input
            .get("context")
            .and_then(|value| value.as_str())
            .map(|value| value.chars().count()),
        Some(PRUNED_STRING_CHARS),
        "an argument nobody reads at length is still cut"
    );
}

/// Every question, its header and its options, for the reader that has room for them.
#[test]
fn a_question_reaches_the_history_whole_while_the_toast_keeps_its_line() {
    let event = HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(serde_json::json!({
            "questions": [
                {
                    "header": "Library",
                    "question": "Which library should we use for parsing the manifest?",
                    "options": [
                        { "label": "serde_json", "description": "what everything else uses" },
                        { "label": "simd-json" },
                    ],
                },
                { "question": "Keep the old runs?", "options": ["yes", "no"] },
            ],
        })),
        ..event("PreToolUse")
    };

    let text = question_text(&event).expect("a question");
    assert_eq!(
        text,
        "Library\nWhich library should we use for parsing the manifest?\n- serde_json\n- \
             simd-json\n\nKeep the old runs?\n- yes\n- no"
    );

    // The toast and the sidebar are unchanged: one line, and only the first question.
    let line = waiting_detail(&event).expect("a wait");
    assert_eq!(
        line,
        "Which library should we use for parsing the manifest?"
    );
    assert!(line.chars().count() <= WAITING_DETAIL_CHARS);
}

/// A shape this does not recognise costs the question's text, not the question.
#[test]
fn an_unreadable_question_falls_back_to_the_one_line_detail() {
    let reshaped = HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(serde_json::json!({ "queries": ["which one?"] })),
        ..event("PreToolUse")
    };
    assert_eq!(question_text(&reshaped).as_deref(), Some("a question"));

    let empty = HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(serde_json::json!({ "questions": [{ "options": [] }] })),
        ..event("PreToolUse")
    };
    assert_eq!(question_text(&empty).as_deref(), Some("a question"));
}

/// The budget behind the per-string cut, for the shape the cut alone cannot bound: an
/// object with more keys than a tool call has any business carrying. What it promises is
/// a size, not any particular key — the keys a reader wants are covered by the cut above.
#[test]
fn a_prune_bounds_an_object_no_string_cut_could() {
    let mut input = serde_json::Map::new();
    for i in 0..8192 {
        input.insert(format!("k{i:04}"), serde_json::json!("x".repeat(512)));
    }
    let event = HookEvent {
        tool_name: Some("mcp__something__enormous".to_owned()),
        tool_input: Some(serde_json::Value::Object(input)),
        ..event("PreToolUse")
    };
    let json = event.broadcast_json(serde_json::to_string(&event).expect("event serializes"));
    assert!(json.len() <= HOOK_BROADCAST_MAX_BYTES, "{}", json.len());
}

#[test]
fn tool_call_identity_accepts_both_provider_spellings_and_survives_pruning() {
    let claude: HookEvent =
        serde_json::from_str(r#"{"hook_event_name":"PreToolUse","tool_use_id":"tool-1"}"#)
            .expect("Claude's spelling parses");
    let alias: HookEvent =
        serde_json::from_str(r#"{"hook_event_name":"PreToolUse","tool_call_id":"tool-2"}"#)
            .expect("the alternate spelling parses");

    assert_eq!(claude.tool_use_id.as_deref(), Some("tool-1"));
    assert_eq!(alias.pruned().tool_use_id.as_deref(), Some("tool-2"));
}

#[test]
fn a_hint_is_one_printable_word_cut_to_size() {
    assert_eq!(clean_hint("  git\tstatus "), Some("gitstatus".to_owned()));
    assert_eq!(clean_hint("\u{7}"), None);
    let long = "a".repeat(HOOK_DETAIL_CHARS + 5);
    let hint = clean_hint(&long).unwrap();
    assert_eq!(hint.chars().count(), HOOK_DETAIL_CHARS);
    assert!(hint.ends_with('…'));
}

/// A `Stop` as Claude Code actually writes it, with every field this module reads.
///
/// Pinned against the shipped schema rather than the docs: the field that matters most
/// here, `last_assistant_message`, is optional in Claude Code's own definition, and a
/// rename would otherwise cost a summary silently.
#[test]
fn a_stop_event_carries_the_agents_last_words() {
    let event: HookEvent = serde_json::from_str(
        r#"{
                "session_id": "abc",
                "transcript_path": "/tmp/t.jsonl",
                "cwd": "/work",
                "permission_mode": "default",
                "hook_event_name": "Stop",
                "stop_hook_active": false,
                "last_assistant_message": "Ran the tests; they pass."
            }"#,
    )
    .expect("parsed");
    assert_eq!(event.hook_event_name, "Stop");
    assert_eq!(event.session_id, "abc");
    assert_eq!(event.cwd.as_deref(), Some("/work"));
    assert_eq!(event.permission_mode.as_deref(), Some("default"));
    assert_eq!(
        event.last_assistant_message.as_deref(),
        Some("Ran the tests; they pass."),
    );
}

/// Everything but the event name is optional, and unknown fields are ignored: a Claude
/// Code that adds one, or drops one we do not read, must not cost us the event.
#[test]
fn a_hook_event_parses_with_nothing_but_its_name() {
    let event: HookEvent = serde_json::from_str(
        r#"{"hook_event_name":"Stop","something_new_in_a_years_time":{"a":1}}"#,
    )
    .expect("parsed");
    assert_eq!(event.hook_event_name, "Stop");
    assert!(event.session_id.is_empty());
    assert_eq!(event.last_assistant_message, None);
    assert_eq!(event.message, None);
    assert_eq!(event.prompt, None);
    assert_eq!(event.agent_id, None);
}

/// The two events that name what the user is being asked, each in its own field.
#[test]
fn a_notification_and_a_prompt_submission_each_carry_their_text() {
    let event: HookEvent = serde_json::from_str(
        r#"{"hook_event_name":"Notification","notification_type":"permission_prompt",
                "message":"Claude needs your permission to use Bash","title":"Claude Code"}"#,
    )
    .expect("parsed");
    assert_eq!(
        event.notification_type.as_deref(),
        Some("permission_prompt")
    );
    assert_eq!(
        event.message.as_deref(),
        Some("Claude needs your permission to use Bash"),
    );

    let event: HookEvent = serde_json::from_str(
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix the flaky test","source":"user"}"#,
    )
    .expect("parsed");
    assert_eq!(event.prompt.as_deref(), Some("fix the flaky test"));
}
