//! Starting a coding agent: what to run, and what Git setup to do first.
//!
//! Two callers ask for the same thing by two different routes — the `hocus-new-agent`
//! modal, where a person fills in a form, and hocus's `agent_spawn` tool, where a model
//! fills in a JSON object. Both mean "start `claude` on a fresh worktree off `main`", and
//! neither has any business deciding separately what that means: a default base branch
//! that drifted, or a `cursor` that ran `cursor` on one path and `cursor-agent` on the
//! other, would be a difference nobody asked for and nobody would find until it bit.
//!
//! So the decision lives here, once. [`Spec`] is the request in terms both callers can
//! fill in, [`plan`] turns it into the pair Zellij wants, and [`open`] is the only place
//! in the tree that calls the host to do it. A caller's job is to build a `Spec` and hand
//! it over; everything after that is the same code for both.
//!
//! [`plan`] is pure and tests on the host triple. [`open`] cannot be — it is a host call —
//! so it is gated the way [`crate::store::Host`] is, with a stub on the host triple that
//! fails loudly rather than quietly answering nothing.

use std::collections::BTreeMap;
use zellij_tile::prelude::{
    validate_firecracker_vm_name, CommandToRun, GitCheckout, PaneId, HOCUS_AGENT_PANE_CONTEXT,
};

/// The branch a new one is cut from when the caller does not say.
///
/// A default rather than a probe of the repository's own HEAD: the modal has to
/// show *something* in the field before it knows which project is selected, and a
/// value that changed as you moved the project select would be worse than one you
/// can read and correct.
pub const DEFAULT_BASE: &str = "main";

/// Where worktrees go, relative to the repository's primary checkout.
pub const WORKTREE_PREFIX: &str = ".worktrees/";

/// The agents a chooser offers, in the order it should offer them.
///
/// Not every kind [`crate::agents`] can *recognise* — that list is what detection
/// knows about, which is deliberately wider than what we will start on request. This
/// one is shared so the modal's select and the tool's schema enum cannot come to
/// disagree about which agents you may ask for.
pub const OFFERED: &[&str] = &["codex", "claude", "cursor"];

/// Everything it takes to start an agent, in terms both callers have.
///
/// Every field is optional because both callers have partial inputs — a form with
/// blank fields, a tool call with omitted keys — and the defaults belong in [`plan`]
/// where both get them, not in a form's initial values where only one would.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The agent to run, by kind: `claude`, `codex`, `cursor`, or any other name
    /// [`crate::agents`] would recognise. Resolved to a program by [`binary_for`].
    pub kind: Option<String>,
    /// A program to run instead, for whatever no kind names. Ignored when `kind` is
    /// set; one of the two is required.
    pub command: Option<String>,
    /// Arguments before the ones this module adds.
    pub args: Vec<String>,
    /// Passed as `--model <model>`, the one flag every agent here spells the same.
    pub model: Option<String>,
    /// An opening prompt, passed as the last positional argument — which is how
    /// `claude`, `codex` and `cursor-agent` all take one.
    pub prompt: Option<String>,
    /// The branch to create. Absent means no Git work at all: run where we are.
    pub branch: Option<String>,
    /// What `branch` is cut from. Defaults to [`DEFAULT_BASE`].
    pub base: Option<String>,
    /// Whether `branch` lives in a new worktree rather than in this checkout.
    /// Defaults to *true* once a branch is named, which is the safer of the two:
    /// a worktree leaves the checkout the caller is looking at alone.
    pub worktree: Option<bool>,
    /// Where that worktree goes, relative to the repository's primary checkout.
    /// Defaults to [`worktree_path_for`] the branch.
    pub worktree_path: Option<String>,
    /// Environment the agent starts with, on top of the pane's own. Carried by an `env(1)`
    /// wrapper in the command - `env K=V claude …` - because a command pane has no
    /// environment of its own to set; `env` execs the agent, so the pane still runs it.
    pub env: BTreeMap<String, String>,
    /// How much the agent may do without asking, in the agent's own terms - see
    /// [`shape_args`] for what each kind accepts. `skip` is understood by every kind that
    /// has a way to turn permissions off.
    pub permission_mode: Option<String>,
    /// A session to resume: an id, or `last` for the most recent.
    pub resume: Option<String>,
    /// Whether a `claude` pane reports its state through its hooks. Defaults to true; the
    /// hooks are handed to it inline, so nothing needs installing.
    pub hooks: Option<bool>,
    /// Named session-scoped Firecracker VM to run in.
    pub firecracker: Option<String>,
}

