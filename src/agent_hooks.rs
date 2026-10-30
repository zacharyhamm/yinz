//! `yinz hooks` — wiring coding-agent hooks to the agent plugins.
//!
//! `agent-service` decides whether an agent pane is busy, idle or blocked on the user
//! by reading its screen every five seconds and hashing what it finds, and — for the
//! last of those three — by asking a model about the pane's tail. That is the only
//! thing that works for an agent which says nothing about itself, and it is a guess
//! bought with about fifteen seconds and an HTTP round trip.
//!
//! Claude Code, Codex and opencode say. This command installs a small script (or, for
//! opencode, a plugin) as an agent hook so that an agent running in a yinz pane reports
//! its own state, and the daemon stops guessing about that pane. Everything here is local
//! file work: it writes a script and merges some entries into a settings file, and never
//! needs a session.
//!
//! The two halves that must agree with something else are marked where they appear: the
//! pipe's name and its argument, which are `agent_proto::wire`'s, and the shape of the
//! settings block, which the JSON agents share.

use anyhow::{anyhow, Context, Result};
use clap::ValueEnum;
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use {yinz_cli::args::HookAgent, yinz_cli::args::Hooks};

#[cfg(test)]
#[path = "unit/hook_writes.rs"]
mod hook_writes;

/// The events, the timeout and the handler shape live in `yinz_types::agent`, so that this
/// installer and a spawned Claude Code pane's inline `--settings` cannot wire different events.
///
/// `pub(crate)` on the Claude table because the CLI tests count against it rather than against
/// a literal: it has grown once already, and a number here goes stale silently the next time
/// it does.
pub(crate) const CLAUDE_EVENTS: &[&str] = yinz_types::agent::CLAUDE_HOOK_EVENTS;
const CODEX_EVENTS: &[&str] = yinz_types::agent::CODEX_HOOK_EVENTS;
const MUSE_EVENTS: &[&str] = yinz_types::agent::MUSE_HOOK_EVENTS;
const AGY_EVENTS: &[&str] = yinz_types::agent::AGY_HOOK_EVENTS;
const PI_EVENTS: &[&str] = yinz_runtime::agent::pi::PI_HOOK_EVENTS;

/// The pi extension `install` writes, with this binary's own path baked in.
///
/// pi is extended with a TypeScript module rather than a shell hook, so it is the
/// second agent - beside opencode - whose wiring is one file the agent loads, and the
/// file is the very one a spawned pane is handed inline: `yinz_runtime::agent::pi`
/// renders it, so an installed copy and a spawned pane's copy cannot drift apart.
fn pi_extension(yinz: &Path) -> String {
    yinz_runtime::agent::pi::render_pi_extension(yinz)
}

/// Bumped whenever [`script`] changes, or whenever the set of events wired around it does, so
/// `status` can say the installed copy is old. v4 was the seventeen-event table; v5 is the one
/// that names the agent it was written for; v6 is the one with a remote route, picking it by
/// the pane token rather than reporting to a session that need not exist on this machine.
/// v10 skips installed Claude reports when the launcher supplies inline hooks.
/// v11 keeps installed reports when the inline command cannot find yinz on PATH.
///
/// A missed bump is silent and total: `status` calls the old script current, and every event
/// on a machine installed before the change goes nowhere.
const SCRIPT_VERSION: u32 = 14;

/// The same discipline for the generated opencode plugin, which is one file rather than a
/// script plus a settings block. Bumped whenever [`opencode_plugin`] changes.
#[cfg(test)]
const OPENCODE_PLUGIN_VERSION: u32 = 6;

/// What marks an entry in the settings file as ours, and nothing else as ours.
///
/// Matched against the entry's `command`, so a person who moved the script or wrapped
/// it in something still has their entry replaced rather than duplicated.
const MARKER: &str = "yinz-hook.sh";

// Compatibility names select their provider once, before any hook operation.
fn provider_location(agent: HookAgent) -> Option<&'static str> {
    match agent {
        HookAgent::Opencode => Some("yinz:agent-opencode"),
        _ => None,
    }
}

/// Provider metadata is evaluated from Scheme in a worker with no host authority.
/// The CLI, after explicit install/uninstall, performs only the declared file operations.
fn provider_command(command: &Hooks, location: &str, data_dir: &Path) -> Result<()> {
    if matches!(
        command,
        Hooks::Install { project: true, .. }
            | Hooks::Uninstall { project: true, .. }
            | Hooks::Install {
                project_dir: Some(_),
                ..
            }
            | Hooks::Uninstall {
                project_dir: Some(_),
                ..
            }
    ) {
        return Err(anyhow!(
            "Scheme provider hook files are installed at their declared home or config paths"
        ));
    }
    let provider = yinz_server::inspect_agent_provider(location, data_dir)?;
    if provider.hooks.is_empty() {
        return Err(anyhow!("{} declares no hook files", provider.kind));
    }
    let home = home()?;
    let config =
        non_empty(std::env::var_os("XDG_CONFIG_HOME")).unwrap_or_else(|| home.join(".config"));
    let binary = std::env::current_exe().context("find this yinz binary")?;
    for hook in &provider.hooks {
        use yinz_runtime::agent::hook_files;
        let path = hook_files::path(hook, &home, &config);
        match command {
            Hooks::Install { dry_run, .. } => {
                let content = hook_files::render(
                    hook,
                    &binary,
                    yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV,
                )?;
                hook_files::read_owned(&path, &hook.marker)?;
                remove_legacy_opencode(&path, &hook.marker, *dry_run)?;
                if *dry_run {
                    println!("would write {}:\n{content}", path.display());
                } else {
                    hook_files::install(&path, hook, &content)?;
                    println!("wrote {}", path.display());
                }
            },
            Hooks::Uninstall { .. } => {
                hook_files::uninstall(&path, hook)?;
                println!("removed {}", path.display());
            },
            Hooks::Status { .. } => provider_status(&path, hook, &binary)?,
        }
    }
    Ok(())
}

