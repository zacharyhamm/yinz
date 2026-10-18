//! Starting a coding agent: what to run, and what Git setup to do first.
//!
//! Three callers ask for the same thing by three different routes — the `new-agent`
//! modal, where a person fills in a form; agent's `agent_spawn` tool, where a model fills in
//! a JSON object; and a Scheme plugin's `yinz-agent-spawn`, which the server answers
//! through `PluginCommand::SpawnAgent`. All of them mean "start `claude` on a fresh worktree
//! off `main`", and none has any business deciding separately what that means: a default
//! base branch that drifted, or a `cursor` that ran `cursor` on one path and `cursor-agent`
//! on the other, would be a difference nobody asked for and nobody would find until it bit.
//!
//! So the decision lives here, once, in the crate every caller already links.
//! [`AgentSpawnSpec`] (aliased [`Spec`]) is the request in terms every caller can fill in and
//! [`plan`] turns it into the pair Yinz wants. Everything here is pure and tests on the
//! host triple; the host call that acts on the plan is the server's `spawn_agent` handler.

use std::collections::BTreeMap;

use yinz_types::{
    agent::AgentSpawnSpec,
    command::{CommandToRun, GitCheckout, PaneTarget},
};

/// The request, as every caller spells it. See [`AgentSpawnSpec`] for the fields.
pub type Spec = AgentSpawnSpec;

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
/// Not every kind agent-proto's `agents` can *recognise* — that list is what detection
/// knows about, which is deliberately wider than what we will start on request. This
/// one is shared so the modal's select and the tool's schema enum cannot come to
/// disagree about which agents you may ask for.
pub const OFFERED: &[&str] = &["codex", "claude", "pi", "cursor", "muse", "agy"];

pub fn offered() -> Vec<String> {
    OFFERED
        .iter()
        .map(|kind| (*kind).to_owned())
        .chain(
            yinz_types::agent::provider::providers()
                .into_iter()
                .map(|provider| provider.kind),
        )
        .collect()
}

/// Fresh conversation in an existing pane's location, with target-appropriate hooks.
pub fn command_in_context(
    kind: &str,
    cwd: Option<std::path::PathBuf>,
    target: PaneTarget,
) -> Result<yinz_types::command::CommandToRun, String> {
    let (command, _) = plan(&Spec {
        kind: Some(kind.to_owned()),
        hooks: Some(!target.is_remote()),
        ..Default::default()
    })?;
    Ok(yinz_types::command::CommandToRun {
        cwd,
        target,
        ..command
    })
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

/// The approval modes `muse --approval-mode` accepts, plus `skip` for `--yolo`,
/// which disables approval and sandboxing and trusts the workspace - the closest
/// analog of Codex's bypass flag. A spawned pane's workspace is the fresh worktree
/// it was cut for, so the trust `--yolo` carries stays inside that checkout.
pub const MUSE_PERMISSION_MODES: &[&str] = &["untrusted", "on-request", "never", "skip"];

/// Antigravity modes: `accept-edits`, `plan`, and `skip` for `--dangerously-skip-permissions`.
pub const AGY_PERMISSION_MODES: &[&str] = &["accept-edits", "plan", "skip"];

/// pi has no permission modes of its own: permission gating is done by pi extensions
/// the user installs, which a command line cannot speak for. The empty table is what
/// `unknown_mode` names when a caller asks for one anyway - there is no mode to accept.
pub const PI_PERMISSION_MODES: &[&str] = &[];

/// Canonicalize an agent kind alias (e.g. "antigravity" -> "agy").
pub fn normalize_kind(kind: &str) -> &str {
    match kind {
        "antigravity" => "agy",
        other => other,
    }
}

/// The reasoning efforts a kind accepts, shared by the launcher and its chooser.
pub fn effort_levels(kind: Option<&str>) -> &'static [&'static str] {
    match kind.map(normalize_kind) {
        Some("codex") => &["none", "minimal", "low", "medium", "high", "xhigh"],
        Some("claude" | "cursor") => &["low", "medium", "high", "xhigh", "max"],
        Some("muse") => &[
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ],
        Some("agy") => &["low", "medium", "high", "max"],
        // pi's own spellings, plus the two the other kinds share: `none` for what pi
        // calls `off`, and `max`, which pi accepts and codex does not.
        Some("pi") => &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
        _ => &[],
    }
}