/// The permission modes `claude --permission-mode` accepts, plus `skip` for
/// `--dangerously-skip-permissions`.
pub const CLAUDE_PERMISSION_MODES: &[&str] = &[
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "manual",
    "dontAsk",
    "plan",
    "skip",
];

/// The approval policies `codex --ask-for-approval` accepts, plus `full-auto` for
/// `--full-auto` and `skip` for `--dangerously-bypass-approvals-and-sandbox`.
pub const CODEX_PERMISSION_MODES: &[&str] = &[
    "untrusted",
    "on-failure",
    "on-request",
    "never",
    "full-auto",
    "skip",
];

/// `cursor-agent` has one switch: `--force`, which `skip` and `force` both name.
pub const CURSOR_PERMISSION_MODES: &[&str] = &["skip", "force"];

/// The program a kind runs.
///
/// Identity for all but one: `cursor` ships its agent as `cursor-agent`, and the bare
/// `cursor` on the path is the editor. The mapping is here rather than at each call
/// site because there used to be two copies of it and they were one release apart from
/// disagreeing.
pub fn binary_for(kind: &str) -> &str {
    match kind {
        "cursor" => "cursor-agent",
        other => other,
    }
}

/// Where a branch's worktree goes when the caller does not say.
pub fn worktree_path_for(branch: &str) -> String {
    format!("{WORKTREE_PREFIX}{branch}")
}

/// The arguments a kind's program takes for what a [`Spec`] asks, as `(leading, flags)`:
/// `leading` goes before the caller's own arguments (a `resume` subcommand), `flags` after
/// them and before the prompt.
///
/// One table, verified against each program's `--help`. A mode a kind does not have is an
/// error naming the ones it does, never silently dropped: a caller who asked for permissions
/// off and got an agent that asks would not find out until it stalled.
pub fn shape_args(kind: Option<&str>, spec: &Spec) -> Result<(Vec<String>, Vec<String>), String> {
    let mut leading = Vec::new();
    let mut flags = Vec::new();
    if let Some(model) = trimmed(&spec.model) {
        flags.push("--model".to_owned());
        flags.push(model.to_owned());
    }
    let mode = trimmed(&spec.permission_mode);
    let resume = trimmed(&spec.resume);
    let firecracker = trimmed(&spec.firecracker).is_some();
    if firecracker && spec.hooks == Some(true) {
        return Err("agent-report hooks are unavailable in Firecracker VMs; omit hooks or set hooks to false".to_owned());
    }
    let hooks = spec.hooks.unwrap_or(!firecracker);
    let unknown_mode = |accepted: &[&str], mode: &str| {
        Err(format!(
            "{kind} has no permission mode {mode:?}; it accepts: {}",
            accepted.join(", "),
            kind = kind.unwrap_or("this command"),
        ))
    };
    match kind {
        Some("claude") => {
            match mode {
                None => {},
                Some("skip") => flags.push("--dangerously-skip-permissions".to_owned()),
                Some(mode) if CLAUDE_PERMISSION_MODES.contains(&mode) => {
                    flags.push("--permission-mode".to_owned());
                    flags.push(mode.to_owned());
                },
                Some(mode) => return unknown_mode(CLAUDE_PERMISSION_MODES, mode),
            }
            match resume {
                None => {},
                Some("last") => flags.push("--continue".to_owned()),
                Some(id) => {
                    flags.push("--resume".to_owned());
                    flags.push(id.to_owned());
                },
            }
            if hooks {
                // The hooks that make the pane report its own state, handed over whole:
                // no script, no settings file, and nothing to install first.
                flags.push("--settings".to_owned());
                flags.push(claude_hook_settings_argument());
            }
        },
        Some("codex") => {
            match mode {
                None => {},
                Some("full-auto") => flags.push("--full-auto".to_owned()),
                Some("skip") => flags.push("--dangerously-bypass-approvals-and-sandbox".to_owned()),
                Some(mode) if CODEX_PERMISSION_MODES.contains(&mode) => {
                    flags.push("--ask-for-approval".to_owned());
                    flags.push(mode.to_owned());
                },
                Some(mode) => return unknown_mode(CODEX_PERMISSION_MODES, mode),
            }
            // A subcommand, so it goes first: `codex resume [OPTIONS] [SESSION_ID] [PROMPT]`.
            match resume {
                None => {},
                Some("last") => leading.extend(["resume".to_owned(), "--last".to_owned()]),
                Some(id) => leading.extend(["resume".to_owned(), id.to_owned()]),
            }
        },
        Some("cursor") => {
            match mode {
                None => {},
                Some("skip" | "force") => flags.push("--force".to_owned()),
                Some(mode) => return unknown_mode(CURSOR_PERMISSION_MODES, mode),
            }
            match resume {
                None => {},
                Some("last") => leading.push("resume".to_owned()),
                Some(id) => {
                    flags.push("--resume".to_owned());
                    flags.push(id.to_owned());
                },
            }
        },
        _ => {
            // A kind nothing here knows, or a bare command: `--model` is the one flag every
            // agent spells the same, and nothing else can be guessed at.
            if let Some(mode) = mode {
                return unknown_mode(&[], mode);
            }
            if resume.is_some() {
                return Err(format!(
                    "{} has no resume spelling known here; pass the flag in args",
                    kind.unwrap_or("this command")
                ));
            }
        },
    }
    Ok((leading, flags))
}

