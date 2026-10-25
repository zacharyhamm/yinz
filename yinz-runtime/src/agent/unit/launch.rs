use super::*;
use std::path::PathBuf;

fn spec(kind: &str) -> Spec {
    if kind == "opencode" {
        crate::agent::provider_fixture::register_opencode();
    }
    Spec {
        kind: Some(kind.to_owned()),
        ..Default::default()
    }
}

#[test]
fn effort_is_translated_or_rejected() {
    for (kind, expected) in [
        (
            "codex",
            vec![
                "agent",
                "codex-untracked",
                "--command",
                "codex",
                "--",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--model",
                "example",
            ],
        ),
        (
            "claude",
            vec![
                "YINZ_CLAUDE_INLINE_HOOKS=0",
                "yinz",
                "agent",
                "claude",
                "--command",
                "claude",
                "--",
                "--effort",
                "high",
                "--model",
                "example",
            ],
        ),
        ("agy", vec!["--effort", "high", "--model", "example"]),
        ("cursor", vec!["--model", "example[effort=high]"]),
        (
            "muse",
            vec!["--reasoning-effort", "high", "--model", "example"],
        ),
    ] {
        let (command, _) = plan(&Spec {
            model: Some("example".into()),
            effort: Some("high".into()),
            hooks: Some(false),
            ..spec(kind)
        })
        .unwrap();
        assert_eq!(command.args, expected);
    }
    for kind in ["opencode", "unknown"] {
        assert!(plan(&Spec {
            effort: Some("high".into()),
            ..spec(kind)
        })
        .is_err());
    }
    assert!(plan(&Spec {
        effort: Some("invalid".into()),
        ..spec("codex")
    })
    .is_err());
}

/// The modal's own defaults, which are now this module's: a worktree off `main`
/// at `.worktrees/<branch>`, with nothing but the branch name typed.
#[test]
fn a_branch_alone_means_a_worktree_off_main() {
    let (command, checkout) = plan(&Spec {
        branch: Some("fix-parser".to_owned()),
        hooks: Some(false),
        ..spec("claude")
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("env"));
    assert_eq!(
        command.args,
        [
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--"
        ]
    );
    assert_eq!(
        checkout,
        GitCheckout::NewWorktree {
            branch: "fix-parser".to_owned(),
            base: DEFAULT_BASE.to_owned(),
            path: PathBuf::from(".worktrees/fix-parser"),
        }
    );
}

#[test]
fn no_branch_means_no_git_work() {
    let (_, checkout) = plan(&spec("codex")).unwrap();
    assert_eq!(checkout, GitCheckout::Current);
}

#[test]
fn a_branch_without_a_worktree_switches_this_checkout() {
    let (_, checkout) = plan(&Spec {
        branch: Some("topic".to_owned()),
        base: Some("develop".to_owned()),
        worktree: Some(false),
        ..spec("claude")
    })
    .unwrap();
    assert_eq!(
        checkout,
        GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "develop".to_owned(),
        }
    );
}

