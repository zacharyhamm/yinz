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
    if matches!(name, Some("yinz" | "sh"))
        && let Some((inner, inner_args)) = crate::agent::unwrapped_program_and_args(program, args)
        && inner != program
    {
        let mut result = args[..args.len() - inner_args.len()].to_vec();
        let managed_codex = managed_codex
            || (name == Some("yinz") && args.get(1).is_some_and(|launcher| launcher == "codex"));
        let (cleaned, separator) = clean_restart_args(inner, inner_args, resuming, managed_codex)?;
        let separator = separator.map(|index| result.len() + index);
        result.extend(cleaned);
        return Ok((result, separator));
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
#[path = "unit/restart.rs"]
mod tests;