/// The `--settings` value a spawned `claude` is handed: the hook wiring from
/// `zellij_utils::agent`, as one JSON argument.
pub fn claude_hook_settings_argument() -> String {
    zellij_tile::agent::claude_hook_settings(zellij_tile::agent::CLAUDE_HOOK_REPORT_COMMAND)
        .to_string()
}

/// What to run, and what to do to the repository before running it.
///
/// The error strings are the ones shown to whoever asked — the modal puts them in its
/// status row, hocus hands them back to the model as the tool result — so they say what
/// is missing rather than that something is.
pub fn plan(spec: &Spec) -> Result<(CommandToRun, GitCheckout), String> {
    let firecracker = trimmed(&spec.firecracker).map(str::to_owned);
    if let Some(name) = firecracker.as_deref() {
        validate_firecracker_vm_name(name)?;
        if trimmed(&spec.branch).is_some() || spec.worktree == Some(true) {
            return Err("Firecracker agents cannot create host branches or worktrees".to_owned());
        }
    }
    let kind = trimmed(&spec.kind).map(str::to_lowercase);
    let program = match (kind.as_deref(), trimmed(&spec.command)) {
        (Some(kind), _) => binary_for(kind).to_owned(),
        (None, Some(command)) => command.to_owned(),
        (None, None) => return Err("an agent or a command is required".to_owned()),
    };

    let (leading, flags) = shape_args(kind.as_deref(), spec)?;
    let mut args = leading;
    args.extend(spec.args.iter().cloned());
    args.extend(flags);
    // Last, and after every flag: a positional argument that came before a flag would
    // be read by some of these programs as the flag's value.
    if let Some(prompt) = trimmed(&spec.prompt) {
        args.push(prompt.to_owned());
    }

    // An environment is an `env(1)` wrapper: assignments first, then the program and its
    // arguments. `env` execs, so the pane's foreground process is still the agent, and the
    // session names the record for it rather than for `env` (`agent_kind_of`).
    let (path, args) = if spec.env.is_empty() {
        (program, args)
    } else {
        let mut wrapped: Vec<String> = spec
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        wrapped.push(program);
        wrapped.extend(args);
        ("env".to_owned(), wrapped)
    };

    let command = CommandToRun {
        path: path.into(),
        args,
        firecracker_vm: firecracker,
        ..Default::default()
    };

    let Some(branch) = trimmed(&spec.branch) else {
        // A worktree asked for with nothing to put in it. Saying so beats silently
        // running in the current checkout, which is the one thing the caller said
        // they did not want.
        if spec.worktree == Some(true) {
            return Err("a branch is required to create a worktree".to_owned());
        }
        return Ok((command, GitCheckout::Current));
    };

    let base = trimmed(&spec.base).unwrap_or(DEFAULT_BASE);
    if !spec.worktree.unwrap_or(true) {
        return Ok((
            command,
            GitCheckout::NewBranch {
                branch: branch.to_owned(),
                base: base.to_owned(),
            },
        ));
    }
    let path = trimmed(&spec.worktree_path)
        .map(str::to_owned)
        .unwrap_or_else(|| worktree_path_for(branch));
    Ok((
        command,
        GitCheckout::NewWorktree {
            branch: branch.to_owned(),
            base: base.to_owned(),
            path: path.into(),
        },
    ))
}

