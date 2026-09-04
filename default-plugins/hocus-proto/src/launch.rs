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

use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use zellij_tile::prelude::{
    CommandToRun, GitCheckout, PaneId, PaneTarget, HOCUS_AGENT_PANE_CONTEXT,
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
    /// Immutable project image selected for the Firecracker VM.
    pub firecracker_image: Option<String>,
}

/// A VM used by exactly one automatic agent when no worktree will be mirrored.
pub fn unique_firecracker_vm() -> String {
    thread_local! {
        static SEQUENCE: Cell<u64> = const { Cell::new(0) };
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = SEQUENCE.with(|sequence| {
        let next = sequence.get().wrapping_add(1);
        sequence.set(next);
        next
    });
    format!("hocus-agent-{now:x}-{sequence:x}")
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
    // A Firecracker pane's hooks are installed in the guest when the VM is provisioned, so
    // nothing is handed to the agent here. Claude Code *merges* hooks across settings sources
    // rather than letting one override another, so a second copy inline would fire every
    // event twice - and it would carry the local `agent-report` command, which cannot reach
    // a session from inside a VM anyway.
    let firecracker = trimmed(&spec.firecracker).is_some();
    let hooks = spec.hooks.unwrap_or(true) && !firecracker;
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

/// The spec's environment, plus what a Firecracker guest needs to run the agent it was
/// asked for.
///
/// A guest logs in as root, and `claude --dangerously-skip-permissions` refuses to run as
/// root. The refusal stands in for "you have no sandbox", and here the VM *is* the sandbox -
/// which is the whole reason the pane was asked for - so the pane would otherwise die on a
/// check that has already been satisfied. `IS_SANDBOX` is how Claude Code is told so.
///
/// The flag itself is what is looked for, in the arguments as they will be run. A
/// `permission_mode` of `skip` is only one of the two ways to ask for it - `shape_args` turns
/// that mode into this flag - and the other, passing it in `args`, is just as supported and
/// dies on the same check.
///
/// A caller that sets the variable itself keeps its value. Nothing is added for a local
/// pane, which runs as the user and needs none of this.
fn guest_environment(spec: &Spec, kind: Option<&str>, args: &[String]) -> BTreeMap<String, String> {
    let mut env = spec.env.clone();
    let firecracker = trimmed(&spec.firecracker).is_some();
    let skipping = args
        .iter()
        .any(|arg| arg == "--dangerously-skip-permissions");
    if firecracker && kind == Some("claude") && skipping {
        env.entry("IS_SANDBOX".to_owned())
            .or_insert_with(|| "1".to_owned());
    }
    env
}

/// What to run, and what to do to the repository before running it.
///
/// The error strings are the ones shown to whoever asked — the modal puts them in its
/// status row, hocus hands them back to the model as the tool result — so they say what
/// is missing rather than that something is.
pub fn plan(spec: &Spec) -> Result<(CommandToRun, GitCheckout), String> {
    let target = PaneTarget::from_parts(
        None,
        trimmed(&spec.firecracker),
        trimmed(&spec.firecracker_image),
        None,
    )?;
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
    // Settled from the flags alone, before the prompt joins them: a prompt whose text reads
    // like a flag is still a prompt, and what the agent was asked to do says nothing about
    // how it was asked to run.
    let env = guest_environment(spec, kind.as_deref(), &args);
    // Last, and after every flag: a positional argument that came before a flag would
    // be read by some of these programs as the flag's value.
    if let Some(prompt) = trimmed(&spec.prompt) {
        args.push(prompt.to_owned());
    }

    // An environment is an `env(1)` wrapper: assignments first, then the program and its
    // arguments. `env` execs, so the pane's foreground process is still the agent, and the
    // session names the record for it rather than for `env` (`agent_kind_of`).
    //
    // Everything here is visible in `ps` and in the pane's title, so it carries settings,
    // never secrets. A credential reaches a Firecracker guest as a file, through the
    // provider's `credential_files`.
    let (path, args) = if env.is_empty() {
        (program, args)
    } else {
        let mut wrapped: Vec<String> = env
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
        target,
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

/// What resuming a recorded run means: the same agent, the same session, the folder the
/// run was in.
///
/// No Git work, which is the one thing that makes this different from every other launch
/// here. A run being resumed already has its checkout — the worktree it was cut in is
/// still there, and its path is what the run recorded as its `cwd`. So this stops at a
/// [`CommandToRun`] rather than going through [`open`], whose host call resolves a
/// repository root and would refuse a run recorded outside one.
///
/// `session_id` is the agent's own, as `AgentRun` stores it. `host` is the machine the
/// `cwd` is on, absent for this one, in the spelling `PaneTarget::parse_display` reads.
///
/// Every refusal here is written to be read on a divider one row high, so each says what is
/// missing in as few characters as it can and leaves the reasoning to these comments.
pub fn resume_command(
    kind: &str,
    session_id: &str,
    cwd: Option<&str>,
    host: Option<&str>,
) -> Result<CommandToRun, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        // A run recorded by a hook script that reported no session id of its own: its id
        // is the `kind:pane:<n>:<started at>` fallback, and there is nothing in it to hand
        // to `--resume`.
        return Err("this run recorded no session id to resume".to_owned());
    }
    let target = PaneTarget::parse_display(host);
    if let PaneTarget::Firecracker { vm, .. } = &target {
        // A run records the VM's name inside `host` but neither the image it was built
        // from nor the guest worktree it was mirrored into, and a session-scoped VM is
        // gone by the time the run reaches the archive. Saying so beats opening a pane
        // that cannot start. Short, because where this is read is a divider one row high:
        // the whole of the reasoning is in this comment and in `docs/AGENT_HOOKS.md`.
        return Err(format!(
            "the Firecracker VM {vm} cannot be rebuilt from what the run recorded"
        ));
    }
    let cwd = cwd
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(std::path::PathBuf::from);
    // The mounts a plugin's own paths are read through. The host rewrites the cwd of a local
    // command that starts with one of them (`translate_plugin_path`, in
    // zellij-server/src/plugins/zellij_exports.rs), and a run's cwd is a real host path taken
    // from the agent's hook rather than a plugin path: a run recorded in `/tmp/scratch` would
    // resume in `$ZELLIJ_TMP_DIR/scratch`, which is the wrong folder rather than a refusal.
    // A remote cwd is the far machine's and is passed through as it stands, so this is the
    // local case only - and the list has to stay in step with the one over there.
    if matches!(target, PaneTarget::Local) {
        if let Some(mount) = cwd.as_ref().and_then(|cwd| {
            ["/host", "/data", "/cache", "/tmp"]
                .into_iter()
                .find(|mount| cwd.starts_with(mount))
        }) {
            return Err(format!(
                "hocus cannot open a pane under {mount}: the plugin mounts claim it"
            ));
        }
    }
    let spec = Spec {
        kind: Some(kind.to_owned()),
        resume: Some(session_id.to_owned()),
        ..Default::default()
    };
    // The checkout is `Current` by construction - no branch is named - and it is the pane's
    // `cwd` below that says where "current" is, not this.
    let (command, _) = plan(&spec)?;
    Ok(CommandToRun {
        cwd,
        target,
        ..command
    })
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
    zellij_tile::shim::open_command_pane_in_git_checkout_near_plugin_with_automatic_firecracker_vm(
        command,
        source_directory,
        checkout,
        spec.firecracker.is_some(),
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

    /// The VM's own install is the only one: the provider wires both agents' hooks in the
    /// guest at provisioning, and Claude Code merges hooks across settings sources rather
    /// than overriding, so a second copy handed in here would fire every event twice.
    #[test]
    fn firecracker_agents_leave_the_hooks_to_the_guest_and_keep_explicit_environment() {
        let (command, checkout) = plan(&Spec {
            firecracker: Some("review".to_owned()),
            env: BTreeMap::from([("TOKEN".to_owned(), "explicit".to_owned())]),
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(checkout, GitCheckout::Current);
        assert_eq!(command.target.firecracker_vm(), Some("review"));
        assert_eq!(command.path, PathBuf::from("env"));
        assert_eq!(command.args[0], "TOKEN=explicit");
        assert!(
            !command.args.iter().any(|arg| arg == "--settings"),
            "{:?}",
            command.args
        );
    }

    #[test]
    fn a_blank_firecracker_field_keeps_local_agent_defaults() {
        let (command, _) = plan(&Spec {
            firecracker: Some("  ".to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(command.target.firecracker_vm(), None);
        assert!(command.args.iter().any(|arg| arg == "--settings"));
    }

    #[test]
    fn a_firecracker_image_requires_a_vm() {
        assert!(plan(&Spec {
            firecracker_image: Some("rust".to_owned()),
            ..spec("codex")
        })
        .is_err());
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

    /// A guest logs in as root, where `--dangerously-skip-permissions` refuses to run. The
    /// VM is the sandbox the flag is asking after, so the pane says so rather than dying on
    /// a check it has already passed.
    #[test]
    fn a_firecracker_claude_skipping_permissions_declares_its_sandbox() {
        let (command, _) = plan(&Spec {
            firecracker: Some("review".to_owned()),
            permission_mode: Some("skip".to_owned()),
            ..without_hooks("claude")
        })
        .unwrap();
        assert_eq!(command.path, PathBuf::from("env"));
        assert_eq!(
            command.args,
            vec!["IS_SANDBOX=1", "claude", "--dangerously-skip-permissions"]
        );
        assert_eq!(
            zellij_tile::agent::agent_kind_of(&command.path, &command.args).as_deref(),
            Some("claude"),
        );
    }

    /// The other way to ask for the flag. `permission_mode` is not the condition - the flag
    /// is - and a caller that spells it out in `args` is on the same root check.
    #[test]
    fn a_flag_passed_in_args_declares_the_sandbox_too() {
        let (command, _) = plan(&Spec {
            firecracker: Some("review".to_owned()),
            args: vec!["--dangerously-skip-permissions".to_owned()],
            ..without_hooks("claude")
        })
        .unwrap();
        assert_eq!(command.path, PathBuf::from("env"));
        assert_eq!(
            command.args,
            vec!["IS_SANDBOX=1", "claude", "--dangerously-skip-permissions"]
        );
    }

    /// The prompt is what the agent was asked to do, not how it was asked to run: a prompt
    /// that reads like the flag is still a prompt.
    #[test]
    fn a_prompt_that_looks_like_the_flag_declares_nothing() {
        let (command, _) = plan(&Spec {
            firecracker: Some("review".to_owned()),
            prompt: Some("--dangerously-skip-permissions".to_owned()),
            ..without_hooks("claude")
        })
        .unwrap();
        assert_ne!(command.path, PathBuf::from("env"));
        assert!(
            !command.args.iter().any(|arg| arg.starts_with("IS_SANDBOX")),
            "{command:?}"
        );
    }

    /// Nothing is added anywhere else: a local pane runs as the user and needs none of it,
    /// a milder permission mode was never refused, and another agent's root check is not
    /// this one's to answer - however it was asked for.
    #[test]
    fn nothing_else_declares_a_sandbox() {
        for spec in [
            Spec {
                permission_mode: Some("skip".to_owned()),
                ..without_hooks("claude")
            },
            Spec {
                args: vec!["--dangerously-skip-permissions".to_owned()],
                ..without_hooks("claude")
            },
            Spec {
                firecracker: Some("review".to_owned()),
                permission_mode: Some("acceptEdits".to_owned()),
                ..without_hooks("claude")
            },
            Spec {
                firecracker: Some("review".to_owned()),
                permission_mode: Some("skip".to_owned()),
                ..without_hooks("codex")
            },
            Spec {
                firecracker: Some("review".to_owned()),
                args: vec!["--dangerously-skip-permissions".to_owned()],
                ..without_hooks("codex")
            },
        ] {
            let (command, _) = plan(&spec).unwrap();
            assert_ne!(command.path, PathBuf::from("env"), "{spec:?}");
            assert!(
                !command.args.iter().any(|arg| arg.starts_with("IS_SANDBOX")),
                "{command:?}"
            );
        }
    }

    /// A caller who set the variable meant it.
    #[test]
    fn an_explicit_sandbox_value_is_kept() {
        let (command, _) = plan(&Spec {
            firecracker: Some("review".to_owned()),
            permission_mode: Some("skip".to_owned()),
            env: BTreeMap::from([("IS_SANDBOX".to_owned(), "0".to_owned())]),
            ..without_hooks("claude")
        })
        .unwrap();
        assert_eq!(
            command.args.first().map(String::as_str),
            Some("IS_SANDBOX=0")
        );
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
        )
        .unwrap();
        assert_eq!(command.path, PathBuf::from("claude"));
        assert_eq!(
            command.args,
            vec![
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
        let command = resume_command("codex", "0192-abc", Some("/w"), None).unwrap();
        assert_eq!(command.path, PathBuf::from("codex"));
        assert_eq!(command.args, vec!["resume", "0192-abc"]);
        let command = resume_command("cursor", "chat-7", None, None).unwrap();
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
        let refused = resume_command("claude", "", Some("/w"), None).unwrap_err();
        assert!(refused.contains("no session id"), "{refused}");
        // Whitespace is blank, the way `trimmed` reads every other field.
        assert!(resume_command("claude", "   ", None, None).is_err());
    }

    /// The kinds with no resume spelling are refused by `shape_args`, which this goes
    /// through - so there is one list of what can be resumed, not two.
    #[test]
    fn a_kind_with_no_resume_spelling_is_refused() {
        let refused = resume_command("gemini", "abc", None, None).unwrap_err();
        assert!(refused.contains("resume spelling"), "{refused}");
    }

    /// A run on another machine keeps its target, so the pane opens where the folder is.
    /// A Firecracker one cannot: the run records the VM's name and neither the image it was
    /// built from nor the guest worktree, and a session-scoped VM is gone by the time the
    /// run reaches the archive.
    #[test]
    fn an_ssh_run_keeps_its_host_and_a_firecracker_one_is_refused() {
        let command = resume_command("claude", "abc", Some("/srv/w"), Some("build-box")).unwrap();
        assert_eq!(
            command.target,
            PaneTarget::Ssh {
                host: "build-box".to_owned()
            }
        );
        assert_eq!(command.cwd, Some(PathBuf::from("/srv/w")));
        let refused =
            resume_command("claude", "abc", Some("/w"), Some("firecracker:vm-1")).unwrap_err();
        assert!(refused.contains("vm-1"), "{refused}");
        assert!(refused.contains("Firecracker"), "{refused}");
    }

    /// The host reads a local command's cwd through the plugin mounts, so a run recorded in
    /// one of the four folders they claim would open its pane somewhere else entirely - in
    /// `$ZELLIJ_TMP_DIR` rather than `/tmp`. A refusal says so; the silent version does not.
    #[test]
    fn a_local_folder_the_plugin_mounts_claim_is_refused() {
        for cwd in ["/tmp/scratch-repo", "/host/w", "/data/w", "/cache/w"] {
            let refused = resume_command("claude", "abc", Some(cwd), None).unwrap_err();
            assert!(refused.contains("plugin mounts"), "{cwd}: {refused}");
        }
        // Only those four, and only on their own component boundaries.
        assert!(resume_command("claude", "abc", Some("/tmpfoo/w"), None).is_ok());
        assert!(resume_command("claude", "abc", Some("/home/u/proj"), None).is_ok());
        // And not remotely: a remote cwd is a path on the far machine, which the host passes
        // through untouched.
        let command = resume_command(
            "claude",
            "abc",
            Some("/tmp/scratch-repo"),
            Some("build-box"),
        )
        .unwrap();
        assert_eq!(command.cwd, Some(PathBuf::from("/tmp/scratch-repo")));
    }
}