/// Current effort choices, including plugin registrations.
pub fn effort_options(kind: &str) -> Vec<String> {
    if let Some(provider) = yinz_types::agent::provider::provider(kind) {
        return provider.launch.efforts.keys().cloned().collect();
    }
    effort_levels(Some(kind))
        .iter()
        .map(|value| (*value).to_owned())
        .collect()
}

pub fn permission_modes(kind: &str) -> Vec<String> {
    if let Some(provider) = yinz_types::agent::provider::provider(kind) {
        return provider.launch.permissions.keys().cloned().collect();
    }
    let modes = match normalize_kind(kind) {
        "codex" => CODEX_PERMISSION_MODES,
        "claude" => CLAUDE_PERMISSION_MODES,
        "cursor" => CURSOR_PERMISSION_MODES,
        "muse" => MUSE_PERMISSION_MODES,
        "agy" => AGY_PERMISSION_MODES,
        // pi's permission gating is its extensions', not a flag; the select offers
        // only the default, which is the truth of it.
        "pi" => PI_PERMISSION_MODES,
        _ => &[],
    };
    modes.iter().map(|value| (*value).to_owned()).collect()
}

/// The program a kind runs.
///
/// Identity for all but one: `cursor` ships its agent as `cursor-agent`, and the bare
/// `cursor` on the path is the editor. The mapping is here rather than at each call
/// site because there used to be two copies of it and they were one release apart from
/// disagreeing.
pub fn binary_for(kind: &str) -> String {
    if let Some(provider) = yinz_types::agent::provider::provider(kind) {
        return provider.executable.clone();
    }
    builtin_binary_for(kind).to_owned()
}

fn builtin_binary_for(kind: &str) -> &str {
    match normalize_kind(kind) {
        "cursor" => "cursor-agent",
        other => other,
    }
}

/// The agent dialect, independently of the executable used to launch it.
/// Explicit kinds and executable aliases share detection's canonical spelling.
pub fn kind_for_spec(spec: &Spec) -> Option<String> {
    trimmed(&spec.kind)
        .map(|kind| {
            yinz_types::agent::kind_of(kind)
                .unwrap_or_else(|| normalize_kind(&kind.to_lowercase()).to_owned())
        })
        .or_else(|| {
            let command = trimmed(&spec.command)?;
            yinz_types::agent::kind_from_program_and_args(std::path::Path::new(command), &spec.args)
        })
}

/// Route a Codex UI through the pane-aware launcher, retaining its original arguments.
pub fn wrap_codex(program: String, args: Vec<String>) -> (String, Vec<String>) {
    wrap_agent_launcher("codex", program, args)
}

/// Route a Muse session through the pane-aware launcher, retaining its original
/// arguments. Muse hooks run with a filtered environment, so the session id, not
/// an inherited pane variable, routes reports back to the pane - the same reason
/// Codex is wrapped.
pub fn wrap_muse(program: String, args: Vec<String>) -> (String, Vec<String>) {
    wrap_agent_launcher("muse", program, args)
}

fn wrap_agent_launcher(
    launcher: &str,
    program: String,
    args: Vec<String>,
) -> (String, Vec<String>) {
    let mut wrapped = vec![
        "agent".to_owned(),
        launcher.to_owned(),
        "--command".to_owned(),
        program,
        "--".to_owned(),
    ];
    wrapped.extend(args);
    ("yinz".to_owned(), wrapped)
}

/// Where a branch's worktree goes: `<dir>/<branch>`, with [`WORKTREE_PREFIX`] for a caller
/// who does not name a directory.
///
/// The branch is the whole of the last component, and there is no way to ask for anything
/// else — a caller chooses where a worktree is *placed*, never what it is called. A worktree
/// whose name and branch disagree is a folder nobody can identify from either half, and it
/// used to be one tool call away.
pub fn worktree_path_in(dir: Option<&str>, branch: &str) -> String {
    let dir = dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .unwrap_or(WORKTREE_PREFIX);
    format!("{}/{branch}", dir.trim_end_matches('/'))
}