fn remove_legacy_opencode(path: &Path, marker: &str, dry_run: bool) -> Result<()> {
    if marker != "yinz-opencode-plugin" {
        return Ok(());
    }
    let legacy = path.with_file_name("hocus.js");
    if std::fs::read_to_string(&legacy)
        .ok()
        .is_some_and(|source| source.contains("hocus-opencode-plugin"))
    {
        if dry_run {
            println!("would remove {}", legacy.display());
        } else {
            std::fs::remove_file(&legacy).context("remove legacy managed opencode hook")?;
        }
    }
    Ok(())
}

fn status_line(message: std::fmt::Arguments<'_>) -> Result<()> {
    crate::agent::allow_closed_stdout(
        writeln!(std::io::stdout().lock(), "{message}").context("cannot write hooks status"),
    )
}

fn provider_status(
    path: &Path,
    hook: &yinz_types::agent::provider::AgentHookArtifact,
    binary: &Path,
) -> Result<()> {
    use yinz_runtime::agent::hook_files;
    let expected = hook_files::render(
        hook,
        binary,
        yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV,
    )?;
    let status = match hook_files::read_owned(path, &hook.marker)? {
        Some(text) if text == expected => "current",
        Some(_) => "outdated; run install again",
        None => "missing",
    };
    status_line(format_args!("hook: {} ({status})", path.display()))?;
    Ok(())
}

pub(crate) fn run(hooks: Hooks, data_dir: &Path) -> Result<()> {
    let (name, project) = match &hooks {
        Hooks::Install {
            agent,
            project,
            project_dir,
            ..
        }
        | Hooks::Uninstall {
            agent,
            project,
            project_dir,
        } => (agent.as_deref(), *project || project_dir.is_some()),
        Hooks::Status { agent } => (agent.as_deref(), false),
    };
    let selected = match name {
        Some(name) => match HookAgent::from_str(name, true) {
            Ok(agent) => Some(agent),
            Err(_) => return provider_command(&hooks, name, data_dir),
        },
        None => None,
    };
    if let Some(location) = selected.and_then(provider_location) {
        return provider_command(&hooks, location, data_dir);
    }
    let reporting = matches!(hooks, Hooks::Status { .. });
    let mut first = true;
    let result = for_each_agent(selected_agents(selected, project)?, |agent| {
        if reporting && !std::mem::take(&mut first) {
            status_line(format_args!(""))?;
        }
        if let Some(location) = provider_location(agent) {
            return provider_command(&hooks, location, data_dir);
        }
        match &hooks {
            Hooks::Install {
                project,
                project_dir,
                dry_run,
                ..
            } => {
                let yinz = std::env::current_exe().context("cannot find this yinz binary")?;
                install(
                    &Files::resolve(agent, *project, project_dir.clone())?,
                    &yinz,
                    *dry_run,
                )
            },
            Hooks::Uninstall {
                project,
                project_dir,
                ..
            } => uninstall(&Files::resolve(agent, *project, project_dir.clone())?),
            Hooks::Status { .. } => {
                let files = if matches!(agent, HookAgent::Claude | HookAgent::Agy) {
                    Files::resolve(agent, true, None)
                        .or_else(|_| Files::resolve(agent, false, None))?
                } else {
                    Files::resolve(agent, false, None)?
                };
                status(&files)
            },
        }
    });
    if reporting {
        // Report shell context once, including when one agent's status failed.
        pane_line()?;
    }
    result
}