/// A field the caller filled in, or `None` for one they left blank.
///
/// Whitespace-only counts as blank: a form field the user tabbed through and a JSON
/// key the model sent as `""` both mean "I did not say", and neither should reach Git
/// as a branch name.
fn trimmed(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

// --------------------------------------------------------------- wasm: the host call

/// Do the Git setup and open the pane. The one call site of the host API that does it.
#[cfg(target_family = "wasm")]
pub fn open(spec: &Spec, source_directory: std::path::PathBuf) -> Result<PaneId, String> {
    let (command, checkout) = plan(spec)?;
    if command.firecracker_vm.is_some() {
        return zellij_tile::shim::open_command_pane_near_plugin(command, agent_pane_context())
            .ok_or_else(|| "zellij did not open the Firecracker agent pane".to_owned());
    }
    zellij_tile::shim::open_command_pane_in_git_checkout_near_plugin(
        command,
        source_directory,
        checkout,
    )
}

/// [`open`] on the host triple, where there is no host to call.
///
/// Both callers are compiled natively to run their tests, so this has to exist on
/// both sides. It fails rather than pretending, for the same reason
/// [`crate::store::Host`]'s stub does: a test that reaches here has left the pure
/// half, and should say so instead of passing against a pane that was never opened.
#[cfg(not(target_family = "wasm"))]
pub fn open(_spec: &Spec, _source_directory: std::path::PathBuf) -> Result<PaneId, String> {
    Err("agents cannot be started outside zellij".to_owned())
}

/// The plugin alias for each of the two hocus panes.
///
/// Aliases rather than `zellij:` urls: plugin identity is (location, configuration), an alias
/// contributes its own block - `shared true` - to that configuration, and the keybindings open
/// both halves by alias. A second spelling would open a second pane rather than reach the one
/// already there.
/// The context that marks a command pane as an agent's.
///
/// The host reads this off the pane's originating plugin when the command exits, and it is
/// what tells `agent_exit` apart from an ordinary command pane holding on its exit code.
/// One spelling, here, because every route that starts an agent has to carry it: the two
/// host calls that set it themselves, and `agent_spawn`'s no-checkout route, which opens an
/// ordinary command pane and so has to say so.
pub fn agent_pane_context() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(HOCUS_AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())])
}

pub const HOCUS_ALIAS: &str = "hocus";
pub const HOCUS_AGENTS_ALIAS: &str = "hocus-agents";

