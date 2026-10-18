use std::path::Path;

/// Replace the prompt and session selectors inside the original command's wrappers.
pub fn restart_with_prompt(
    program: &Path,
    args: &[String],
    session_id: Option<&str>,
    prompt: Option<&str>,
) -> Result<Vec<String>, String> {
    // Use one provider revision for cleaning, defaults, resume and prompt grammar.
    crate::agent::provider::with_snapshot(&crate::agent::provider::providers(), || {
        use crate::agent::launch::{prompt_args, shape_args, Spec};
        let resuming = session_id.is_some_and(|id| !id.trim().is_empty());
        let (cleaned, separator) = clean_restart_args(program, args, resuming, false)?;
        let mut inner_program = program;
        let mut inner_args = cleaned.as_slice();
        loop {
            let (next_program, next_args) =
                crate::agent::unwrapped_program_and_args(inner_program, inner_args)
                    .ok_or("unsupported agent wrapper for restart")?;
            if next_program == inner_program && next_args.len() == inner_args.len() {
                break;
            }
            inner_program = next_program;
            inner_args = next_args;
        }
        let kind = crate::agent::agent_kind_of(inner_program, inner_args)
            .ok_or("unsupported agent command for restart")?;
        let spec = Spec {
            kind: Some(kind.clone()),
            resume: session_id.map(str::to_owned),
            prompt: prompt.map(str::to_owned),
            hooks: Some(false),
            ..Default::default()
        };
        let (mut leading, flags) = shape_args(Some(&kind), &spec)?;
        let provider = crate::agent::provider::provider(&kind);
        if let Some(provider) = &provider {
            // Launch defaults are already present in the held command.
            leading.drain(..provider.launch.args.len());
        }
        let prefix_len = cleaned.len() - inner_args.len();
        let mut result = cleaned[..prefix_len].to_vec();
        result.extend(leading);
        let separator = separator.unwrap_or(cleaned.len());
        result.extend_from_slice(&cleaned[prefix_len..separator]);
        result.extend(flags);
        result.extend(prompt_args(Some(&kind), provider.as_deref(), &spec)?);
        result.extend_from_slice(&cleaned[separator..]);
        Ok(result)
    })
}

/// Remove an agent's opening prompt and conversation selectors without changing its launch
/// options. Unknown option arities and non-interactive commands must be refused: guessing
/// could replay work or silently change the model, permissions, or working directory.
pub fn restart_args(program: &Path, args: &[String]) -> Result<Vec<String>, String> {
    clean_restart_args(program, args, false, false).map(|(args, _)| args)
}

// The separator index names a retained delimiter, never an option value equal to `--`.
fn clean_restart_args(
    program: &Path,
    args: &[String],
    resuming: bool,
    managed_codex: bool,
) -> Result<(Vec<String>, Option<usize>), String> {
    let name = program.file_name().and_then(|name| name.to_str());
    if matches!(name, Some("yinz" | "sh")) {
        if let Some((inner, inner_args)) = crate::agent::unwrapped_program_and_args(program, args) {
            if inner != program {
                let mut result = args[..args.len() - inner_args.len()].to_vec();
                let managed_codex = managed_codex
                    || (name == Some("yinz")
                        && args.get(1).is_some_and(|launcher| launcher == "codex"));
                let (cleaned, separator) =
                    clean_restart_args(inner, inner_args, resuming, managed_codex)?;
                let separator = separator.map(|index| result.len() + index);
                result.extend(cleaned);
                return Ok((result, separator));
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
        let (inner_args, separator) = clean_restart_args(
            Path::new(program),
            &args[index + 1..],
            resuming,
            managed_codex,
        )?;
        let separator = separator.map(|index| cleaned.len() + index);
        cleaned.extend(inner_args);
        return Ok((cleaned, separator));
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
    let managed_codex =
        managed_codex && kind == "codex" && super::codex::bridge_options(args).is_some();
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
            if managed_codex
                && resuming
                && matches!(
                    flag,
                    "--full-auto"
                        | "--dangerously-bypass-approvals-and-sandbox"
                        | "--ask-for-approval"
                        | "-a"
                        | "--sandbox"
                        | "-s"
                )
            {
                return Err(format!(
                    "Yinz's managed Codex bridge cannot resume with {flag}; start a fresh conversation to preserve the requested permissions"
                ));
            }
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
    Ok((cleaned, None))
}

fn provider_restart_args(
    kind: &str,
    grammar: &crate::agent::provider::AgentRestart,
    args: &[String],
) -> Result<(Vec<String>, Option<usize>), String> {
    let mut cleaned = Vec::new();
    let mut index = 0;
    let mut positionals = 0;
    let mut after_separator = false;
    let mut separator = None;
    while let Some(arg) = args.get(index) {
        index += 1;
        if arg == "--" && !after_separator {
            after_separator = true;
            if grammar.keep_positionals {
                separator = Some(cleaned.len());
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
    Ok((cleaned, separator))
}

#[cfg(test)]
mod tests {
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
                        restart_with_prompt(Path::new(program), &original, session, Some("P"))
                            .unwrap(),
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
                restart_with_prompt(Path::new("codex"), &strings(&flags), Some("sid"), None)
                    .is_ok()
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