/// Run a command once for each selected agent, keeping one agent's failure to itself.
///
/// The agents are wired independently - an unparseable `~/.claude/settings.json` has nothing to
/// say about Codex or opencode - and the default form of each of these commands does all of
/// them. Stopping at the first error would leave the rest untouched and unmentioned, which is
/// the one thing a command that promises "every agent" must not do. Every failure is named
/// where it happened, and the run still ends non-zero, so a setup script still notices.
pub(crate) fn for_each_agent(
    agents: Vec<HookAgent>,
    mut run: impl FnMut(HookAgent) -> Result<()>,
) -> Result<()> {
    let mut failed = Vec::new();
    for agent in agents {
        if let Err(error) = run(agent) {
            eprintln!("{}: {error:#}", agent_name(agent));
            failed.push(agent_name(agent));
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    // Not the errors over again: `main` prints whatever comes back, and they are just above.
    Err(anyhow!("{} failed - see above", failed.join(" and ")))
}

/// The agent's own name for itself, before there is a [`Files`] to ask.
fn agent_name(agent: HookAgent) -> &'static str {
    match agent {
        HookAgent::Claude => "Claude Code",
        HookAgent::Codex => "Codex",
        HookAgent::Opencode => "opencode",
        HookAgent::Muse => "Muse",
        HookAgent::Agy => "Antigravity (agy)",
        HookAgent::Pi => "pi",
        HookAgent::All => unreachable!("all is expanded before an agent is named"),
    }
}

pub(crate) fn selected_agents(agent: Option<HookAgent>, project: bool) -> Result<Vec<HookAgent>> {
    let agent = agent.unwrap_or(if project {
        HookAgent::Claude
    } else {
        HookAgent::All
    });
    if project && !matches!(agent, HookAgent::Claude | HookAgent::Agy) {
        return Err(anyhow!(
            "only Claude Code and Antigravity have project hook files; use --agent claude or --agent agy with --project"
        ));
    }
    Ok(match agent {
        HookAgent::Claude => vec![HookAgent::Claude],
        HookAgent::Codex => vec![HookAgent::Codex],
        HookAgent::Opencode => vec![HookAgent::Opencode],
        HookAgent::Muse => vec![HookAgent::Muse],
        HookAgent::Agy => vec![HookAgent::Agy],
        HookAgent::Pi => vec![HookAgent::Pi],
        HookAgent::All => vec![
            HookAgent::Claude,
            HookAgent::Codex,
            HookAgent::Opencode,
            HookAgent::Muse,
            HookAgent::Agy,
            HookAgent::Pi,
        ],
    })
}

/// The script an agent runs, with this binary's own path baked in.
///
/// Baked rather than looked up because a hook inherits the environment `claude` was
/// started with, which is not always one where `yinz` is on the path — a desktop
/// launcher, a login shell that sets the path in a file an interactive shell reads.
/// The lookup stays as the fallback for the case the baked path stops existing,
/// which is what an upgrade that moves the binary looks like.
fn script(agent: HookAgent, yinz: &Path) -> String {
    // Codex hooks run in a shared daemon and Muse hooks run with a filtered
    // environment: neither inherits pane identity, so both route by the session id
    // the payload carries, through the pane a launcher registered.
    if matches!(agent, HookAgent::Codex | HookAgent::Muse) {
        let yinz = yinz.to_string_lossy().replace('\'', "'\\''");
        let (report, note, output) = match agent {
            HookAgent::Codex => (
                "codex-report",
                "Codex hooks run in a shared daemon. The conversation id, not that daemon's\n# inherited environment, selects the pane registered by `yinz agent codex`.",
                // A plain string, not a format template, so the braces stay single.
                "printf '{}\n'\n",
            ),
            HookAgent::Muse => (
                "muse-report",
                "Muse hooks run with a filtered environment. The session id, not an inherited\n# pane variable, selects the pane registered by `yinz agent muse`.",
                // Silent, where Codex answers neutral JSON: Muse reads stdout as an
                // optional decision object, and no output is no interference.
                "",
            ),
            _ => unreachable!("matched above"),
        };
        return format!(
            r#"#!/bin/sh
# yinz-hook v{SCRIPT_VERSION} - generated by `yinz hooks install`.
# {note}
YINZ_BIN='{yinz}'
[ -x "$YINZ_BIN" ] || YINZ_BIN=yinz
"$YINZ_BIN" agent {report} >/dev/null 2>&1
{output}exit 0
"#
        );
    }
    if agent == HookAgent::Agy {
        let yinz = yinz.to_string_lossy().replace('\'', "'\\''");
        let token_env = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let events = AGY_EVENTS.join("|");
        return format!(
            r#"#!/bin/sh
# yinz-hook v{SCRIPT_VERSION} - generated by `yinz hooks install`. Do not edit;
# `yinz hooks install` rewrites this file, and `--uninstall` removes it.
#
# Antigravity (agy) runs this on each wired hook event, with that event's JSON on stdin.
# It hands the JSON to whoever owns the pane - the session directly for a local pane
# (`yinz action agent-report`), that machine's pane broker for a remote one
# (`yinz --pty-broker report`) - which carries what it means on the pane and
# broadcasts the event itself to the plugins: the pane reports its own busy/waiting/idle
# state instead of being guessed at from its screen.
#
# Two rules this file keeps, both of which matter:
#   - it writes JSON decisions on stdout as expected by Antigravity.
#   - it always exits 0. A hook that fails must not interfere with the agent it is
#     only reporting on.

finish() {{
    case "$1" in
        PreToolUse)
            # "ask" preserves Antigravity's standard permission flow (respecting cached
            # permissions, permission allow rules, user prompts, and --dangerously-skip-permissions)
            # rather than unconditionally authorizing tools with "allow".
            printf '{{"decision":"ask"}}\n'
            ;;
        Stop)
            printf '{{"decision":"stop"}}\n'
            ;;
        *)
            printf '{{}}\n'
            ;;
    esac
    exit 0
}}

# The remote token is tested first, and that order is the whole point: a remote pane
# carries a ${token_env} *and* a $YINZ_PANE_ID, and the pane id it carries is the
# asking session's, on a machine that has no such session. Neither set: not a yinz pane at all,
# but Antigravity still requires valid JSON output.
[ -n "${token_env}" ] || [ -n "$YINZ_PANE_REF" ] || finish "$1"

YINZ_BIN='{yinz}'
[ -x "$YINZ_BIN" ] || YINZ_BIN=yinz

feed() {{
    case "$1" in
        {events})
            # Rust adds a missing event name after parsing the untouched payload.
            printf '["%s",' "$1"
            cat
            printf ']\n'
            ;;
        *) cat ;;
    esac
}}

# Either way the event travels on stdin rather than as an argument, clear of execve's
# per-argument limit.
if [ -n "${token_env}" ]; then
    # A remote pane - ssh.
    feed "$1" | "$YINZ_BIN" --pty-broker report agy >/dev/null 2>&1
else
    # A local pane: the action resolves $YINZ_PANE_REF at dispatch.
    feed "$1" | "$YINZ_BIN" action agent-report --hook-json --kind agy >/dev/null 2>&1
fi