/// The arguments a kind's program takes for what a [`Spec`] asks, as `(leading, flags)`:
/// `leading` goes before the caller's own arguments (a `resume` subcommand), `flags` after
/// them and before the prompt.
///
/// One table, verified against each program's `--help`. A mode a kind does not have is an
/// error naming the ones it does, never silently dropped: a caller who asked for permissions
/// off and got an agent that asks would not find out until it stalled.
pub fn shape_args(kind: Option<&str>, spec: &Spec) -> Result<(Vec<String>, Vec<String>), String> {
    let kind = kind.map(normalize_kind);
    if let Some(provider) = kind.and_then(yinz_types::agent::provider::provider) {
        return provider_args(&provider, spec);
    }
    builtin_shape_args(kind, spec)
}

fn builtin_shape_args(
    kind: Option<&str>,
    spec: &Spec,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut leading = Vec::new();
    let mut flags = Vec::new();
    let mut model = trimmed(&spec.model).map(str::to_owned);
    if let (Some("codex"), Some(model)) = (kind, model.as_deref()) {
        // ponytail: catch Claude shorthand; broader validation needs the configured
        // provider's catalog, not an allowlist that rejects custom model IDs.
        let alias = model.to_ascii_lowercase();
        let alias = alias.strip_suffix("[1m]").unwrap_or(&alias);
        if matches!(alias, "haiku" | "sonnet" | "opus" | "opusplan") {
            return Err(format!(
                "Codex does not support Claude model alias {model:?}; use --kind claude or a Codex model ID"
            ));
        }
    }
    if let Some(effort) = trimmed(&spec.effort) {
        let accepted = effort_levels(kind);
        if !accepted.contains(&effort) {
            return Err(format!(
                "{} does not support effort {effort:?}; accepted: {}",
                kind.unwrap_or("this command"),
                accepted.join(", ")
            ));
        }
        match kind {
            Some("codex") => flags.extend([
                "-c".to_owned(),
                format!("model_reasoning_effort=\"{effort}\""),
            ]),
            Some("claude" | "agy") => flags.extend(["--effort".to_owned(), effort.to_owned()]),
            Some("muse") => flags.extend(["--reasoning-effort".to_owned(), effort.to_owned()]),
            // pi spells the levels the same way, except the one the other kinds call
            // `none`: pi's own word for it is `off`.
            Some("pi") => flags.extend([
                "--thinking".to_owned(),
                if effort == "none" {
                    "off".to_owned()
                } else {
                    effort.to_owned()
                },
            ]),
            Some("cursor") => {
                let value = model
                    .as_mut()
                    .ok_or("Cursor effort requires an explicit model")?;
                if value.contains(['[', ']']) {
                    return Err(
                        "Cursor effort requires a model without bracket overrides".to_owned()
                    );
                }
                value.push_str(&format!("[effort={effort}]"));
            },
            _ => unreachable!(),
        }
    }
    if let Some(model) = model {
        flags.push("--model".to_owned());
        flags.push(model);
    }
    let mode = trimmed(&spec.permission_mode);
    let resume = trimmed(&spec.resume);
    let hooks = spec.hooks.unwrap_or(true);
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
            if spec.hooks.unwrap_or(true) && resume.is_some() && mode.is_some() {
                return Err("Yinz's managed Codex bridge cannot resume with --permission-mode; omit --permission-mode to resume, or start a new conversation with the requested mode".to_owned());
            }
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
        Some("muse") => {
            // No hooks are handed over here: Muse takes no inline hook settings, so a
            // spawned pane reports itself only when `yinz hooks install` has run -
            // the same shape Codex has, for the same reason.
            match mode {
                None => {},
                Some("skip") => flags.push("--yolo".to_owned()),
                Some(mode) if MUSE_PERMISSION_MODES.contains(&mode) => {
                    flags.push("--approval-mode".to_owned());
                    flags.push(mode.to_owned());
                },
                Some(mode) => return unknown_mode(MUSE_PERMISSION_MODES, mode),
            }
            // A subcommand, so it goes first: `muse resume --last` and
            // `muse resume <session-ref>`. Bare `muse resume` opens an interactive
            // picker, which is why a resume is always spelled with its target.
            match resume {
                None => {},
                Some("last") => leading.extend(["resume".to_owned(), "--last".to_owned()]),
                Some(id) => leading.extend(["resume".to_owned(), id.to_owned()]),
            }
        },
        Some("agy") => {
            match mode {
                None => {},
                Some("skip") => flags.push("--dangerously-skip-permissions".to_owned()),
                Some("accept-edits" | "acceptEdits") => {
                    flags.push("--mode".to_owned());
                    flags.push("accept-edits".to_owned());
                },
                Some("plan") => {
                    flags.push("--mode".to_owned());
                    flags.push("plan".to_owned());
                },
                Some(mode) => return unknown_mode(AGY_PERMISSION_MODES, mode),
            }
            match resume {
                None => {},
                Some("last") => flags.push("--continue".to_owned()),
                Some(id) => {
                    flags.push("--conversation".to_owned());
                    flags.push(id.to_owned());
                },
            }
        },
        Some("pi") => {
            // No permission mode is spelled: pi's permission gating is its extensions',
            // not the command line's, and a caller who asked for one needs to hear that
            // rather than get an agent that asks about everything. Named rather than the
            // empty `unknown_mode` table, which would say "it accepts: " and nothing else.
            if let Some(mode) = mode {
                return Err(format!(
                    "pi has no permission mode {mode:?}; permissions are set by pi \
                     extensions, not the command line"
                ));
            }
            match resume {
                None => {},
                Some("last") => flags.push("--continue".to_owned()),
                Some(id) => {
                    flags.push("--session".to_owned());
                    flags.push(id.to_owned());
                },
            }
            if hooks {
                flags.push("--extension".to_owned());
                flags.push(super::pi::EXTENSION_ARGUMENT.to_owned());
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

fn provider_args(
    provider: &yinz_types::agent::provider::AgentProvider,
    spec: &Spec,
) -> Result<(Vec<String>, Vec<String>), String> {
    use yinz_types::agent::provider::expand_args;
    let mut leading = provider.launch.args.clone();
    let mut flags = Vec::new();
    if let Some(model) = trimmed(&spec.model) {
        if provider.launch.model_args.is_empty() {
            return Err(format!(
                "{} does not support selecting a model",
                provider.kind
            ));
        }
        flags.extend(expand_args(&provider.launch.model_args, model));
    }
    for (label, value, options) in [
        ("effort", trimmed(&spec.effort), &provider.launch.efforts),
        (
            "permission mode",
            trimmed(&spec.permission_mode),
            &provider.launch.permissions,
        ),
    ] {
        if let Some(value) = value {
            let args = options.get(value).ok_or_else(|| {
                format!(
                    "{} does not support {label} {value:?}; accepted: {}",
                    provider.kind,
                    options.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            flags.extend(expand_args(args, value));
        }
    }
    if let Some(resume) = trimmed(&spec.resume) {
        let template = if resume == "last" {
            &provider.launch.resume_last_args
        } else {
            &provider.launch.resume_args
        };
        if template.is_empty() {
            return Err(format!(
                "{} does not support resuming {resume:?}",
                provider.kind
            ));
        }
        if provider.launch.resume_before_args {
            leading.extend(expand_args(template, resume));
        } else {
            flags.extend(expand_args(template, resume));
        }
    }
    Ok((leading, flags))
}

/// The `--settings` value a spawned `claude` is handed: the hook wiring from
/// `yinz_types::agent`, as one JSON argument.
pub fn claude_hook_settings_argument() -> String {
    super::claude_hook_settings(super::CLAUDE_HOOK_REPORT_COMMAND).to_string()
}

/// The planned arguments without the `--settings <hook json>` pair this planner adds for
/// claude, and the `--extension <path>` pair it adds for pi: a few kilobytes of hook
/// wiring and a data-dir path that say nothing about the launch. Only the planner's own
/// pairs are dropped; any other `--settings` or `--extension` is kept.
pub fn display_args(args: &[String]) -> Vec<String> {
    let hooks = claude_hook_settings_argument();
    let mut shown = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--settings" && args.get(index + 1) == Some(&hooks)
            || args[index] == "--extension"
                && args
                    .get(index + 1)
                    .is_some_and(|arg| super::pi::is_extension_argument(arg))
        {
            index += 2;
            continue;
        }
        shown.push(args[index].clone());
        index += 1;
    }
    shown
}

/// A pane's stringified command (`RunCommand`'s space-joined form) without the planner's
/// hook wiring: the `--settings <hook json>` pair, the `YINZ_CLAUDE_INLINE_HOOKS`
/// and agent-pane markers, and the
/// `env` wrapper when the markers were all it carried.
///
/// The settings are matched as JSON, not text: a plugin and the server can order the same
/// object's keys differently. Pi's generated extension is filtered by `RunCommand`
/// before its arguments are joined. For display only. Classification reads the full command.
pub fn display_command(command: &str) -> String {
    const FLAG: &str = " --settings ";
    let mut command = command.to_owned();
    if let Some(flag) = command.find(&format!("{FLAG}{{")) {
        let json = flag + FLAG.len();
        let mut values =
            serde_json::Deserializer::from_str(&command[json..]).into_iter::<serde_json::Value>();
        if let Some(Ok(value)) = values.next() {
            if value == super::claude_hook_settings(super::CLAUDE_HOOK_REPORT_COMMAND) {
                let end = json + values.byte_offset();
                command.replace_range(flag..end, "");
            }
        }
    }
    let mut words: Vec<&str> = command.split(' ').collect();
    let is_env = |word: &str| word == "env" || word.ends_with("/env");
    let runs_a_program = words.iter().skip(1).any(|word| !word.contains('='));
    if runs_a_program && words.first().is_some_and(|word| is_env(word)) {
        let pane_marker = format!("{}=1", yinz_types::agent::AGENT_PANE_ENV);
        words.retain(|word| {
            !matches!(
                *word,
                "YINZ_CLAUDE_INLINE_HOOKS=1" | "YINZ_CLAUDE_INLINE_HOOKS=0"
            ) && *word != pane_marker
        });
        if words.get(1).is_some_and(|word| !word.contains('=')) {
            words.remove(0);
        }
    }
    let start = if words.first().is_some_and(|word| is_env(word)) {
        words
            .iter()
            .skip(1)
            .position(|word| !word.contains('='))
            .map(|index| index + 1)
    } else {
        Some(0)
    };
    if let Some(start) = start {
        if words.get(start..start + 6).is_some_and(|args| {
            (args[0] == "yinz" || args[0].ends_with("/yinz"))
                && args[1] == "agent"
                && matches!(args[2], "claude" | "codex-untracked")
                && args[3] == "--command"
                && args[5] == "--"
        }) {
            let executable = words[start + 4];
            words.splice(start..start + 6, [executable]);
        }
    }
    words.join(" ")
}

fn launch_environment(spec: &Spec, kind: Option<&str>) -> BTreeMap<String, String> {
    let mut env = spec.env.clone();
    if kind == Some("claude") {
        // Override inherited markers too: installed hooks must report without inline hooks.
        let inline_hooks = spec.hooks.unwrap_or(true);
        env.insert(
            "YINZ_CLAUDE_INLINE_HOOKS".into(),
            if inline_hooks { "1" } else { "0" }.into(),
        );
    }
    env
}

/// What to run, and what to do to the repository before running it.
///
/// The error strings are the ones shown to whoever asked — the modal puts them in its
/// status row, agent hands them back to the model as the tool result — so they say what
/// is missing rather than that something is.
pub fn plan(spec: &Spec) -> Result<(CommandToRun, GitCheckout), String> {
    let target = PaneTarget::Local;
    let kind = kind_for_spec(spec);
    let provider = kind
        .as_deref()
        .and_then(yinz_types::agent::provider::provider);
    let executable = trimmed(&spec.command);
    if let (Some(explicit), Some(executable)) = (trimmed(&spec.kind), executable) {
        if let Some(detected) = yinz_types::agent::kind_from_program_and_args(
            std::path::Path::new(executable),
            &spec.args,
        ) {
            if kind.as_deref() != Some(detected.as_str()) {
                return Err(format!(
                    "agent kind {explicit:?} conflicts with {detected} executable {executable:?}"
                ));
            }
        }
    }
    let program = match (kind.as_deref(), executable, &provider) {
        (_, Some(command), _) => command.to_owned(),
        (Some(_), None, Some(provider)) => provider.executable.clone(),
        (Some(kind), None, None) => match trimmed(&spec.kind) {
            Some(name) if yinz_types::agent::kind_of(name).is_some() && name.contains('/') => {
                name.to_owned()
            },
            Some(name)
                if yinz_types::agent::kind_of(name).is_some() && name.contains(['.', '-']) =>
            {
                name.to_lowercase()
            },
            _ => builtin_binary_for(kind).to_owned(),
        },
        (None, None, _) => return Err("an agent or a command is required".to_owned()),
    };
    let indirect_agent = yinz_types::agent::kind_of(&program).is_none()
        && yinz_types::agent::kind_from_program_and_args(
            std::path::Path::new(&program),
            &spec.args,
        )
        .is_some();
    if matches!(kind.as_deref(), Some("codex" | "muse"))
        && spec.hooks.unwrap_or(true)
        && indirect_agent
    {
        return Err("managed Codex and Muse require a directly executable agent, not an interpreter command; point command at the agent's executable shim".to_owned());
    }

    let (leading, flags) = match &provider {
        Some(provider) => provider_args(provider, spec)?,
        None => builtin_shape_args(kind.as_deref(), spec)?,
    };
    if indirect_agent && !leading.is_empty() {
        return Err("resume for an interpreter or wrapper command requires a directly executable agent shim; alternatively, put the complete resume invocation in args and omit resume".to_owned());
    }
    let mut args = leading;
    args.extend(spec.args.iter().cloned());
    args.extend(flags);
    // Settled from the flags alone, before the prompt joins them: a prompt whose text reads
    // like a flag is still a prompt, and what the agent was asked to do says nothing about
    // how it was asked to run.
    let env = launch_environment(spec, kind.as_deref());
    args.extend(prompt_args(kind.as_deref(), provider.as_deref(), spec)?);

    let (program, args) = match kind.as_deref() {
        Some("codex") if spec.hooks.unwrap_or(true) => wrap_codex(program, args),
        Some("codex") => wrap_agent_launcher("codex-untracked", program, args),
        Some("claude") => wrap_agent_launcher("claude", program, args),
        Some("muse") if spec.hooks.unwrap_or(true) => wrap_muse(program, args),
        _ => (program, args),
    };
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

    // A directory to put a worktree in, on a spec that will not cut one, is a caller who
    // means something this cannot do. Answered rather than ignored: the alternative is an
    // agent somewhere other than where the field says, with nothing said about it.
    let worktree_dir = trimmed(&spec.worktree_dir);

    let Some(branch) = trimmed(&spec.branch) else {
        // A worktree asked for with nothing to put in it. Saying so beats silently
        // running in the current checkout, which is the one thing the caller said
        // they did not want.
        if spec.worktree == Some(true) {
            return Err("a branch is required to create a worktree".to_owned());
        }
        if worktree_dir.is_some() {
            return Err("worktree_dir applies only to a spawn that cuts a worktree".to_owned());
        }
        return Ok((command, GitCheckout::Current));
    };

    let base = trimmed(&spec.base).unwrap_or(DEFAULT_BASE);
    if !spec.worktree.unwrap_or(true) {
        if worktree_dir.is_some() {
            return Err("worktree_dir applies only to a spawn that cuts a worktree".to_owned());
        }
        return Ok((
            command,
            GitCheckout::NewBranch {
                branch: branch.to_owned(),
                base: base.to_owned(),
            },
        ));
    }
    let path = worktree_path_in(worktree_dir, branch);
    Ok((
        command,
        GitCheckout::NewWorktree {
            branch: branch.to_owned(),
            base: base.to_owned(),
            path: path.into(),
        },
    ))
}

/// Opening prompt grammar shared by launches and restarts of held commands.
pub(crate) fn prompt_args(
    kind: Option<&str>,
    provider: Option<&yinz_types::agent::provider::AgentProvider>,
    spec: &Spec,
) -> Result<Vec<String>, String> {
    // `muse resume` takes no opening prompt - the parser rejects the combination -
    // so a spec asking for both is refused rather than planned into a command
    // that dies on launch. Every other kind takes its prompt positionally beside
    // a resume.
    if kind == Some("muse") && trimmed(&spec.resume).is_some() && trimmed(&spec.prompt).is_some() {
        return Err(
            "a muse resume takes no opening prompt; prompt the pane after it starts".to_owned(),
        );
    }
    let mut args = Vec::new();
    // Prompt placement belongs to the provider; argv templates preserve it as one argument.
    if let Some(prompt) = trimmed(&spec.prompt) {
        if let Some(provider) = &provider {
            if provider.launch.prompt_args.is_empty() {
                return Err(format!(
                    "{} does not support an opening prompt",
                    provider.kind
                ));
            }
            args.extend(yinz_types::agent::provider::expand_args(
                &provider.launch.prompt_args,
                prompt,
            ));
        } else {
            if kind == Some("agy") {
                args.push("--prompt-interactive".to_owned());
            }
            args.push(prompt.to_owned());
        }
    }

    Ok(args)
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
/// `bubblewrap` is the recorded environment target (a checkout),
/// so the resume uses that environment's writable state or fails closed.
///
/// Every refusal here is written to be read on a divider one row high, so each says what is
/// missing in as few characters as it can and leaves the reasoning to these comments.
pub fn resume_command(
    kind: &str,
    session_id: &str,
    cwd: Option<&str>,
    host: Option<&str>,
    bubblewrap: Option<&str>,
) -> Result<CommandToRun, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        // A run recorded by a hook script that reported no session id of its own: its id
        // is the `kind:pane:<n>:<started at>:<machine>` fallback, and there is nothing in
        // it to hand to `--resume`.
        return Err("this run recorded no session id to resume".to_owned());
    }
    let target = match bubblewrap.map(str::trim).filter(|root| !root.is_empty()) {
        Some(_) if host.is_some() => {
            return Err("this run recorded both a host and a Bubblewrap environment".to_owned())
        },
        Some(root) => PaneTarget::bubblewrap(std::path::Path::new(root))?,
        None => PaneTarget::parse_display(host),
    };

    let cwd = cwd
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(std::path::PathBuf::from);
    // The mounts a plugin's own paths are read through. The host rewrites the cwd of a local
    // command that starts with one of them (`translate_plugin_path`, in
    // yinz-plugin-host/src/yinz_exports.rs), and a run's cwd is a real host path taken
    // from the agent's hook rather than a plugin path: a run recorded in `/tmp/scratch` would
    // resume in `$YINZ_TMP_DIR/plugin-tmp/scratch`, which is the wrong folder rather than a
    // refusal. A remote cwd is the far machine's and is passed through as it stands, so this is
    // the local case only - and the list has to stay in step with the one over there.
    if !target.is_remote() {
        if let Some(mount) = cwd.as_ref().and_then(|cwd| {
            ["/host", "/data", "/cache", "/tmp"]
                .into_iter()
                .find(|mount| cwd.starts_with(mount))
        }) {
            return Err(format!(
                "agent cannot open a pane under {mount}: the plugin mounts claim it"
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

/// Codex needs network access to connect to Yinz's Unix socket; this also enables internet access.
pub fn allow_sandboxed_yinz(spec: &mut AgentSpawnSpec) -> Result<(), String> {
    if kind_for_spec(spec).as_deref() != Some("codex") {
        return Err(
            "--allow-yinz applies only to Codex launches (--kind codex or a Codex command)".into(),
        );
    }
    spec.args.extend(codex_yinz_args());
    Ok(())
}

pub fn codex_yinz_args() -> [String; 2] {
    [
        "-c".into(),
        "sandbox_workspace_write.network_access=true".into(),
    ]
}

#[cfg(test)]
#[path = "unit/launch_tests.rs"]
mod launch_tests;

#[cfg(test)]
mod tests {
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
        let command =
            resume_command("claude", "abc", Some("/srv/w"), Some("build-box"), None).unwrap();
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
}
