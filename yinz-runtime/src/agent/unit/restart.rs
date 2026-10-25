use super::*;
use crate::agent::launch::{plan, Spec};

#[test]
fn restart_replaces_prompt_and_session_inside_wrappers_preserving_flags() {
    for (program, prefix) in [
        ("claude", vec![]),
        (
            "yinz",
            strings(&["agent", "claude", "--command", "/opt/claude", "--"]),
        ),
        (
            "env",
            strings(&[
                "-C",
                "/tmp/topic",
                "SETTING=a b",
                "yinz",
                "agent",
                "claude",
                "--command",
                "claude",
                "--",
            ]),
        ),
    ] {
        let flags = strings(&["--model", "X", "--permission-mode", "Y", "--add-dir=/extra"]);
        let original = [
            prefix.clone(),
            flags.clone(),
            strings(&["--resume", "old", "task"]),
        ]
        .concat();
        for session in [None, Some("sid")] {
            let expected = [
                prefix.clone(),
                flags.clone(),
                if session.is_some() {
                    strings(&["--resume", "sid", "P"])
                } else {
                    strings(&["P"])
                },
            ]
            .concat();
            assert_eq!(
                restart_with_prompt(Path::new(program), &original, session, Some("P")).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn restart_distinguishes_option_values_from_separators() {
    crate::agent::provider_fixture::register_opencode();
    for (kind, options, tail, retained, resume, prompt) in [
        (
            "claude",
            vec!["--system-prompt", "--"],
            vec!["old task"],
            vec![],
            vec!["--resume", "sid"],
            vec!["P"],
        ),
        (
            "claude",
            vec!["--system-prompt", "--"],
            vec!["--", "old task"],
            vec![],
            vec!["--resume", "sid"],
            vec!["P"],
        ),
        (
            "opencode",
            vec!["-m", "--"],
            vec!["--prompt", "old task"],
            vec![],
            vec!["--session", "sid"],
            vec!["--prompt", "P"],
        ),
        (
            "opencode",
            vec!["-m", "--"],
            vec!["--prompt", "old task", "--", "-project"],
            vec!["--", "-project"],
            vec!["--session", "sid"],
            vec!["--prompt", "P"],
        ),
    ] {
        let mut wrappers = vec![
            (kind, vec![]),
            ("env", strings(&["--", "SETTING=1", kind])),
            (
                "env",
                strings(&["--", "SETTING=1", "env", "-u", "OLD", kind]),
            ),
        ];
        if kind == "claude" {
            wrappers.push(("yinz", strings(&["agent", kind, "--command", kind, "--"])));
        }
        for (program, prefix) in wrappers {
            let original = [prefix.clone(), strings(&options), strings(&tail)].concat();
            for session in [None, Some("sid")] {
                let expected = [
                    prefix.clone(),
                    strings(&options),
                    if session.is_some() {
                        strings(&resume)
                    } else {
                        vec![]
                    },
                    strings(&prompt),
                    strings(&retained),
                ]
                .concat();
                assert_eq!(
                    restart_with_prompt(Path::new(program), &original, session, Some("P")).unwrap(),
                    expected,
                    "{program}: {original:?}, {session:?}"
                );
            }
        }
    }
}

#[test]
fn managed_codex_restart_rejects_preserved_permission_overrides() {
    for mode in std::iter::once(None).chain(
        crate::agent::launch::CODEX_PERMISSION_MODES
            .iter()
            .copied()
            .map(Some),
    ) {
        for hooks in [true, false] {
            for with_env in [false, true] {
                let spec = Spec {
                    kind: Some("codex".into()),
                    permission_mode: mode.map(str::to_owned),
                    hooks: Some(hooks),
                    prompt: Some("original task".into()),
                    env: if with_env {
                        [("SETTING".into(), "value".into())].into()
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                };
                let (original, _) = plan(&spec).unwrap();
                let (fresh, _) = plan(&Spec {
                    prompt: Some("new task".into()),
                    ..spec
                })
                .unwrap();
                assert_eq!(
                    restart_with_prompt(&original.path, &original.args, None, Some("new task"))
                        .unwrap(),
                    fresh.args
                );
                for session in ["sid", "last"] {
                    let result = restart_with_prompt(
                        &original.path,
                        &original.args,
                        Some(session),
                        Some("new task"),
                    );
                    if hooks && mode.is_some() {
                        let error = result.unwrap_err();
                        assert!(error.contains("managed Codex"), "{error}");
                        assert!(error.contains("fresh"), "{error}");
                    } else {
                        assert!(result.is_ok(), "{mode:?}, {hooks}, {with_env}: {result:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn standalone_codex_resumes_preserve_permission_overrides() {
    for standalone in [
        vec!["--no-daemon"],
        vec!["--add-dir", "/extra"],
        vec!["--add-dir=/extra"],
        vec!["--remote", "unix:///tmp/codex.sock"],
        vec!["--remote=unix:///tmp/codex.sock"],
        vec!["-c", "sandbox_workspace_write.writable_roots=['/extra']"],
        vec!["--config=sandbox_workspace_write.writable_roots=['/extra']"],
    ] {
        for permissions in [
            vec!["--full-auto"],
            vec!["--dangerously-bypass-approvals-and-sandbox"],
            vec!["--ask-for-approval", "never"],
            vec!["--ask-for-approval=never"],
            vec!["-a", "never"],
            vec!["-a=never"],
            vec!["--sandbox", "read-only"],
            vec!["--sandbox=read-only"],
            vec!["-s", "read-only"],
            vec!["-s=read-only"],
        ] {
            for options in [
                [strings(&standalone), strings(&permissions)].concat(),
                [strings(&permissions), strings(&standalone)].concat(),
            ] {
                for (program, prefix) in [
                    ("codex", vec![]),
                    ("yinz", strings(&["agent", "codex", "--"])),
                    (
                        "yinz",
                        strings(&["agent", "codex", "--command", "/opt/codex", "--"]),
                    ),
                    (
                        "env",
                        strings(&["--", "SETTING=value", "yinz", "agent", "codex", "--"]),
                    ),
                ] {
                    for previous in [
                        strings(&["old task"]),
                        strings(&["resume", "old-session", "old task"]),
                    ] {
                        let original = [prefix.clone(), options.clone(), previous].concat();
                        for session in ["session-id", "last"] {
                            let resume = if session == "last" { "--last" } else { session };
                            let expected = [
                                prefix.clone(),
                                strings(&["resume", resume]),
                                options.clone(),
                                strings(&["replacement prompt"]),
                            ]
                            .concat();
                            assert_eq!(
                                restart_with_prompt(
                                    Path::new(program),
                                    &original,
                                    Some(session),
                                    Some("replacement prompt"),
                                )
                                .unwrap(),
                                expected,
                                "{program}: {original:?}, {session}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn codex_restart_validates_options_only_inside_managed_wrappers() {
    for flags in [
        vec!["--full-auto"],
        vec!["--dangerously-bypass-approvals-and-sandbox"],
        vec!["--ask-for-approval", "never"],
        vec!["--ask-for-approval=never"],
        vec!["-a", "never"],
        vec!["-a=never"],
        vec!["--sandbox", "read-only"],
        vec!["--sandbox=read-only"],
        vec!["-s", "read-only"],
        vec!["-s=read-only"],
    ] {
        for prefix in [
            vec!["agent", "codex", "--"],
            vec!["agent", "codex", "--command", "/opt/codex", "--"],
        ] {
            let args = [strings(&prefix), strings(&flags)].concat();
            let error =
                restart_with_prompt(Path::new("yinz"), &args, Some("sid"), None).unwrap_err();
            assert!(
                error.contains(flags[0].split('=').next().unwrap()),
                "{error}"
            );
        }
        assert!(
            restart_with_prompt(Path::new("codex"), &strings(&flags), Some("sid"), None).is_ok()
        );
    }
    for args in [
        vec!["--model", "--full-auto"],
        vec!["--model=--full-auto"],
        vec!["--", "--full-auto"],
        vec!["original task mentions --full-auto"],
    ] {
        let original = [strings(&["agent", "codex", "--"]), strings(&args)].concat();
        assert!(restart_with_prompt(
            Path::new("yinz"),
            &original,
            Some("sid"),
            Some("--full-auto")
        )
        .is_ok());
    }
}

#[test]
fn restart_uses_provider_resume_and_prompt_grammar() {
    crate::agent::provider_fixture::register_opencode();
    for (program, original, expected) in [
        (
            "codex",
            vec!["--model", "X", "task"],
            vec!["resume", "sid", "--model", "X", "P"],
        ),
        (
            "agy",
            vec!["--model", "X", "--prompt-interactive", "task"],
            vec![
                "--model",
                "X",
                "--conversation",
                "sid",
                "--prompt-interactive",
                "P",
            ],
        ),
        (
            "opencode",
            vec!["--prompt=task", "--", "-project"],
            vec!["--session", "sid", "--prompt", "P", "--", "-project"],
        ),
        (
            "opencode",
            vec!["--prompt=task", "-m", "X", "/project"],
            vec!["-m", "X", "/project", "--session", "sid", "--prompt", "P"],
        ),
    ] {
        assert_eq!(
            restart_with_prompt(
                Path::new(program),
                &strings(&original),
                Some("sid"),
                Some("P")
            )
            .unwrap(),
            strings(&expected),
            "{program}"
        );
    }
    assert!(
        restart_with_prompt(Path::new("muse"), &[], Some("sid"), Some("P"))
            .unwrap_err()
            .contains("no opening prompt")
    );
    assert!(restart_with_prompt(
        Path::new("claude"),
        &strings(&["--unknown", "task"]),
        None,
        Some("P")
    )
    .is_err());
    assert_eq!(
        restart_with_prompt(
            Path::new("muse"),
            &strings(&["--model", "X", "task"]),
            Some("sid"),
            None
        )
        .unwrap(),
        strings(&["resume", "sid", "--model", "X"])
    );
}

#[test]
fn provider_restart_keeps_existing_defaults_once_and_positions_resume_first() {
    use crate::agent::provider::{provider, register};
    crate::agent::provider_fixture::register_opencode();
    let mut custom = (*provider("opencode").unwrap()).clone();
    custom.kind = "restart-fixture".into();
    custom.executable = "restart-fixture".into();
    custom.launch.args = strings(&["--auto"]);
    custom.launch.resume_before_args = true;
    register(42, custom).unwrap();
    assert_eq!(
        restart_with_prompt(
            Path::new("restart-fixture"),
            &strings(&["--auto", "--model", "X", "--prompt", "task"]),
            Some("sid"),
            Some("P")
        )
        .unwrap(),
        strings(&[
            "--session",
            "sid",
            "--auto",
            "--model",
            "X",
            "--prompt",
            "P"
        ])
    );
}

#[test]
fn pi_restart_removes_prompts_and_session_selectors_with_exact_arity() {
    let args = strings(&[
        "--model",
        "sonnet",
        "-r",
        "--session",
        "old",
        "--fork",
        "forked",
        "--session-id",
        "id",
        "--continue",
        "-e",
        "user.ts",
        "--models",
        "a,b",
        "-t",
        "read",
        "-xt",
        "bash",
        "-n",
        "name",
        "--mode",
        "text",
        "-ne",
        "--",
        "hello",
        "world",
        "@file",
    ]);
    assert_eq!(
        restart_args(Path::new("pi"), &args).unwrap(),
        strings(&[
            "--model", "sonnet", "-e", "user.ts", "--models", "a,b", "-t", "read", "-xt", "bash",
            "-n", "name", "--mode", "text", "-ne"
        ])
    );
    for args in [
        vec!["--print", "prompt"],
        vec!["-p"],
        vec!["--mode", "json"],
        vec!["--mode", "rpc"],
        vec!["--model"],
        vec!["--session", "--model", "sonnet"],
        vec!["--model=sonnet"],
        vec!["--unknown", "value"],
        vec!["--export", "session"],
        vec!["--list-models"],
        vec!["install", "package"],
    ] {
        assert!(
            restart_args(Path::new("pi"), &strings(&args)).is_err(),
            "{args:?}"
        );
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn restart_preserves_planned_options_and_environment_without_prompt_or_resume() {
    crate::agent::provider_fixture::register_opencode();
    for (kind, mode) in [
        ("claude", "skip"),
        ("codex", "full-auto"),
        ("cursor", "force"),
        ("opencode", "auto"),
        ("muse", "skip"),
        ("agy", "skip"),
        ("agy", "accept-edits"),
    ] {
        for resume in [None, Some("last"), Some("conversation-id")] {
            let mut spec = Spec {
                kind: Some(kind.into()),
                model: Some("chosen-model".into()),
                permission_mode: (kind != "codex" || resume.is_none()).then(|| mode.into()),
                // `muse resume` takes no opening prompt, so the planner refuses
                // the combination; every other kind plans - and restarts - both.
                prompt: (kind != "muse" || resume.is_none()).then(|| "original work".into()),
                resume: resume.map(str::to_owned),
                env: [("SETTING".into(), "a b=c".into())].into(),
                args: if kind == "opencode" {
                    strings(&["/project with spaces"])
                } else {
                    vec![]
                },
                ..Default::default()
            };
            let (original, _) = plan(&spec).unwrap();
            spec.prompt = None;
            spec.resume = None;
            let (expected, _) = plan(&spec).unwrap();
            let restarted = restart_args(&original.path, &original.args).unwrap();
            assert_eq!(restarted, expected.args, "{kind}: {resume:?}");
            assert_eq!(restart_args(&original.path, &restarted).unwrap(), restarted);
        }
    }
}

#[test]
fn restart_obeys_agent_argument_arity() {
    crate::agent::provider_fixture::register_opencode();
    for (program, input, expected) in [
        (
            "claude",
            vec![
                "--model",
                "resume",
                "prompt",
                "--permission-mode",
                "plan",
                "--settings",
                "{}",
            ],
            vec![
                "--model",
                "resume",
                "--permission-mode",
                "plan",
                "--settings",
                "{}",
            ],
        ),
        (
            "claude",
            vec![
                "-r",
                "id",
                "-c",
                "--session-id=new-id",
                "--fork-session",
                "--resume-session-at",
                "message",
                "--model=sonnet",
                "--",
                "--literal prompt",
            ],
            vec!["--model=sonnet"],
        ),
        (
            "claude",
            vec!["--resume", "--model", "sonnet", "prompt"],
            vec!["--model", "sonnet"],
        ),
        (
            "codex",
            vec![
                "-c",
                "model_reasoning_effort=\"high\"",
                "resume",
                "--last",
                "--all",
                "prompt",
                "-p",
                "profile",
                "-s",
                "workspace-write",
            ],
            vec![
                "-c",
                "model_reasoning_effort=\"high\"",
                "-p",
                "profile",
                "-s",
                "workspace-write",
            ],
        ),
        (
            "codex",
            vec!["fork", "id", "prompt", "--model=gpt", "--image=image.png"],
            vec!["--model=gpt"],
        ),
        (
            "cursor-agent",
            vec![
                "--resume=id",
                "--continue",
                "--force",
                "a",
                "multiword",
                "prompt",
                "--workspace",
                "/work",
            ],
            vec!["--force", "--workspace", "/work"],
        ),
        (
            "opencode",
            vec![
                "--prompt=work",
                "-c",
                "-s",
                "id",
                "--fork",
                "-m",
                "provider/model",
                "--",
                "-project",
            ],
            vec!["-m", "provider/model", "--", "-project"],
        ),
        (
            "muse",
            vec![
                "resume",
                "--last",
                "--approval-mode",
                "never",
                "--model",
                "muse-spark",
                "--image=shot.png",
                "--session-id",
                "old-session",
                "--prompt-file",
                "prompt.md",
                "original prompt",
            ],
            vec!["--approval-mode", "never", "--model", "muse-spark"],
        ),
        (
            "muse-bin-1.4.2-R4684.1",
            vec!["--yolo", "work"],
            vec!["--yolo"],
        ),
        (
            "yinz",
            vec![
                "agent",
                "muse",
                "--command",
                "muse",
                "--",
                "resume",
                "abc",
                "--model",
                "m",
            ],
            vec!["agent", "muse", "--command", "muse", "--", "--model", "m"],
        ),
        (
            "/usr/bin/env",
            vec![
                "-i",
                "-u",
                "OLD",
                "-C",
                "/dir",
                "K=V",
                "/usr/bin/env",
                "--",
                "OTHER=1",
                "/opt/bin/codex",
                "--model",
                "gpt",
                "work",
            ],
            vec![
                "-i",
                "-u",
                "OLD",
                "-C",
                "/dir",
                "K=V",
                "/usr/bin/env",
                "--",
                "OTHER=1",
                "/opt/bin/codex",
                "--model",
                "gpt",
            ],
        ),
        (
            "agy",
            vec![
                "--model",
                "chosen-model",
                "--prompt-interactive",
                "work",
                "--mode",
                "plan",
                "--effort",
                "high",
            ],
            vec![
                "--model",
                "chosen-model",
                "--mode",
                "plan",
                "--effort",
                "high",
            ],
        ),
        (
            "agy",
            vec![
                "-i",
                "work",
                "-c",
                "--conversation=conv-123",
                "--dangerously-skip-permissions",
                "--",
                "--literal prompt",
            ],
            vec!["--dangerously-skip-permissions"],
        ),
        ("agy", vec!["--conversation", "conv-1", "work"], vec![]),
        (
            "agy",
            vec!["--add-dir", "/work", "--prompt-interactive", "task"],
            vec!["--add-dir", "/work"],
        ),
        (
            "agy",
            vec!["--add-dir", "/work", "prompt"],
            vec!["--add-dir", "/work"],
        ),
        (
            "agy",
            vec!["--add-dir", "/work", "--add-dir", "/extra dir", "task"],
            vec!["--add-dir", "/work", "--add-dir", "/extra dir"],
        ),
        (
            "agy",
            vec!["--add-dir=/extra", "--prompt-interactive", "work"],
            vec!["--add-dir=/extra"],
        ),
        (
            "antigravity",
            vec![
                "--model",
                "custom",
                "--add-dir",
                "/work",
                "--prompt-interactive",
                "work",
            ],
            vec!["--model", "custom", "--add-dir", "/work"],
        ),
    ] {
        assert_eq!(
            restart_args(Path::new(program), &strings(&input)).unwrap(),
            strings(&expected),
            "{program}: {input:?}"
        );
    }
}

#[test]
fn muse_restart_preserves_workspace() {
    for workspace in [
        strings(&["--workspace", "/project with spaces"]),
        strings(&["--workspace=/project with spaces"]),
    ] {
        for session in [
            strings(&["original prompt"]),
            strings(&["resume", "session-id"]),
        ] {
            for (program, prefix) in [
                ("muse", vec![]),
                (
                    "yinz",
                    strings(&["agent", "muse", "--command", "muse", "--"]),
                ),
            ] {
                let expected = [prefix, workspace.clone()].concat();
                let input = [expected.clone(), session.clone()].concat();
                let restarted = restart_args(Path::new(program), &input).unwrap();
                assert_eq!(restarted, expected);
                assert_eq!(
                    restart_args(Path::new(program), &restarted).unwrap(),
                    expected
                );
            }
        }
    }
    assert_eq!(
        restart_args(Path::new("muse"), &strings(&["--workspace"])).unwrap_err(),
        "missing value for --workspace"
    );
}

#[test]
fn restart_refuses_print_mode_including_env_wrappers() {
    for program in ["claude", "cursor-agent", "agy"] {
        for flag in ["-p", "--print"] {
            assert!(restart_args(Path::new(program), &strings(&[flag, "work"])).is_err());
            assert!(restart_args(
                Path::new("env"),
                &strings(&["SETTING=1", program, flag, "work"])
            )
            .is_err());
        }
    }
    assert!(restart_args(Path::new("agy"), &strings(&["--prompt", "work"])).is_err());
    assert_eq!(
        restart_args(Path::new("codex"), &strings(&["-p", "profile", "work"])).unwrap(),
        strings(&["-p", "profile"])
    );
}

#[test]
fn restart_refuses_unknown_or_ambiguous_commands() {
    crate::agent::provider_fixture::register_opencode();
    for (program, input) in [
        ("gemini", vec!["work"]),
        ("sh", vec!["-c", "claude work"]),
        ("cursor", vec!["work"]),
        ("codex", vec!["--new-option", "value", "prompt"]),
        ("claude", vec!["--model"]),
        ("claude", vec!["--add-dir", "/work", "prompt"]),
        ("codex", vec!["exec", "work"]),
        ("opencode", vec!["run", "work"]),
        ("opencode", vec!["/one", "/two"]),
        ("claude", vec!["one", "two"]),
        ("muse", vec!["exec", "work"]),
        ("muse", vec!["--worktree", "create", "prompt"]),
        ("muse", vec!["--image", "shot.png", "prompt"]),
        ("muse", vec!["one", "two"]),
        ("muse", vec!["resume", "abc", "def"]),
        ("env", vec!["--split-string", "claude work"]),
        ("env", vec!["-u"]),
        ("agy", vec!["update"]),
        ("agy", vec!["one", "two"]),
        ("agy", vec!["--add-dir"]),
        ("agy", vec!["--model"]),
    ] {
        assert!(
            restart_args(Path::new(program), &strings(&input)).is_err(),
            "{program}: {input:?}"
        );
    }
}