finish "$1"
"#
        );
    }
    let yinz = shell_quote(yinz);
    let (name, wire_kind, kind, no_pane, output, output_rule) = match agent {
        HookAgent::Claude => (
            "Claude Code",
            "claude",
            "",
            "exit 0",
            "",
            "it never writes to stdout, which some events inject into context",
        ),
        HookAgent::Codex => unreachable!("Codex uses the session router above"),
        HookAgent::Muse => unreachable!("Muse uses the session router above"),
        HookAgent::Agy => unreachable!("Agy uses its own script generator above"),
        HookAgent::Opencode => {
            unreachable!("opencode runs a plugin, not a shell script; see `opencode_plugin`")
        },
        HookAgent::Pi => {
            unreachable!("pi runs an extension, not a shell script; see `pi_extension`")
        },
        HookAgent::All => unreachable!("all is expanded before a script is generated"),
    };
    // Spelled from the constant the broker reads, so the two halves of the remote route
    // cannot drift apart: a variable name that does not match is silently no route at all.
    let token_env = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    format!(
        r#"#!/bin/sh
# yinz-hook v{SCRIPT_VERSION} - generated by `yinz hooks install`. Do not edit;
# `yinz hooks install` rewrites this file, and `--uninstall` removes it.
#
# {name} runs this on each wired hook event, with that event's JSON on stdin.
# It hands the JSON to whoever owns the pane - the session directly for a local pane
# (`yinz action agent-report`), that machine's pane broker for a remote one
# (`yinz --pty-broker report`) - which carries what it means on the pane and
# broadcasts the event itself to the plugins: the pane reports its own busy/waiting/idle
# state instead of being guessed at from its screen, and what a record has no room for -
# the agent's own last message, the prompt behind it - reaches whoever wants it.
#
# Two rules this file keeps, both of which matter:
#   - {output_rule}.
#   - it always exits 0. A hook that fails must not interfere with the agent it is
#     only reporting on.

# Inline hooks need yinz on PATH; otherwise use this script's absolute binary path.
[ "$YINZ_CLAUDE_INLINE_HOOKS" = 1 ] && command -v yinz >/dev/null 2>&1 && exit 0

# The remote token is tested first, and that order is the whole point: a remote pane
# carries a ${token_env} *and* a $YINZ_PANE_ID, and the pane id it carries is the
# asking session's, on a machine that has no such session. Reading it here would send
# every event to whatever session happens to run on this box, under a pane id that
# means something else. Neither set: not a yinz pane at all, and nothing to report to.
[ -n "${token_env}" ] || [ -n "$YINZ_PANE_REF" ] || {{ {no_pane}; }}

YINZ_BIN={yinz}
[ -x "$YINZ_BIN" ] || YINZ_BIN=yinz

# Either way the event travels on stdin rather than as an argument, clear of execve's
# per-argument limit.
if [ -n "${token_env}" ]; then
    # A remote pane - ssh. There is no session on this machine to tell,
    # so the event goes to the broker that owns the pty, which names the pane by the
    # token it put here and relays it on the pane's own channel to the pane's server.
    "$YINZ_BIN" --pty-broker report {wire_kind} >/dev/null 2>&1
else
    # A local pane: the action resolves $YINZ_PANE_REF at dispatch.
    "$YINZ_BIN" action agent-report --hook-json{kind} >/dev/null 2>&1
fi
{output}

exit 0
"#
    )
}

#[cfg(test)]
fn opencode_plugin(yinz: &Path) -> String {
    let data = tempfile::tempdir().expect("provider fixture directory");
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), data.path())
        .expect("install provider fixture");
    let provider = yinz_server::inspect_agent_provider("yinz:agent-opencode", data.path())
        .expect("builtin Scheme provider evaluates");
    yinz_runtime::agent::hook_files::render(
        &provider.hooks[0],
        yinz,
        yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV,
    )
    .expect("hook template renders")
}

/// One handler entry, as both agents' settings spell it.
fn handler(command: &str, timeout: u64) -> Value {
    yinz_types::agent::hook_handler(command, timeout)
}

/// Whether one handler entry is one we wrote.
fn is_ours(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(MARKER) || command.contains("hocus-hook.sh"))
}

/// `settings` with our entries removed and then added back, for every event we wire.
///
/// Idempotent by construction: the removal runs first, so installing twice leaves one
/// entry per event rather than two. Nothing else in the file is touched — it is parsed
/// as a whole `Value` and handed back with the same keys it arrived with, so a setting
/// this binary has never heard of survives the round trip.
#[cfg(test)]
pub(crate) fn install_into(settings: Value, command: &str) -> Result<Value> {
    install_events_into(
        settings,
        command,
        CLAUDE_EVENTS,
        yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
    )
}

#[cfg(test)]
pub(crate) fn install_agy_into(settings: Value, command: &str) -> Result<Value> {
    install_agy_events_into(
        settings,
        command,
        AGY_EVENTS,
        yinz_types::agent::AGY_HOOK_TIMEOUT_SECONDS,
    )
}

fn install_events_into(
    settings: Value,
    command: &str,
    events: &[&str],
    timeout: u64,
) -> Result<Value> {
    let mut settings = uninstall_from(settings);
    let root = settings
        .as_object_mut()
        .expect("uninstall_from always answers an object");
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings hooks must be an object"))?;
    for event in events {
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| anyhow!("settings hooks.{event} must be an array"))?;
        // No `matcher`: an entry without one fires on every occurrence of the
        // event, which is what every one of them and both reporting feeds want.
        groups.push(json!({ "hooks": [handler(command, timeout)] }));
    }
    Ok(settings)
}

/// `settings` with every entry of ours removed, and nothing else changed.
///
/// A group we emptied goes with it, so uninstalling does not leave a pile of
/// `{"hooks":[]}` behind; a group that still holds someone else's handler stays, minus
/// ours. An event left with no groups is dropped for the same reason.
pub(crate) fn uninstall_from(mut settings: Value) -> Value {
    if !settings.is_object() {
        // An empty or unparseable file is an empty settings object, not an error: the
        // install below has to work on a machine where nobody has written one yet.
        settings = Value::Object(Map::new());
    }
    if installed_events(&settings).is_empty() {
        return settings;
    }
    let Some(hooks) = settings
        .as_object_mut()
        .expect("made an object above")
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
    else {
        return settings;
    };
    hooks.retain(|_, groups| {
        let Some(groups) = groups.as_array_mut() else {
            return true;
        };
        let mut removed = false;
        groups.retain_mut(|group| {
            if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = handlers.len();
                handlers.retain(|handler| !is_ours(handler));
                let changed = handlers.len() != before;
                removed |= changed;
                return !changed || !handlers.is_empty();
            }
            true
        });
        !removed || !groups.is_empty()
    });
    if hooks.is_empty()
        && let Some(root) = settings.as_object_mut()
    {
        root.remove("hooks");
    }
    settings
}

/// Which events in a settings document have an entry of ours.
pub(crate) fn installed_events(settings: &Value) -> Vec<String> {
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    hooks
        .iter()
        .filter(|(_, groups)| {
            groups
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|group| group.get("hooks")?.as_array())
                .flatten()
                .any(is_ours)
        })
        .map(|(event, _)| event.clone())
        .collect()
}

