use std::path::Path;

/// Remove an agent's opening prompt and conversation selectors without changing its launch
/// options. Unknown option arities and non-interactive commands must be refused: guessing
/// could replay work or silently change the model, permissions, or working directory.
pub fn restart_args(program: &Path, args: &[String]) -> Result<Vec<String>, String> {
    let name = program.file_name().and_then(|name| name.to_str());
    if matches!(name, Some("yinz" | "sh")) {
        if let Some((inner, inner_args)) = crate::agent::unwrapped_program_and_args(program, args) {
            if inner != program {
                let mut result = args[..args.len() - inner_args.len()].to_vec();
                result.extend(restart_args(inner, inner_args)?);
                return Ok(result);
            }
        }
    }
    if name == Some("env") {
        let mut index = 0;
        let mut options_done = false;
        while let Some(arg) = args.get(index) {
            if options_done {
                if arg.contains('=') && !arg.starts_with('-') {
                    index += 1;
                    continue;
                }
                break;
            }
            index += 1;
            match arg.as_str() {
                "--" => options_done = true,
                "-i" | "--ignore-environment" | "-" => {},
                "-u" | "--unset" | "-C" | "--chdir" => {
                    args.get(index).ok_or("incomplete env option")?;
                    index += 1;
                },
                _ if arg.starts_with("--unset=") || arg.starts_with("--chdir=") => {},
                _ if !arg.starts_with('-') && arg.contains('=') => {},
                _ if arg.starts_with('-') => {
                    return Err("unsupported env option for restart".into())
                },
                _ => {
                    index -= 1;
                    break;
                },
            }
        }
        let program = args.get(index).ok_or("env has no agent command")?;
        let mut cleaned = args[..=index].to_vec();
        cleaned.extend(restart_args(Path::new(program), &args[index + 1..])?);
        return Ok(cleaned);
    }
    if let Some(provider) = name
        .and_then(crate::agent::kind_of)
        .as_deref()
        .and_then(crate::agent::provider::provider)
    {
        let grammar = provider
            .restart
            .as_ref()
            .ok_or_else(|| format!("{} does not support restart", provider.kind))?;
        return provider_restart_args(&provider.kind, grammar, args);
    }
    let kind = match name {
        Some("claude") => "claude",
        Some("codex") => "codex",
        Some("cursor-agent") => "cursor",
        Some("muse") => "muse",
        // The launcher execs a versioned binary (`muse-bin-1.4.2-R4684.1`); the pane
        // is still a muse pane, the way `kind_of` reads it.
        Some(name) if name.starts_with("muse-bin-") => "muse",
        Some("agy" | "antigravity") => "agy",
        Some("pi") => "pi",
        _ => return Err("unsupported agent command for restart".into()),
    };
    // Only fixed-arity options are accepted. Variadic options must use an inline value;
    // otherwise a following word could be either another value or the startup prompt.
    let (values, switches, variadic, commands): (&[&str], &[&str], &[&str], &[&str]) = match kind {
        "claude" => (
            &[
                "--model",
                "--permission-mode",
                "--effort",
                "--settings",
                "--setting-sources",
                "--system-prompt",
                "--append-system-prompt",
                "--agent",
                "--agents",
                "--output-format",
                "--input-format",
                "--fallback-model",
                "--max-budget-usd",
                "--json-schema",
                "--plugin-dir",
                "--plugin-url",
            ],
            &[
                "--dangerously-skip-permissions",
                "--allow-dangerously-skip-permissions",
                "--verbose",
                "--strict-mcp-config",
                "--no-session-persistence",
                "--disable-slash-commands",
                "--safe-mode",
                "--bare",
            ],
            &[
                "--add-dir",
                "--allowedTools",
                "--allowed-tools",
                "--disallowedTools",
                "--disallowed-tools",
                "--tools",
                "--mcp-config",
            ],
            &[
                "agents",
                "attach",
                "auth",
                "auto-mode",
                "doctor",
                "gateway",
                "import",
                "install",
                "logs",
                "mcp",
                "plugin",
                "plugins",
                "project",
                "respawn",
                "rm",
                "setup-token",
                "stop",
                "kill",
                "ultrareview",
                "update",
                "upgrade",
                "help",
            ],
        ),
        "codex" => (
            &[
                "-m",
                "--model",
                "-c",
                "--config",
                "-a",
                "--ask-for-approval",
                "-s",
                "--sandbox",
                "-p",
                "--profile",
                "-C",
                "--cd",
                "--add-dir",
                "--enable",
                "--disable",
                "--local-provider",
                "--remote",
                "--remote-auth-token-env",
            ],
            &[
                "--full-auto",
                "--dangerously-bypass-approvals-and-sandbox",
                "--approve-for-me",
                "--dangerously-bypass-hook-trust",
                "--oss",
                "--search",
                "--no-alt-screen",
                "--no-daemon",
                "--strict-config",
            ],
            &["-i", "--image"],
            &[
                "agents",
                "exec",
                "e",
                "review",
                "login",
                "logout",
                "mcp",
                "plugin",
                "app-server",
                "remote-control",
                "completion",
                "update",
                "doctor",
                "sandbox",
                "debug",
                "apply",
                "a",
                "queue",
                "archive",
                "delete",
                "migrate-rollouts",
                "unarchive",
                "cloud",
                "exec-server",
                "features",
                "help",
            ],
        ),
        "cursor" => (
            &[
                "--model",
                "--mode",
                "--sandbox",
                "--workspace",
                "--add-dir",
                "--plugin-dir",
                "--api-key",
                "-H",
                "--header",
                "-e",
                "--endpoint",
                "--output-format",
            ],
            &[
                "-f",
                "--force",
                "--yolo",
                "--auto-review",
                "--approve-mcps",
                "--trust",
                "--plan",
                "--stream-partial-output",
            ],
            &[],
            &[
                "install-shell-integration",
                "uninstall-shell-integration",
                "login",
                "logout",
                "mcp",
                "plugin",
                "worker",
                "status",
                "whoami",
                "models",
                "bedrock",
                "about",
                "update",
                "create-chat",
                "generate-rule",
                "rule",
                "agent",
                "ls",
                "help",
            ],
        ),
        "muse" => (
            &[
                "--workspace",
                "--model",
                "--reasoning-effort",
                "--provider",
                "--preset",
                "--agents",
                "--base-url",
                "--permission-profile",
                "--approval-mode",
                "--approval-judge",
                "--echo-delay-ms",
                "--sandbox-network",
                "--worktree-base",
                "--worktree-existing",
                "--output-schema",
                "--max-model-steps",
                "--max-tool-output-bytes",
                "--context-compaction-strategy",
                "--context-compaction-soft-threshold",
                "--context-compaction-hard-threshold",
            ],
            &[
                "--yolo",
                "--trust-workspace",
                "--disable-approval",
                "--disable-sandbox",
                "--disable-write",
                "--disable-shell",
                "--enable-shell-tool",
                "--disable-web-tools",
                "--no-foreign-personal-context",
                "--no-session-log",
                "--parallel-tool-calls",
                "--no-parallel-tool-calls",
                "--subagent-worktree-isolation",
                "--allow-workspace-switch",
                "--user-input-auto-resolve",
                "--json",
            ],
            // Inline only, and dropped: an image belonged to the old turn. The
            // separate-value form is refused below - the next word could be the
            // image or the startup prompt. `-w`/`--worktree` are missing for the
            // same reason: a bare `-w` takes no value and `--worktree create`
            // takes one, so the arity cannot be known.
            &["--image"],
            &[
                "exec",
                "config",
                "export",
                "trace",
                "model-profile",
                "skills",
                "voice",
                "plugins",
                "sandbox",
                "schema",
                "serve",
                "session-message",
                "mcp",
                "auth",
                "login",
                "logout",
                "init",
                "help",
            ],
        ),
        "agy" => (
            &[
                "--model",
                "--mode",
                "--effort",
                "--agent",
                "--project",
                "--add-dir",
                "--log-file",
                "--json-schema",
                "--input-format",
                "--output-format",
                "--print-timeout",
            ],
            &[
                "--dangerously-skip-permissions",
                "--disable-slash-commands",
                "--new-project",
                "--remote-control",
                "--sandbox",
            ],
            &[],
            &[
                "agent",
                "agents",
                "changelog",
                "help",
                "install",
                "mcp",
                "mic-serve",
                "models",
                "plugin",
                "plugins",
                "remote-control",
                "update",
            ],
        ),
        "pi" => (
            &[
                "--model",
                "--models",
                "--provider",
                "--thinking",
                "--mode",
                "-t",
                "--tools",
                "-xt",
                "--exclude-tools",
                "-e",
                "--extension",
                "--skill",
                "--prompt-template",
                "--theme",
                "--use-theme",
                "--session-dir",
                "--system-prompt",
                "--append-system-prompt",
                "--api-key",
                "-n",
                "--name",
                "--tui-mode",
            ],
            &[
                "-ne",
                "--no-extensions",
                "-ns",
                "--no-skills",
                "-np",
                "--no-prompt-templates",
                "--no-themes",
                "-nc",
                "--no-context-files",
                "--no-session",
                "-nt",
                "--no-tools",
                "-nbt",
                "--no-builtin-tools",
                "--verbose",
                "-a",
                "--approve",
                "-na",
                "--no-approve",
                "--offline",
            ],
            &[],
            &[
                "install",
                "remove",
                "uninstall",
                "update",
                "list",
                "config",
                "auth",
                "mcp",
                "help",
            ],
        ),
        _ => unreachable!(),
    };
    let mut cleaned = Vec::new();
    let mut index = 0;
    let mut positionals = 0;
    let mut resume = false;
    let mut after_separator = false;
    while let Some(arg) = args.get(index) {
        index += 1;
        if arg == "--" && !after_separator {
            after_separator = true;
            continue;
        }
        if !after_separator && arg.starts_with('-') {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
            if kind == "pi"
                && (inline.is_some()
                    || (flag == "--mode" && args.get(index).map(String::as_str) != Some("text")))
            {
                return Err(format!(
                    "unsupported or ambiguous Pi option {arg} for restart"
                ));
            }
            let remove_value = matches!(
                (kind, flag),
                ("claude", "--session-id" | "--resume-session-at")
                    | ("muse", "--session-id" | "--prompt-file")
                    | ("agy", "--conversation" | "--prompt-interactive" | "-i")
                    // pi opens by path, partial id, or exact id: all three name the
                    // conversation the old turn lived in.
                    | ("pi", "--session" | "--session-id" | "--fork")
            );
            let remove_optional = matches!(
                (kind, flag),
                ("claude", "-r" | "--resume") | ("cursor", "--resume")
            );
            let remove_switch =
                matches!(
                    (kind, flag),
                    ("claude", "-c" | "--continue" | "--fork-session")
                        | ("cursor", "--continue")
                        | ("muse", "--last")
                        | ("agy", "-c" | "--continue")
                        | ("pi", "-c" | "--continue" | "-r" | "--resume")
                ) || (kind == "codex" && resume && matches!(flag, "--last" | "--all"));
            if remove_optional {
                if inline.is_none() && args.get(index).is_some_and(|next| !next.starts_with('-')) {
                    index += 1;
                }
            } else if remove_value || values.contains(&flag) {
                if !remove_value {
                    cleaned.push(arg.clone());
                }
                if inline.is_none() {
                    let value = args
                        .get(index)
                        .ok_or_else(|| format!("missing value for {flag}"))?;
                    if kind == "pi" && (value.is_empty() || value.starts_with('-')) {
                        return Err(format!("missing or ambiguous value for {flag}"));
                    }
                    if !remove_value {
                        cleaned.push(value.clone());
                    }
                    index += 1;
                }
            } else if (remove_switch || switches.contains(&flag)) && inline.is_none() {
                if !remove_switch {
                    cleaned.push(arg.clone());
                }
            } else if variadic.contains(&flag) && inline.is_some() {
                // Images are input to the old turn, not launch settings.
                if !matches!(kind, "codex" | "muse") {
                    cleaned.push(arg.clone());
                }
            } else {
                return Err(format!(
                    "unsupported or ambiguous {kind} option {flag} for restart"
                ));
            }
        } else {
            if positionals == 0 && !after_separator {
                if (kind == "codex" && matches!(arg.as_str(), "resume" | "fork"))
                    || (kind == "cursor" && arg == "resume")
                    || (kind == "muse" && arg == "resume")
                {
                    if resume {
                        return Err("ambiguous resume command".into());
                    }
                    resume = true;
                    continue;
                }
                if !resume && commands.contains(&arg.as_str()) {
                    return Err(format!("unsupported {kind} subcommand for restart"));
                }
            }
            positionals += 1;
            let limit = if matches!(kind, "cursor" | "pi") {
                usize::MAX
            } else if kind == "codex" && resume {
                2
            } else {
                1
            };
            if positionals > limit {
                return Err(format!("ambiguous {kind} positional arguments for restart"));
            }
        }
    }
    Ok(cleaned)
}