/// The caller says where a worktree is *placed*; what it is called is the branch,
/// here and everywhere else. A trailing slash on the directory is not a second one
/// in the path.
#[test]
fn a_given_directory_holds_a_worktree_still_named_for_its_branch() {
    for directory in ["/elsewhere", "/elsewhere/"] {
        let (_, checkout) = plan(&Spec {
            branch: Some("topic".to_owned()),
            worktree_dir: Some(directory.to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert!(matches!(
            checkout,
            GitCheckout::NewWorktree { path, .. } if path == *"/elsewhere/topic"
        ));
    }
}

/// A directory to put a worktree in, on a spec that will not cut one, is a caller
/// who means something this cannot do - so it is answered rather than dropped.
#[test]
fn a_directory_with_no_worktree_to_put_in_it_is_refused() {
    let refusal = "worktree_dir applies only to a spawn that cuts a worktree";
    assert_eq!(
        plan(&Spec {
            worktree_dir: Some(".worktrees".to_owned()),
            ..spec("claude")
        })
        .unwrap_err(),
        refusal,
    );
    assert_eq!(
        plan(&Spec {
            branch: Some("topic".to_owned()),
            worktree: Some(false),
            worktree_dir: Some(".worktrees".to_owned()),
            ..spec("claude")
        })
        .unwrap_err(),
        refusal,
    );
}

#[test]
fn a_worktree_with_no_branch_says_so_rather_than_running_here() {
    assert_eq!(
        plan(&Spec {
            worktree: Some(true),
            ..spec("claude")
        })
        .unwrap_err(),
        "a branch is required to create a worktree",
    );
}

/// `--model` before the prompt, because the prompt is positional and would
/// otherwise be read as the flag's value.
#[test]
fn the_model_flag_comes_before_the_prompt() {
    let (command, _) = plan(&Spec {
        model: Some("opus".to_owned()),
        prompt: Some("fix the parser".to_owned()),
        args: vec!["--verbose".to_owned()],
        hooks: Some(false),
        ..spec("claude")
    })
    .unwrap();
    assert_eq!(
        command.args,
        vec![
            "YINZ_CLAUDE_INLINE_HOOKS=0".to_owned(),
            "yinz".to_owned(),
            "agent".to_owned(),
            "claude".to_owned(),
            "--command".to_owned(),
            "claude".to_owned(),
            "--".to_owned(),
            "--verbose".to_owned(),
            "--model".to_owned(),
            "opus".to_owned(),
            "fix the parser".to_owned(),
        ]
    );
}

#[test]
fn cursor_is_the_one_kind_that_is_not_its_own_program() {
    assert_eq!(binary_for("cursor"), "cursor-agent");
    for kind in ["claude", "codex", "gemini", "aider", "opencode"] {
        assert_eq!(binary_for(kind), kind);
    }
}

/// The modal's select says `Codex`; the tool's schema says `codex`. Both arrive
/// here and mean the same agent.
#[test]
fn a_kind_is_matched_however_it_was_capitalised() {
    let (command, _) = plan(&spec("Cursor")).unwrap();
    assert_eq!(command.path, PathBuf::from("cursor-agent"));
}

#[test]
fn a_command_stands_in_for_a_kind_but_one_of_them_is_required() {
    let (command, _) = plan(&Spec {
        command: Some("zsh".to_owned()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("zsh"));
    assert_eq!(
        plan(&Spec::default()).unwrap_err(),
        "an agent or a command is required",
    );
}

/// A `claude` pane reports its own state: the hook wiring rides along as `--settings`,
/// and lists exactly the events `yinz hooks install` would wire.
#[test]
fn claude_spawns_carry_the_hook_settings() {
    let (command, _) = plan(&spec("claude")).unwrap();
    assert_eq!(
        &command.args[..8],
        [
            "YINZ_CLAUDE_INLINE_HOOKS=1",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "--settings"
        ]
    );
    let settings: serde_json::Value = serde_json::from_str(&command.args[8]).unwrap();
    let hooks = settings["hooks"].as_object().unwrap();
    for event in crate::agent::CLAUDE_HOOK_EVENTS {
        assert_eq!(
            hooks[*event][0]["hooks"][0]["command"],
            crate::agent::CLAUDE_HOOK_REPORT_COMMAND,
            "{event}"
        );
    }
    // Opted out, and the other kinds never had them.
    let (command, _) = plan(&Spec {
        hooks: Some(false),
        ..spec("claude")
    })
    .unwrap();
    assert_eq!(
        command.args,
        [
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--"
        ]
    );
    let (command, _) = plan(&spec("codex")).unwrap();
    assert_eq!(command.path, PathBuf::from("yinz"));
    assert_eq!(command.args, ["agent", "codex", "--command", "codex", "--"]);
    assert_eq!(
        crate::agent::agent_kind_of(&command.path, &command.args).as_deref(),
        Some("codex")
    );
    let (command, _) = plan(&Spec {
        hooks: Some(false),
        args: vec!["--no-daemon".into(), "original prompt".into()],
        ..spec("codex")
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("yinz"));
    assert_eq!(
        command.args,
        [
            "agent",
            "codex-untracked",
            "--command",
            "codex",
            "--",
            "--no-daemon",
            "original prompt"
        ]
    );
}

fn without_hooks(kind: &str) -> Spec {
    Spec {
        hooks: Some(false),
        ..spec(kind)
    }
}

#[test]
fn env_wraps_the_command_and_keeps_the_prompt_last() {
    let (command, _) = plan(&Spec {
        env: BTreeMap::from([
            ("ANTHROPIC_MODEL".to_owned(), "opus".to_owned()),
            ("FOO".to_owned(), "a=b".to_owned()),
        ]),
        prompt: Some("go".to_owned()),
        ..without_hooks("claude")
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("env"));
    assert_eq!(
        command.args,
        vec![
            "ANTHROPIC_MODEL=opus",
            "FOO=a=b",
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "go"
        ]
    );
    assert_eq!(
        crate::agent::agent_kind_of(&command.path, &command.args).as_deref(),
        Some("claude"),
        "the record is named for the agent, not the wrapper"
    );
}

#[test]
fn claude_permission_mode_and_resume_are_flags() {
    let (command, _) = plan(&Spec {
        permission_mode: Some("acceptEdits".to_owned()),
        resume: Some("abc-123".to_owned()),
        model: Some("opus".to_owned()),
        prompt: Some("go".to_owned()),
        ..without_hooks("claude")
    })
    .unwrap();
    assert_eq!(
        command.args,
        vec![
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "--model",
            "opus",
            "--permission-mode",
            "acceptEdits",
            "--resume",
            "abc-123",
            "go"
        ]
    );
    let (command, _) = plan(&Spec {
        permission_mode: Some("skip".to_owned()),
        resume: Some("last".to_owned()),
        ..without_hooks("claude")
    })
    .unwrap();
    assert_eq!(
        command.args,
        vec![
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "--dangerously-skip-permissions",
            "--continue"
        ]
    );
}

/// `codex resume` is a subcommand, so it goes ahead of everything - including the
/// caller's own arguments - while the approval policy is an ordinary flag.
#[test]
fn codex_resume_is_a_subcommand_ahead_of_the_flags() {
    let (command, _) = plan(&Spec {
        resume: Some("last".to_owned()),
        permission_mode: Some("never".to_owned()),
        args: vec!["--verbose".to_owned()],
        prompt: Some("go".to_owned()),
        ..without_hooks("codex")
    })
    .unwrap();
    assert_eq!(
        command.args,
        vec![
            "agent",
            "codex-untracked",
            "--command",
            "codex",
            "--",
            "resume",
            "--last",
            "--verbose",
            "--ask-for-approval",
            "never",
            "go"
        ]
    );
    let (command, _) = plan(&Spec {
        resume: Some("0192-abc".to_owned()),
        permission_mode: Some("full-auto".to_owned()),
        ..without_hooks("codex")
    })
    .unwrap();
    assert_eq!(
        command.args,
        vec![
            "agent",
            "codex-untracked",
            "--command",
            "codex",
            "--",
            "resume",
            "0192-abc",
            "--full-auto"
        ]
    );
    let (command, _) = plan(&Spec {
        permission_mode: Some("skip".to_owned()),
        ..without_hooks("codex")
    })
    .unwrap();
    assert_eq!(
        command.args,
        vec![
            "agent",
            "codex-untracked",
            "--command",
            "codex",
            "--",
            "--dangerously-bypass-approvals-and-sandbox"
        ]
    );
}

/// `muse resume` is a subcommand like Codex's, the approval policy is an ordinary
/// flag, and the pane runs behind the agent launcher so the session id routes
/// reports back to it - Muse hooks inherit no pane variables to route on.
#[test]
fn muse_resume_is_a_subcommand_behind_the_launcher() {
    let (command, _) = plan(&Spec {
        resume: Some("last".to_owned()),
        permission_mode: Some("never".to_owned()),
        args: vec!["--verbose".to_owned()],
        ..without_hooks("muse")
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("muse"));
    assert_eq!(
        command.args,
        vec!["resume", "--last", "--verbose", "--approval-mode", "never"]
    );
    let (command, _) = plan(&Spec {
        resume: Some("01a0f4fc-89e7".to_owned()),
        permission_mode: Some("skip".to_owned()),
        ..without_hooks("muse")
    })
    .unwrap();
    assert_eq!(command.args, vec!["resume", "01a0f4fc-89e7", "--yolo"]);
    // A fresh session takes its prompt positionally, last of all.
    let (command, _) = plan(&Spec {
        model: Some("muse-spark".to_owned()),
        prompt: Some("go".to_owned()),
        ..without_hooks("muse")
    })
    .unwrap();
    assert_eq!(command.args, vec!["--model", "muse-spark", "go"]);
    // But never beside a resume: `muse resume` takes no opening prompt.
    let refused = plan(&Spec {
        resume: Some("last".to_owned()),
        prompt: Some("go".to_owned()),
        ..spec("muse")
    })
    .unwrap_err();
    assert!(refused.contains("no opening prompt"), "{refused}");
    let refused = plan(&Spec {
        permission_mode: Some("plan".to_owned()),
        ..spec("muse")
    })
    .unwrap_err();
    assert!(
        refused.contains("untrusted, on-request, never, skip"),
        "{refused}"
    );
    // Behind the launcher by default, and bare with hooks off.
    let (command, _) = plan(&spec("muse")).unwrap();
    assert_eq!(command.path, PathBuf::from("yinz"));
    assert_eq!(command.args, ["agent", "muse", "--command", "muse", "--"]);
    let (command, _) = plan(&without_hooks("muse")).unwrap();
    assert_eq!(command.path, PathBuf::from("muse"));
    assert!(command.args.is_empty());
    let command = resume_command("muse", "01a0f4fc-89e7", Some("/w"), None, None).unwrap();
    assert_eq!(command.path, PathBuf::from("yinz"));
    assert_eq!(
        command.args,
        vec![
            "agent",
            "muse",
            "--command",
            "muse",
            "--",
            "resume",
            "01a0f4fc-89e7"
        ]
    );
}

#[test]
fn cursor_only_knows_force() {
    let (command, _) = plan(&Spec {
        permission_mode: Some("skip".to_owned()),
        resume: Some("chat-7".to_owned()),
        ..spec("cursor")
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("cursor-agent"));
    assert_eq!(command.args, vec!["--force", "--resume", "chat-7"]);
    let (command, _) = plan(&Spec {
        resume: Some("last".to_owned()),
        ..spec("cursor")
    })
    .unwrap();
    assert_eq!(command.args, vec!["resume"]);
    let refused = plan(&Spec {
        permission_mode: Some("plan".to_owned()),
        ..spec("cursor")
    })
    .unwrap_err();
    assert!(refused.contains("skip, force"), "{refused}");
}

#[test]
fn opencode_takes_its_prompt_as_a_flag_and_resumes_by_session() {
    // The whole TUI surface at once: a provider/model, `--auto`, a session to
    // continue, and the prompt. The prompt is a flag because opencode's only
    // positional is a project path.
    let (command, _) = plan(&Spec {
        model: Some("anthropic/claude-sonnet-4".to_owned()),
        permission_mode: Some("auto".to_owned()),
        resume: Some("ses_123".to_owned()),
        prompt: Some("fix the parser".to_owned()),
        ..without_hooks("opencode")
    })
    .unwrap();
    assert_eq!(command.path, PathBuf::from("opencode"));
    assert_eq!(
        command.args,
        vec![
            "--model",
            "anthropic/claude-sonnet-4",
            "--auto",
            "--session",
            "ses_123",
            "--prompt",
            "fix the parser",
        ]
    );
    // `skip` is the spelling every other kind shares; `last` is `--continue`.
    let (command, _) = plan(&Spec {
        permission_mode: Some("skip".to_owned()),
        resume: Some("last".to_owned()),
        ..without_hooks("opencode")
    })
    .unwrap();
    assert_eq!(command.args, vec!["--auto", "--continue"]);
    let refused = plan(&Spec {
        permission_mode: Some("plan".to_owned()),
        ..spec("opencode")
    })
    .unwrap_err();
    assert!(refused.contains("auto, skip"), "{refused}");
}

#[test]
fn an_unknown_permission_mode_names_the_accepted_ones() {
    let refused = plan(&Spec {
        permission_mode: Some("yolo".to_owned()),
        ..spec("claude")
    })
    .unwrap_err();
    assert!(refused.contains("\"yolo\""), "{refused}");
    assert!(refused.contains("acceptEdits"), "{refused}");
    assert!(refused.contains("skip"), "{refused}");
}

/// A bare command, or an agent nothing here knows, still takes `--model`; the rest
/// cannot be guessed at and is said so rather than dropped.
#[test]
fn a_raw_command_refuses_modes_it_cannot_spell() {
    let (command, _) = plan(&Spec {
        command: Some("aider".to_owned()),
        model: Some("gpt".to_owned()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(command.args, vec!["--model", "gpt"]);
    assert!(plan(&Spec {
        command: Some("aider".to_owned()),
        permission_mode: Some("skip".to_owned()),
        ..Default::default()
    })
    .is_err());
    assert!(plan(&Spec {
        resume: Some("last".to_owned()),
        ..spec("gemini")
    })
    .unwrap_err()
    .contains("args"));
}

/// A form field tabbed through and a JSON key sent as `""` are the same thing:
/// nothing said. Neither may reach Git as a branch name.
#[test]
fn blank_fields_are_the_same_as_absent_ones() {
    let (command, checkout) = plan(&Spec {
        branch: Some("   ".to_owned()),
        model: Some("".to_owned()),
        prompt: Some(" ".to_owned()),
        permission_mode: Some("".to_owned()),
        resume: Some(" ".to_owned()),
        hooks: Some(false),
        ..spec("claude")
    })
    .unwrap();
    assert_eq!(
        command.args,
        [
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--"
        ]
    );
    assert_eq!(checkout, GitCheckout::Current);
}

// ----------------------------------------------------------------- resuming a run

/// The whole of what `r` in the agents panel means: the same agent, the same session,
/// the folder the run was in - and the hooks, so the resumed pane reports itself and
/// the daemon adopts the run back rather than starting a second one.
#[test]
fn a_claude_run_resumes_in_the_folder_it_recorded() {
    let command = resume_command(
        "claude",
        "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f",
        Some("/home/u/proj"),
        None,
        None,
    )
    .unwrap();
    assert_eq!(command.path, PathBuf::from("env"));
    assert_eq!(
        command.args,
        vec![
            "YINZ_CLAUDE_INLINE_HOOKS=1",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "--resume",
            "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f",
            "--settings",
            &claude_hook_settings_argument(),
        ]
    );
    assert_eq!(command.cwd, Some(PathBuf::from("/home/u/proj")));
    assert_eq!(command.target, PaneTarget::Local);
}

/// `codex resume` is a subcommand, and `resume_command` goes through `plan`, so it gets
/// the same ordering every other caller does rather than a second spelling of its own.
#[test]
fn a_codex_run_resumes_through_its_subcommand() {
    let command = resume_command("codex", "0192-abc", Some("/w"), None, None).unwrap();
    assert_eq!(command.path, PathBuf::from("yinz"));
    assert_eq!(
        command.args,
        vec![
            "agent",
            "codex",
            "--command",
            "codex",
            "--",
            "resume",
            "0192-abc"
        ]
    );
    let command = resume_command("cursor", "chat-7", None, None, None).unwrap();
    assert_eq!(command.path, PathBuf::from("cursor-agent"));
    assert_eq!(command.args, vec!["--resume", "chat-7"]);
    // Nothing recorded, nothing claimed: the pane lands where a command pane with no cwd
    // lands, rather than in a directory guessed at here.
    assert_eq!(command.cwd, None);
}

/// A run whose hook script reported no session id is keyed by pane and start time. There
/// is nothing in that to hand `--resume`, and saying so beats starting a fresh agent the
/// user would take for their old conversation.
#[test]
fn a_run_with_no_session_id_cannot_be_resumed() {
    let refused = resume_command("claude", "", None, None, None).unwrap_err();
    assert!(refused.contains("no session id"), "{refused}");
    assert!(resume_command("claude", "   ", None, None, None).is_err());
}

/// The kinds with no resume spelling are refused by `shape_args`, which this goes
/// through - so there is one list of what can be resumed, not two.
#[test]
fn a_kind_with_no_resume_spelling_is_refused() {
    let refused = resume_command("gemini", "abc", None, None, None).unwrap_err();
    assert!(refused.contains("resume spelling"), "{refused}");
}

#[test]
fn an_ssh_run_keeps_its_host() {
    let command = resume_command("claude", "abc", Some("/srv/w"), Some("build-box"), None).unwrap();
    assert_eq!(command.target, PaneTarget::ssh("build-box").unwrap());
    assert_eq!(command.cwd, Some(PathBuf::from("/srv/w")));
}

/// The host reads a local command's cwd through the plugin mounts, so a run recorded in
/// one of the four folders they claim would open its pane somewhere else entirely - in
/// `$YINZ_TMP_DIR/plugin-tmp` rather than `/tmp`. A refusal says so; the silent version
/// does not.
#[test]
fn a_local_folder_the_plugin_mounts_claim_is_refused() {
    for cwd in ["/tmp/scratch-repo", "/host/w", "/data/w", "/cache/w"] {
        let refused = resume_command("claude", "abc", Some(cwd), None, None).unwrap_err();
        assert!(refused.contains("plugin mounts"), "{cwd}: {refused}");
    }
    // Only those four, and only on their own component boundaries.
    assert!(resume_command("claude", "abc", Some("/tmpfoo/w"), None, None).is_ok());
    assert!(resume_command("claude", "abc", Some("/home/u/proj"), None, None).is_ok());
    // And not remotely: a remote cwd is a path on the far machine, which the host passes
    // through untouched.
    let command = resume_command(
        "claude",
        "abc",
        Some("/tmp/scratch-repo"),
        Some("build-box"),
        None,
    )
    .unwrap();
    assert_eq!(command.cwd, Some(PathBuf::from("/tmp/scratch-repo")));
}

#[test]
fn a_sandboxed_run_resumes_in_its_bubblewrap_environment() {
    let command = resume_command(
        "claude",
        "abc",
        Some("/repo/.worktrees/task"),
        None,
        Some("/repo"),
    )
    .unwrap();
    assert_eq!(
        command.target,
        PaneTarget::Bubblewrap {
            root: PathBuf::from("/repo")
        }
    );
    assert_eq!(command.cwd, Some(PathBuf::from("/repo/.worktrees/task")));
    // The pane is on this machine, so the plugin mounts still claim their paths.
    assert!(resume_command("claude", "abc", Some("/tmp/w"), None, Some("/repo")).is_err());
    assert!(resume_command("claude", "abc", Some("/w"), None, Some("relative")).is_err());
    assert!(resume_command("claude", "abc", Some("/w"), Some("box"), Some("/repo")).is_err());
}

#[test]
fn agy_launch_spec_translation() {
    assert_eq!(normalize_kind("agy"), "agy");
    assert_eq!(normalize_kind("antigravity"), "agy");
    assert_eq!(binary_for("agy"), "agy");
    assert_eq!(binary_for("antigravity"), "agy");

    for alias in ["agy", "antigravity", "Antigravity"] {
        let (command, _) = plan(&Spec {
            kind: Some(alias.into()),
            effort: Some("high".into()),
            permission_mode: Some("skip".into()),
            prompt: Some("investigate bug".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(command.path, PathBuf::from("agy"), "{alias}");
        assert_eq!(
            command.args,
            vec![
                "--effort",
                "high",
                "--dangerously-skip-permissions",
                "--prompt-interactive",
                "investigate bug"
            ],
            "{alias}"
        );

        let (command, _) = plan(&Spec {
            kind: Some(alias.into()),
            permission_mode: Some("accept-edits".into()),
            resume: Some("last".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            command.args,
            vec!["--mode", "accept-edits", "--continue"],
            "{alias}"
        );

        let (command, _) = plan(&Spec {
            kind: Some(alias.into()),
            permission_mode: Some("plan".into()),
            resume: Some("conv-abc".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            command.args,
            vec!["--mode", "plan", "--conversation", "conv-abc"],
            "{alias}"
        );
    }

    let command = resume_command("antigravity", "conv-xyz", Some("/w"), None, None).unwrap();
    assert_eq!(command.path, PathBuf::from("agy"));
    assert_eq!(command.args, vec!["--conversation", "conv-xyz"]);
}