fn install_agy_events_into(
    settings: Value,
    command: &str,
    events: &[&str],
    timeout: u64,
) -> Result<Value> {
    let mut settings = uninstall_agy_from(settings);
    let root = settings
        .as_object_mut()
        .expect("uninstall_agy_from always answers an object");
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings hooks must be an object"))?;
    for event in events {
        let full_command = format!("{command} {event}");
        let item = if *event == "PreToolUse" || *event == "PostToolUse" {
            json!({
                "matcher": ".*",
                "hooks": [handler(&full_command, timeout)]
            })
        } else {
            handler(&full_command, timeout)
        };
        let items = hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let items = items
            .as_array_mut()
            .ok_or_else(|| anyhow!("settings hooks.{event} must be an array"))?;
        items.push(item);
    }
    Ok(settings)
}

pub(crate) fn uninstall_agy_from(mut settings: Value) -> Value {
    if !settings.is_object() {
        settings = Value::Object(Map::new());
    }
    if installed_agy_events(&settings).is_empty() {
        return settings;
    }
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return settings;
    };
    hooks.retain(|_, items| {
        let Some(items) = items.as_array_mut() else {
            return true;
        };
        let mut removed = false;
        items.retain_mut(|item| {
            if is_ours(item) {
                removed = true;
                return false;
            }
            if let Some(handlers) = item.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = handlers.len();
                handlers.retain(|handler| !is_ours(handler));
                let changed = handlers.len() != before;
                removed |= changed;
                return !changed || !handlers.is_empty();
            }
            true
        });
        !removed || !items.is_empty()
    });
    if hooks.is_empty()
        && let Some(root) = settings.as_object_mut()
    {
        root.remove("hooks");
    }
    settings
}

pub(crate) fn installed_agy_events(settings: &Value) -> Vec<String> {
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    hooks
        .iter()
        .filter(|(_, items)| {
            items.as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    is_ours(item)
                        || item
                            .get("hooks")
                            .and_then(Value::as_array)
                            .is_some_and(|handlers| handlers.iter().any(is_ours))
                })
            })
        })
        .map(|(event, _)| event.clone())
        .collect()
}

/// A variable that is set to nothing is one that is not set.
///
/// `export CODEX_HOME="$SOMETHING_UNSET"` leaves `Some("")` behind, and an empty root turns
/// every path built on it into a relative one - which is to say, into whichever folder the
/// person happened to be standing in when they ran `install`.
fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub(crate) fn home() -> Result<PathBuf> {
    non_empty(std::env::var_os("HOME"))
        .ok_or_else(|| anyhow!("HOME is not set, so there is no agent config to install into"))
}

pub(crate) fn agent_root(
    agent: HookAgent,
    claude_home: Option<OsString>,
    codex_home: Option<OsString>,
    pi_home: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    yinz_runtime::agent::paths::config_root(
        match agent {
            HookAgent::Claude => "claude",
            HookAgent::Codex => "codex",
            HookAgent::Pi => "pi",
            HookAgent::Opencode => "opencode",
            HookAgent::Muse => "muse",
            HookAgent::Agy => "agy",
            HookAgent::All => unreachable!("all is expanded before resolving files"),
        },
        claude_home,
        codex_home,
        pi_home,
        xdg_config_home,
        home,
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// The files a command touches, resolved once from the flags and the environment so that
/// everything below is plain path work - and runs against a temporary folder in a test.
pub(crate) struct Files {
    pub(crate) agent: HookAgent,
    /// The provider-local generated script, shared by its user and project installs.
    pub(crate) script: PathBuf,
    /// The provider's user-level hooks/settings document.
    pub(crate) user_settings: PathBuf,
    /// The project folder, when the command is about one rather than the user.
    pub(crate) project: Option<PathBuf>,
}

impl Files {
    /// `--project-dir` names the folder; a bare `--project` means the current one, which is
    /// what a person at a shell wants and what a plugin - whose cwd is the server's, not the
    /// project's - cannot use.
    fn resolve(agent: HookAgent, project: bool, project_dir: Option<PathBuf>) -> Result<Files> {
        anyhow::ensure!(
            provider_location(agent).is_none(),
            "provider hook paths come from its descriptor"
        );
        if matches!(
            agent,
            HookAgent::Codex | HookAgent::Opencode | HookAgent::Muse | HookAgent::Pi
        ) && (project || project_dir.is_some())
        {
            return Err(anyhow!("{} hooks are user-level only", agent_name(agent)));
        }
        let project = match project_dir {
            Some(dir) => Some(dir),
            None if project => {
                Some(std::env::current_dir().context("cannot tell which project this is")?)
            },
            None => None,
        };
        let root = agent_root(
            agent,
            std::env::var_os("CLAUDE_CONFIG_DIR"),
            std::env::var_os("CODEX_HOME"),
            std::env::var_os("PI_CODING_AGENT_DIR"),
            std::env::var_os("XDG_CONFIG_HOME"),
            home,
        )?;
        // Script hooks have a settings document beside them; pi loads its extension directly.
        let (script, user_settings) = match agent {
            HookAgent::Claude => (root.join("yinz").join(MARKER), root.join("settings.json")),
            HookAgent::Codex => (root.join("yinz").join(MARKER), root.join("hooks.json")),
            HookAgent::Muse => (
                yinz_muse::hook_script_path(&root),
                yinz_muse::settings_path(&root),
            ),
            HookAgent::Opencode => unreachable!("provider paths are resolved from its descriptor"),
            // The extension is its own wiring, so settings() is never asked for it.
            HookAgent::Pi => {
                let extension = root
                    .join("extensions")
                    .join(yinz_runtime::agent::pi::PI_EXTENSION_FILE);
                (extension.clone(), extension)
            },
            HookAgent::Agy => (
                root.join("yinz").join(MARKER),
                root.join("config").join("hooks.json"),
            ),
            HookAgent::All => unreachable!("all is expanded before resolving files"),
        };
        Ok(Files {
            agent,
            script,
            user_settings,
            project,
        })
    }

    /// The settings file this command writes.
    ///
    /// A project's is `settings.local.json`: Claude Code reads it beside the committed
    /// `settings.json` and keeps it out of git, so a per-project install does not land in the
    /// project's history with this machine's binary path in it. Claude Code *merges* hooks
    /// across settings files rather than one overriding the other, which is why installing into
    /// both the user's and a project's fires everything twice - `status` says so when it sees it.
    fn settings(&self) -> PathBuf {
        match &self.project {
            Some(dir) => match self.agent {
                HookAgent::Claude => dir.join(".claude").join("settings.local.json"),
                HookAgent::Agy => dir.join(".agents").join("hooks.json"),
                _ => unreachable!("only Claude and Antigravity support project settings"),
            },
            None => self.user_settings.clone(),
        }
    }

    /// The committed `.claude/settings.json` an older yinz wrote a project install into.
    fn legacy_project_settings(&self) -> Option<PathBuf> {
        (self.agent == HookAgent::Claude)
            .then_some(self.project.as_ref())
            .flatten()
            .map(|dir| dir.join(".claude").join("settings.json"))
    }

    fn events(&self) -> &'static [&'static str] {
        match self.agent {
            HookAgent::Claude => CLAUDE_EVENTS,
            HookAgent::Codex => CODEX_EVENTS,
            HookAgent::Opencode => unreachable!("provider hooks are declared by its descriptor"),
            HookAgent::Muse => MUSE_EVENTS,
            HookAgent::Agy => AGY_EVENTS,
            HookAgent::Pi => PI_EVENTS,
            HookAgent::All => unreachable!("all is expanded before resolving files"),
        }
    }

    fn name(&self) -> &'static str {
        agent_name(self.agent)
    }

    fn timeout(&self) -> u64 {
        match self.agent {
            HookAgent::Claude => yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
            HookAgent::Codex => yinz_types::agent::CODEX_HOOK_TIMEOUT_SECONDS,
            HookAgent::Opencode => unreachable!("provider hooks are declared by its descriptor"),
            HookAgent::Muse => yinz_types::agent::MUSE_HOOK_TIMEOUT_SECONDS,
            HookAgent::Agy => yinz_types::agent::AGY_HOOK_TIMEOUT_SECONDS,
            // pi reports from its extension events, which have no hook timeout to wire:
            // this arm exists only because the one-file install shares the type.
            HookAgent::Pi => 0,
            HookAgent::All => unreachable!("all is expanded before resolving files"),
        }
    }

    pub(crate) fn installed_events(&self, settings: &Value) -> Vec<String> {
        if self.agent == HookAgent::Agy {
            installed_agy_events(settings)
        } else {
            installed_events(settings)
        }
    }

    /// The legacy file with our entries still in it, if there is one to move out of.
    fn wired_legacy_project_settings(&self) -> Result<Option<(PathBuf, Value)>> {
        let Some(path) = self.legacy_project_settings() else {
            return Ok(None);
        };
        let settings = read_settings(&path)?;
        Ok((!installed_events(&settings).is_empty()).then_some((path, settings)))
    }
}