fn provider_restart_args(
    kind: &str,
    grammar: &crate::agent::provider::AgentRestart,
    args: &[String],
) -> Result<Vec<String>, String> {
    let mut cleaned = Vec::new();
    let mut index = 0;
    let mut positionals = 0;
    let mut after_separator = false;
    while let Some(arg) = args.get(index) {
        index += 1;
        if arg == "--" && !after_separator {
            after_separator = true;
            if grammar.keep_positionals {
                cleaned.push(arg.clone());
            }
            continue;
        }
        if !after_separator && arg.starts_with('-') {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
            let has = |flags: &[String]| flags.iter().any(|item| item == flag);
            if has(&grammar.remove_optional_values) {
                if inline.is_none() && args.get(index).is_some_and(|next| !next.starts_with('-')) {
                    index += 1;
                }
            } else if has(&grammar.values) || has(&grammar.remove_values) {
                let keep = has(&grammar.values);
                if keep {
                    cleaned.push(arg.clone());
                }
                if inline.is_none() {
                    let value = args
                        .get(index)
                        .ok_or_else(|| format!("missing value for {flag}"))?;
                    if keep {
                        cleaned.push(value.clone());
                    }
                    index += 1;
                }
            } else if inline.is_none() && (has(&grammar.switches) || has(&grammar.remove_switches))
            {
                if has(&grammar.switches) {
                    cleaned.push(arg.clone());
                }
            } else if inline.is_some() && has(&grammar.inline_values) {
                cleaned.push(arg.clone());
            } else {
                return Err(format!(
                    "unsupported or ambiguous {kind} option {flag} for restart"
                ));
            }
        } else {
            if positionals == 0 && !after_separator && grammar.forbidden_commands.contains(arg) {
                return Err(format!("unsupported {kind} subcommand for restart"));
            }
            positionals += 1;
            if positionals > grammar.max_positionals {
                return Err(format!("ambiguous {kind} positional arguments for restart"));
            }
            if grammar.keep_positionals {
                cleaned.push(arg.clone());
            }
        }
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::launch::{plan, Spec};

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
                "--model", "sonnet", "-e", "user.ts", "--models", "a,b", "-t", "read", "-xt",
                "bash", "-n", "name", "--mode", "text", "-ne"
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
}