/// The configuration that makes a launched hocus pane an overlay.
///
/// One definition for both halves and both directions. `LaunchOrFocusPlugin { overlay true;
/// awaits_placement true }` writes exactly these keys, so a launch from a plugin has to match them
/// character for character or the keybinding and the plugin address two different instances - and
/// the failure looks like a duplicate pane rather than an error.
///
/// `awaits_placement` is here for that reason and not because a plugin-opened half needs it any
/// less: it says "do not paint me at the default geometry I was born with, my owner will place
/// me", which is what keeps a launched panel from flashing in the middle of the screen on top of
/// its other half. Both halves place themselves through hocus's `apply_overlay_geometry`, which
/// is what releases it.
pub fn overlay_configuration() -> std::collections::BTreeMap<String, String> {
    let mut configuration = std::collections::BTreeMap::new();
    configuration.insert("overlay".to_owned(), "true".to_owned());
    configuration.insert("awaits_placement".to_owned(), "true".to_owned());
    configuration
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec(kind: &str) -> Spec {
        Spec {
            kind: Some(kind.to_owned()),
            ..Default::default()
        }
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
        assert_eq!(command.path, PathBuf::from("claude"));
        assert!(command.args.is_empty());
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
    fn firecracker_agents_skip_host_git_and_hooks_but_keep_explicit_environment() {
        let (command, checkout) = plan(&Spec {
            firecracker: Some("review".to_owned()),
            env: BTreeMap::from([("TOKEN".to_owned(), "explicit".to_owned())]),
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(checkout, GitCheckout::Current);
        assert_eq!(command.firecracker_vm.as_deref(), Some("review"));
        assert_eq!(command.path, PathBuf::from("env"));
        assert_eq!(command.args[0], "TOKEN=explicit");
        assert!(!command.args.iter().any(|arg| arg == "--settings"));
    }

    #[test]
    fn a_blank_firecracker_field_keeps_local_agent_defaults() {
        let (command, _) = plan(&Spec {
            firecracker: Some("  ".to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(command.firecracker_vm, None);
        assert!(command.args.iter().any(|arg| arg == "--settings"));
    }

    #[test]
    fn firecracker_agents_reject_host_git_and_agent_report_hooks() {
        for invalid in [
            Spec {
                firecracker: Some("review".to_owned()),
                branch: Some("topic".to_owned()),
                ..spec("codex")
            },
            Spec {
                firecracker: Some("review".to_owned()),
                worktree: Some(true),
                ..spec("codex")
            },
            Spec {
                firecracker: Some("review".to_owned()),
                hooks: Some(true),
                ..spec("claude")
            },
        ] {
            assert!(plan(&invalid).is_err());
        }
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

    /// The one place a worktree path is *not* derived: the caller said where.
    #[test]
    fn a_given_worktree_path_is_used_as_it_stands() {
        let (_, checkout) = plan(&Spec {
            branch: Some("topic".to_owned()),
            worktree_path: Some("/elsewhere/topic".to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert!(matches!(
            checkout,
            GitCheckout::NewWorktree { path, .. } if path == *"/elsewhere/topic"
        ));
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
        for kind in ["claude", "codex", "gemini", "aider"] {
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
    /// and lists exactly the events `zellij hocus hooks install` would wire.
    #[test]
    fn claude_spawns_carry_the_hook_settings() {
        let (command, _) = plan(&spec("claude")).unwrap();
        assert_eq!(command.args[0], "--settings");
        let settings: serde_json::Value = serde_json::from_str(&command.args[1]).unwrap();
        let hooks = settings["hooks"].as_object().unwrap();
        for event in zellij_tile::agent::CLAUDE_HOOK_EVENTS {
            assert_eq!(
                hooks[*event][0]["hooks"][0]["command"],
                zellij_tile::agent::CLAUDE_HOOK_REPORT_COMMAND,
                "{event}"
            );
        }
        // Opted out, and the other kinds never had them.
        let (command, _) = plan(&Spec {
            hooks: Some(false),
            ..spec("claude")
        })
        .unwrap();
        assert!(command.args.is_empty());
        let (command, _) = plan(&spec("codex")).unwrap();
        assert!(command.args.is_empty());
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
            vec!["ANTHROPIC_MODEL=opus", "FOO=a=b", "claude", "go"]
        );
        assert_eq!(
            zellij_tile::agent::agent_kind_of(&command.path, &command.args).as_deref(),
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
            vec!["--dangerously-skip-permissions", "--continue"]
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
            ..spec("codex")
        })
        .unwrap();
        assert_eq!(
            command.args,
            vec![
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
            ..spec("codex")
        })
        .unwrap();
        assert_eq!(command.args, vec!["resume", "0192-abc", "--full-auto"]);
        let (command, _) = plan(&Spec {
            permission_mode: Some("skip".to_owned()),
            ..spec("codex")
        })
        .unwrap();
        assert_eq!(
            command.args,
            vec!["--dangerously-bypass-approvals-and-sandbox"]
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
        assert!(command.args.is_empty());
        assert_eq!(checkout, GitCheckout::Current);
    }
}