/// A settings file as a `Value`. A file that is not there is an empty object; one that
/// does not parse is an error, because writing over it would destroy it.
fn read_settings(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Write a settings file, keeping a copy of what was there.
///
/// Through a temporary file in the same folder and a rename, so a crash midway leaves
/// the old file rather than half of the new one. The `.bak` is written first and is
/// what a person reaches for when this got something wrong.
fn write_settings(path: &Path, settings: &Value) -> Result<bool> {
    let mut text = serde_json::to_string_pretty(settings).context("cannot serialize settings")?;
    text.push('\n');
    write_generated(path, &text)
}

/// The contents of a generated file, or `None` where there is no file.
fn read_generated(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Write a generated file, keeping a copy of what was there.
///
/// Settings and hooks share atomic replacement, private new files, and preserved permissions.
fn write_generated(path: &Path, text: &str) -> Result<bool> {
    let destination = if path.is_symlink() {
        path.canonicalize()
            .with_context(|| format!("cannot resolve {}", path.display()))?
    } else {
        path.to_owned()
    };
    let metadata = match std::fs::metadata(&destination) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file(),
                "{} is not a regular file",
                path.display()
            );
            Some(metadata)
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()))
        },
    };
    let backed_up = metadata.is_some();
    let folder = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder to write into", path.display()))?;
    std::fs::create_dir_all(folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    let kind = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("js");
    if backed_up {
        let backup = path.with_extension(format!("{kind}.bak"));
        let mut saved = tempfile::NamedTempFile::new_in(folder)
            .with_context(|| format!("cannot stage backup for {}", path.display()))?;
        let mut original = std::fs::File::open(&destination)
            .with_context(|| format!("cannot read {} for backup", path.display()))?;
        std::io::copy(&mut original, &mut saved)
            .with_context(|| format!("cannot back up {}", path.display()))?;
        if let Some(metadata) = &metadata {
            saved
                .as_file()
                .set_permissions(metadata.permissions())
                .context("cannot preserve backup permissions")?;
        }
        saved
            .as_file()
            .sync_all()
            .context("cannot sync hook backup")?;
        saved
            .persist(&backup)
            .with_context(|| format!("cannot replace backup {}", backup.display()))?;
    }
    let destination_folder = destination
        .parent()
        .context("hook destination has no folder")?;
    let mut temporary = tempfile::NamedTempFile::new_in(destination_folder).with_context(|| {
        format!(
            "cannot create temporary file in {}",
            destination_folder.display()
        )
    })?;
    if let Some(metadata) = metadata {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .context("cannot preserve hook permissions")?;
    }
    temporary
        .write_all(text.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .context("cannot sync hook file")?;
    temporary
        .persist(&destination)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(backed_up)
}

/// pi's install: one extension file, and no settings merge, because the file *is* the
/// wiring - the opencode shape, in pi's own extension directory.
///
/// The file is the same one a spawned pane is handed inline, so a machine with both sees
/// two copies in one pi process; the extension claims the process itself and the second
/// copy reports nothing, which is why the install never has to say "choose one".
fn install_pi(files: &Files, yinz: &Path, dry_run: bool) -> Result<()> {
    let extension = pi_extension(yinz);
    if let Some(existing) = read_generated(&files.script)?
        && !existing.contains(yinz_runtime::agent::pi::PI_EXTENSION_MARKER)
    {
        return Err(anyhow!(
            "{} exists and was not written by yinz; move it aside first",
            files.script.display()
        ));
    }
    if dry_run {
        println!("would write {}:\n", files.script.display());
        println!("{extension}");
        return Ok(());
    }
    let backed_up = write_generated(&files.script, &extension)?;
    println!("wrote {}", files.script.display());
    println!(
        "wired {} events through the pi extension",
        files.events().len()
    );
    if backed_up {
        println!(
            "kept the previous extension as {}",
            files.script.with_extension("ts.bak").display()
        );
    }
    println!("restart pi so it loads the extension");
    println!("a pane yinz spawns carries its own copy inline, so it reports from the start");
    println!("run `yinz hooks status` to check");
    Ok(())
}

pub(crate) fn install(files: &Files, yinz: &Path, dry_run: bool) -> Result<()> {
    if files.agent == HookAgent::Pi {
        return install_pi(files, yinz, dry_run);
    }
    let script = script(files.agent, yinz);
    let settings_path = files.settings();
    let settings = read_settings(&settings_path)?;
    let command = shell_quote(&files.script);
    let mut updated = if files.agent == HookAgent::Agy {
        install_agy_events_into(settings, &command, files.events(), files.timeout())?
    } else {
        install_events_into(settings, &command, files.events(), files.timeout())?
    };
    if files.agent == HookAgent::Muse {
        // Muse refuses every command when its settings lack `schema_version`, so a
        // file that has none - one we created, or one written before the setting
        // existed - gains one here. One that has any value keeps it: only Muse
        // itself may move its own schema forward.
        updated
            .as_object_mut()
            .expect("install_events_into always answers an object")
            .entry("schema_version")
            .or_insert(Value::Number(1.into()));
    }
    // A project install used to land in the committed file. Moving it out here is what makes
    // re-running `install --project` the upgrade path, rather than a second copy that fires
    // every event twice.
    let legacy = files.wired_legacy_project_settings()?;

    if dry_run {
        println!("would write {}:\n", files.script.display());
        println!("{script}");
        println!(
            "would merge into {} (everything else in it is left as it is):\n",
            settings_path.display()
        );
        let block = updated.get("hooks").unwrap_or(&Value::Null);
        println!("{}", serde_json::to_string_pretty(block)?);
        if let Some((path, settings)) = legacy {
            println!(
                "\nwould move {} events out of {}",
                files.installed_events(&settings).len(),
                path.display()
            );
        }
        return Ok(());
    }

    let folder = files.script.parent().expect("the script is in a folder");
    std::fs::create_dir_all(folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    write_generated(&files.script, &script)?;
    make_executable(&files.script)?;
    let backed_up = write_settings(&settings_path, &updated)?;

    println!("wrote {}", files.script.display());
    println!(
        "wired {} events in {}",
        files.events().len(),
        settings_path.display()
    );
    if backed_up {
        println!(
            "kept the previous settings as {}",
            settings_path.with_extension("json.bak").display()
        );
    }
    if let Some((path, settings)) = legacy {
        let moved = files.installed_events(&settings).len();
        write_settings(&path, &uninstall_from(settings))?;
        println!(
            "moved {moved} events out of {} - a project install lives in settings.local.json now",
            path.display()
        );
    }
    // Said rather than hidden: the file is re-serialized from a parse, so its keys
    // come back in sorted order. Nothing about the settings changes, but a file under
    // version control will show a diff that is mostly not ours.
    println!("(settings are rewritten from a parse, so key order may change)");
    println!("run `yinz hooks status` to check");
    match files.agent {
        HookAgent::Codex => {
            println!("open `/hooks` inside Codex to review and trust the installed hooks");
            println!(
                "launch with `yinz agent codex -- [codex arguments]` for shared-daemon pane tracking"
            );
        },
        HookAgent::Muse => {
            println!(
                "launch with `yinz agent muse -- [muse arguments]` for pane tracking (automatic for Yinz agent launches)"
            );
        },
        HookAgent::Claude => {
            println!("open `/hooks` inside Claude Code to inspect the installed hooks");
        },
        _ => {},
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("cannot make {} executable", path.display()))
}

pub(crate) fn uninstall(files: &Files) -> Result<()> {
    if files.agent == HookAgent::Pi {
        return uninstall_pi(files);
    }
    let settings_path = files.settings();
    let settings = read_settings(&settings_path)?;
    let removed = files.installed_events(&settings).len();
    if removed > 0 {
        let updated = if files.agent == HookAgent::Agy {
            uninstall_agy_from(settings)
        } else {
            uninstall_from(settings)
        };
        write_settings(&settings_path, &updated)?;
    }
    println!("removed {removed} events from {}", settings_path.display());
    if let Some((path, settings)) = files.wired_legacy_project_settings()? {
        let removed = files.installed_events(&settings).len();
        write_settings(&path, &uninstall_from(settings))?;
        println!("removed {removed} events from {}", path.display());
    }

    // The script is one file for the whole machine: a project uninstall leaves it to the
    // user-level install that still points at it.
    if files.project.is_some()
        && !files
            .installed_events(&read_settings(&files.user_settings)?)
            .is_empty()
    {
        println!(
            "kept {} - {} still uses it",
            files.script.display(),
            files.user_settings.display()
        );
        return Ok(());
    }
    match std::fs::remove_file(&files.script) {
        Ok(()) => println!("removed {}", files.script.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => {
            return Err(error).with_context(|| format!("cannot remove {}", files.script.display()))
        },
    }
    Ok(())
}

/// Remove pi's generated extension only when it is one we wrote.
fn uninstall_pi(files: &Files) -> Result<()> {
    let marker = yinz_runtime::agent::pi::PI_EXTENSION_MARKER;
    match read_generated(&files.script)? {
        Some(text) if text.contains(marker) => {
            std::fs::remove_file(&files.script)
                .with_context(|| format!("cannot remove {}", files.script.display()))?;
            println!("removed {}", files.script.display());
        },
        Some(_) => {
            return Err(anyhow!(
                "{} was not written by yinz; leaving it alone",
                files.script.display()
            ))
        },
        None => println!("nothing to remove at {}", files.script.display()),
    }
    Ok(())
}

/// What is wired, and the things that would quietly stop it working.
fn status(files: &Files) -> Result<()> {
    if files.agent == HookAgent::Pi {
        let marker = yinz_runtime::agent::pi::PI_EXTENSION_MARKER;
        let version = yinz_runtime::agent::pi::PI_EXTENSION_VERSION;
        status_line(format_args!("{}:", files.name()))?;
        match read_generated(&files.script)? {
            Some(text) if text.contains(&format!("{marker} v{version}")) => {
                status_line(format_args!(
                    "extension:  {} (current)",
                    files.script.display()
                ))?;
            },
            Some(text) if !text.contains(marker) => status_line(format_args!(
                "extension:  {} (foreign file - move it aside before install)",
                files.script.display()
            ))?,
            Some(_) => status_line(format_args!(
                "extension:  {} (older than this yinz - run `install` again)",
                files.script.display()
            ))?,
            None => status_line(format_args!(
                "extension:  {} (missing)",
                files.script.display()
            ))?,
        }
        return Ok(());
    }
    status_line(format_args!("{}:", files.name()))?;
    match std::fs::read_to_string(&files.script) {
        Ok(text) if text.contains(&format!("yinz-hook v{SCRIPT_VERSION} ")) => {
            status_line(format_args!(
                "script:  {} (current)",
                files.script.display()
            ))?;
        },
        Ok(_) => status_line(format_args!(
            "script:  {} (older than this yinz - run `install` again)",
            files.script.display()
        ))?,
        Err(_) => status_line(format_args!(
            "script:  {} (missing)",
            files.script.display()
        ))?,
    }

    let mut checks = vec![("user", files.user_settings.clone())];
    if files.project.is_some() {
        checks.push(("project", files.settings()));
        // The committed file is only worth a line when an older install is still in it.
        if let Ok(Some((legacy, _))) = files.wired_legacy_project_settings() {
            checks.push(("legacy", legacy));
        }
    }
    let mut wired_anywhere = 0usize;
    for (label, path) in checks {
        let heading = format!("{label}:");
        let settings = match read_settings(&path) {
            Ok(settings) => settings,
            Err(error) => {
                status_line(format_args!("{heading:<9}{} ({error:#})", path.display()))?;
                continue;
            },
        };
        let events = files.installed_events(&settings);
        wired_anywhere += usize::from(!events.is_empty());
        status_line(format_args!(
            "{heading:<9}{} - {} of {} events wired",
            path.display(),
            events.len(),
            files.events().len()
        ))?;
        if label == "legacy" {
            status_line(format_args!(
                "            an older project install; `install --project` moves it to \
                 settings.local.json"
            ))?;
        }
        // One line rather than one per event: an upgrade that wires new events leaves a
        // working install several short, and a column of them reads as a list of faults
        // rather than as the one instruction it is.
        let missing: Vec<&str> = files
            .events()
            .iter()
            .copied()
            .filter(|event| !events.iter().any(|wired| wired == event))
            .collect();
        if !events.is_empty() && !missing.is_empty() {
            status_line(format_args!("            missing: {}", missing.join(", ")))?;
            status_line(format_args!("            run `install` again to add them"))?;
        }
        // The two ways a correctly written hook never runs.
        if settings.get("disableAllHooks") == Some(&Value::Bool(true)) {
            status_line(format_args!(
                "            disableAllHooks is on: nothing here runs"
            ))?;
        }
        if settings.get("allowManagedHooksOnly") == Some(&Value::Bool(true)) {
            status_line(format_args!(
                "            allowManagedHooksOnly is on: nothing here runs"
            ))?;
        }
        if files.agent == HookAgent::Muse
            && settings.get("schema_version").is_none()
            && path.exists()
        {
            status_line(format_args!(
                "            schema_version is missing: muse refuses to run without it"
            ))?;
        }
    }
    if files.agent == HookAgent::Claude && wired_anywhere > 1 {
        status_line(format_args!(
            "note:    wired in more than one file. Claude Code merges hooks rather than \
             overriding, so every event fires twice"
        ))?;
    }

    if files.agent == HookAgent::Codex && wired_anywhere > 0 {
        status_line(format_args!(
            "trust:   review the installed hooks with `/hooks` inside Codex"
        ))?;
        status_line(format_args!(
            "launch:  `yinz agent codex -- [codex arguments]` (automatic for Yinz agent launches)"
        ))?;
    }
    if files.agent == HookAgent::Muse && wired_anywhere > 0 {
        status_line(format_args!(
            "launch:  `yinz agent muse -- [muse arguments]` (automatic for Yinz agent launches)"
        ))?;
    }
    Ok(())
}

/// Whether an agent started in this shell would reach a pane at all.
///
/// Outside [`status`] because it is about the shell rather than about an agent: a report
/// covering both of them would otherwise say it twice.
fn pane_line() -> Result<()> {
    // Same order the script keeps, and for the same reason: a remote pane carries both,
    // and only the token names the pane the report will actually reach.
    let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    if std::env::var_os(token).is_some_and(|value| !value.is_empty()) {
        status_line(format_args!(
            "pane:    this shell is a remote yinz pane, reporting through its broker"
        ))?;
        return Ok(());
    }
    match yinz_process::pane_context::current_pane() {
        Ok(Some((session, pane_id))) => status_line(format_args!(
            "pane:    this shell is yinz pane {pane_id} in {session}"
        ))?,
        Err(error) => status_line(format_args!("pane:    {error:#}"))?,
        Ok(None) => status_line(format_args!(
            "pane:    this shell is not in a yinz pane, so an agent started here \
             would report nothing"
        ))?,
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "unit/claude_reporting.rs"]
mod claude_reporting_tests;

#[cfg(all(test, unix))]
#[path = "unit/agent_hooks.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/agent_hook_upgrades.rs"]
mod upgrade_tests;
