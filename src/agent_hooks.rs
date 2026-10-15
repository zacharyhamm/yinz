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

/// The events, the timeout and the handler shape live in `yinz_types::agent`, so that this
/// installer and a spawned Claude Code pane's inline `--settings` cannot wire different events.
///
/// `pub(crate)` on the Claude table because the CLI tests count against it rather than against
/// a literal: it has grown once already, and a number here goes stale silently the next time
/// it does.
pub(crate) const CLAUDE_EVENTS: &[&str] = yinz_types::agent::CLAUDE_HOOK_EVENTS;
const CODEX_EVENTS: &[&str] = yinz_types::agent::CODEX_HOOK_EVENTS;
const OPENCODE_EVENTS: &[&str] = yinz_types::agent::OPENCODE_HOOK_EVENTS;
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
const SCRIPT_VERSION: u32 = 12;

/// The same discipline for the generated opencode plugin, which is one file rather than a
/// script plus a settings block. Bumped whenever [`opencode_plugin`] changes.
#[cfg(test)]
const OPENCODE_PLUGIN_VERSION: u32 = 6;

/// What marks an entry in the settings file as ours, and nothing else as ours.
///
/// Matched against the entry's `command`, so a person who moved the script or wrapped
/// it in something still has their entry replaced rather than duplicated.
const MARKER: &str = "yinz-hook.sh";

/// The opencode plugin's file name and the string that marks the file as ours.
const OPENCODE_PLUGIN_FILE: &str = "yinz.js";

fn legacy_agent(name: Option<&str>) -> Result<Option<HookAgent>> {
    name.map(|name| HookAgent::from_str(name, true).map_err(|error| anyhow!(error)))
        .transpose()
}

/// Provider metadata is evaluated from Scheme in a worker with no host authority.
/// The CLI, after explicit install/uninstall, performs only the declared file operations.
fn provider_command(command: &Hooks) -> Result<bool> {
    let (agent, project) = match command {
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
    let Some(agent) = agent else {
        return Ok(false);
    };
    if legacy_agent(Some(agent)).is_ok() && agent != "opencode" {
        return Ok(false);
    }
    if project {
        return Err(anyhow!(
            "Scheme provider hook files are installed at their declared home or config paths"
        ));
    }
    let location = if agent == "opencode" {
        "yinz:agent-opencode"
    } else {
        agent
    };
    let provider = yinz_server::inspect_agent_provider(location)?;
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
    Ok(true)
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

pub(crate) fn run(hooks: Hooks) -> Result<()> {
    if provider_command(&hooks)? {
        return Ok(());
    }
    // The default/all commands use the same provider-owned paths as an explicit install.
    let mut opencode_hooks = hooks.clone();
    match &mut opencode_hooks {
        Hooks::Install { agent, .. } | Hooks::Uninstall { agent, .. } | Hooks::Status { agent } => {
            *agent = Some("opencode".to_owned())
        },
    }
    match hooks {
        Hooks::Install {
            agent,
            project,
            project_dir,
            dry_run,
        } => {
            let yinz = std::env::current_exe().context("cannot find this yinz binary")?;
            for_each_agent(
                selected_agents(
                    legacy_agent(agent.as_deref())?,
                    project || project_dir.is_some(),
                )?,
                |agent| {
                    if agent == HookAgent::Opencode {
                        return provider_command(&opencode_hooks).map(|_| ());
                    }
                    install(
                        &Files::resolve(agent, project, project_dir.clone())?,
                        &yinz,
                        dry_run,
                    )
                },
            )
        },
        Hooks::Uninstall {
            agent,
            project,
            project_dir,
        } => for_each_agent(
            selected_agents(
                legacy_agent(agent.as_deref())?,
                project || project_dir.is_some(),
            )?,
            |agent| {
                if agent == HookAgent::Opencode {
                    return provider_command(&opencode_hooks).map(|_| ());
                }
                uninstall(&Files::resolve(agent, project, project_dir.clone())?)
            },
        ),
        Hooks::Status { agent } => {
            let mut first = true;
            let reported = for_each_agent(
                selected_agents(legacy_agent(agent.as_deref())?, false)?,
                |agent| {
                    // Two reports run together otherwise, and the second agent's heading reads
                    // as another line of the first one's.
                    if !std::mem::take(&mut first) {
                        status_line(format_args!(""))?;
                    }
                    if agent == HookAgent::Opencode {
                        return provider_command(&opencode_hooks).map(|_| ());
                    }
                    let files = if matches!(agent, HookAgent::Claude | HookAgent::Agy) {
                        Files::resolve(agent, true, None)
                            .or_else(|_| Files::resolve(agent, false, None))?
                    } else {
                        Files::resolve(agent, false, None)?
                    };
                    status(&files)
                },
            );
            // A fact about this shell rather than about either agent: said once, and said
            // even when a settings file could not be read.
            pane_line()?;
            reported
        },
    }
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
    if [ -n "$1" ]; then
        awk -v e="$1" '!done && /^[[:space:]]*{{/ {{ if ($0 !~ /"hook_event_name"/) {{ sub(/^[[:space:]]*{{/, "{{\"hook_event_name\":\"" e "\","); }} done=1 }} {{ print }}'
    else
        cat
    fi
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
    let yinz = yinz.display();
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

YINZ_BIN='{yinz}'
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

// OpenCode is a Scheme provider; the compatibility CLI spelling resolves its module.
fn opencode_provider() -> Result<yinz_types::agent::provider::AgentProvider> {
    yinz_server::inspect_agent_provider("yinz:agent-opencode")
}

#[cfg(test)]
fn opencode_plugin(yinz: &Path) -> String {
    let provider = opencode_provider().expect("builtin Scheme provider evaluates");
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
    if hooks.is_empty() {
        settings
            .as_object_mut()
            .expect("made an object above")
            .remove("hooks");
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
    let Some(root) = settings.as_object_mut() else {
        return settings;
    };
    root.retain(|_, block| {
        let Some(block) = block.as_object_mut() else {
            return true;
        };
        block.retain(|_, value| {
            let Some(items) = value.as_array_mut() else {
                return true;
            };
            items.retain_mut(|item| {
                if is_ours(item) {
                    return false;
                }
                if let Some(hooks) = item.get_mut("hooks").and_then(Value::as_array_mut) {
                    hooks.retain(|h| !is_ours(h));
                    return !hooks.is_empty();
                }
                true
            });
            !items.is_empty()
        });
        !block.is_empty()
    });
    settings
}

pub(crate) fn installed_agy_events(settings: &Value) -> Vec<String> {
    let Some(root) = settings.as_object() else {
        return Vec::new();
    };
    let mut wired = Vec::new();
    for block in root.values().filter_map(Value::as_object) {
        for (event, value) in block {
            let has_ours = match value {
                Value::Array(items) => items.iter().any(|item| {
                    if is_ours(item) {
                        return true;
                    }
                    item.get("hooks")
                        .and_then(Value::as_array)
                        .is_some_and(|hooks| hooks.iter().any(is_ours))
                }),
                _ => false,
            };
            if has_ours && !wired.contains(event) {
                wired.push(event.clone());
            }
        }
    }
    wired
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
        // For Claude Code, Codex and Antigravity the generated artefact is a script and the
        // wiring is a settings document beside it. opencode loads a plugin file, so the file
        // *is* the wiring: one path is both, and `settings()` is never asked for it.
        let (script, user_settings) = match agent {
            HookAgent::Claude => (root.join("yinz").join(MARKER), root.join("settings.json")),
            HookAgent::Codex => (root.join("yinz").join(MARKER), root.join("hooks.json")),
            HookAgent::Muse => (
                yinz_muse::hook_script_path(&root),
                yinz_muse::settings_path(&root),
            ),
            HookAgent::Opencode => {
                let plugin = root.join("plugin").join(OPENCODE_PLUGIN_FILE);
                (plugin.clone(), plugin)
            },
            // The opencode shape exactly: one file the agent loads, which is both the
            // artifact and the wiring, and `settings()` is never asked for it.
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
            HookAgent::Opencode => OPENCODE_EVENTS,
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
            HookAgent::Opencode => yinz_types::agent::OPENCODE_HOOK_TIMEOUT_SECONDS,
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

    /// The one-file installs answer nothing here: their wiring is the file itself, and
    /// `status` reads that rather than a settings document.
    fn is_one_file(&self) -> bool {
        matches!(self.agent, HookAgent::Opencode | HookAgent::Pi)
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
    let folder = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder to write into", path.display()))?;
    std::fs::create_dir_all(folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    let backed_up = path.exists();
    if backed_up {
        let backup = path.with_extension("json.bak");
        std::fs::copy(path, &backup).with_context(|| {
            format!("cannot back {} up to {}", path.display(), backup.display())
        })?;
    }
    let mut text = serde_json::to_string_pretty(settings).context("cannot serialize settings")?;
    text.push('\n');
    let temporary = path.with_extension("json.yinz-tmp");
    std::fs::write(&temporary, &text)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(backed_up)
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
/// The same temporary-and-rename as [`write_settings`], because the opencode plugin and
/// the pi extension are loaded by their own processes: a half-written file would be one
/// that does not parse.
fn write_generated(path: &Path, text: &str) -> Result<bool> {
    let folder = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder to write into", path.display()))?;
    std::fs::create_dir_all(folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    let backed_up = path.exists();
    let kind = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("js");
    if backed_up {
        let backup = path.with_extension(format!("{kind}.bak"));
        std::fs::copy(path, &backup).with_context(|| {
            format!("cannot back {} up to {}", path.display(), backup.display())
        })?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(folder)
        .with_context(|| format!("cannot create temporary file in {}", folder.display()))?;
    temporary
        .write_all(text.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))?;
    temporary
        .persist(path)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(backed_up)
}

/// opencode's install: one plugin file, and no settings merge, because the file *is* the
/// wiring.
fn install_opencode(files: &Files, yinz: &Path, dry_run: bool) -> Result<()> {
    let provider = opencode_provider()?;
    let hook = provider
        .hooks
        .first()
        .context("provider has no hook artifact")?;
    let content = yinz_runtime::agent::hook_files::render(
        hook,
        yinz,
        yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV,
    )?;
    yinz_runtime::agent::hook_files::read_owned(&files.script, &hook.marker)?;
    remove_legacy_opencode(&files.script, &hook.marker, dry_run)?;
    if dry_run {
        println!("would write {}:\n{content}", files.script.display());
    } else {
        yinz_runtime::agent::hook_files::install(&files.script, hook, &content)?;
        println!(
            "wrote {}; restart {} to load the hook",
            files.script.display(),
            provider.kind
        );
    }
    Ok(())
}

/// pi's install: one extension file, and no settings merge, because the file *is* the
/// wiring - the opencode shape, in pi's own extension directory.
///
/// The file is the same one a spawned pane is handed inline, so a machine with both sees
/// two copies in one pi process; the extension claims the process itself and the second
/// copy reports nothing, which is why the install never has to say "choose one".
fn install_pi(files: &Files, yinz: &Path, dry_run: bool) -> Result<()> {
    let extension = pi_extension(yinz);
    if let Some(existing) = read_generated(&files.script)? {
        if !existing.contains(yinz_runtime::agent::pi::PI_EXTENSION_MARKER) {
            return Err(anyhow!(
                "{} exists and was not written by yinz; move it aside first",
                files.script.display()
            ));
        }
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
    if files.agent == HookAgent::Opencode {
        return install_opencode(files, yinz, dry_run);
    }
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
    std::fs::write(&files.script, &script)
        .with_context(|| format!("cannot write {}", files.script.display()))?;
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
    if files.is_one_file() {
        return uninstall_opencode(files);
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

/// Remove the one generated file of a one-file agent - the opencode plugin or the pi
/// extension - if it is one we wrote.
fn uninstall_opencode(files: &Files) -> Result<()> {
    if files.agent == HookAgent::Opencode {
        let provider = opencode_provider()?;
        let hook = provider
            .hooks
            .first()
            .context("provider has no hook artifact")?;
        yinz_runtime::agent::hook_files::uninstall(&files.script, hook)?;
        println!("removed {}", files.script.display());
        return Ok(());
    }
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
    if files.agent == HookAgent::Opencode {
        let provider = opencode_provider()?;
        let hook = provider
            .hooks
            .first()
            .context("provider has no hook artifact")?;
        let binary = std::env::current_exe().context("find this yinz binary")?;
        return provider_status(&files.script, hook, &binary);
    }
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
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "yinz-yinz-hooks-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    /// Installing wires every event, and installing twice wires each of them once.
    /// Idempotence is by construction — the removal runs first — but the table has grown
    /// and this is what says the growth reached the settings file.
    #[test]
    fn installing_wires_every_event_and_installing_twice_changes_nothing() {
        let command = "/home/me/.claude/yinz/yinz-hook.sh";
        let once = install_into(json!({}), command).expect("installed");
        let mut wired = installed_events(&once);
        wired.sort();
        let mut expected: Vec<String> = CLAUDE_EVENTS.iter().map(|e| (*e).to_owned()).collect();
        expected.sort();
        assert_eq!(wired, expected);

        // The three the summary and the toast now hang off, named rather than merely
        // counted: dropping one would leave a feed stuck on `busy` or a question silent.
        for event in ["Stop", "StopFailure", "PermissionRequest"] {
            assert!(CLAUDE_EVENTS.contains(&event), "{event}");
        }

        let twice = install_into(once.clone(), command).expect("installed again");
        assert_eq!(twice, once);
    }

    #[test]
    fn generated_script_streams_a_large_payload_instead_of_putting_it_in_argv() {
        let temp = temp_dir("payload");
        let fake_yinz = temp.join("yinz");
        let hook = temp.join("yinz-hook.sh");
        let captured = temp.join("payload");
        let argc = temp.join("argc");
        std::fs::write(
            &fake_yinz,
            "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
        )
        .unwrap();
        make_executable(&fake_yinz).unwrap();
        std::fs::write(&hook, script(HookAgent::Claude, &fake_yinz)).unwrap();
        make_executable(&hook).unwrap();

        let payload = vec![b'x'; 200 * 1024];
        let mut child = Command::new(&hook)
            .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
            .env("YINZ_PANE_REF", "abcdef012345")
            .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
            .env("CAPTURE", &captured)
            .env("ARGC", &argc)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&payload).unwrap();
        assert!(child.wait().unwrap().success());

        assert_eq!(std::fs::read(captured).unwrap(), payload);
        assert_eq!(std::fs::read_to_string(argc).unwrap(), "3");
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn generated_codex_script_routes_without_daemon_pane_env_and_returns_neutral_json() {
        let temp = temp_dir("codex-payload");
        let fake_yinz = temp.join("yinz");
        let captured = temp.join("payload");
        let argc = temp.join("argc");
        std::fs::write(
            &fake_yinz,
            "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
        )
        .unwrap();
        make_executable(&fake_yinz).unwrap();
        let hook = temp.join("yinz-hook.sh");
        std::fs::write(&hook, script(HookAgent::Codex, &fake_yinz)).unwrap();
        make_executable(&hook).unwrap();

        let output = Command::new(&hook)
            .env_remove("YINZ_PANE_REF")
            .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
            .env("CAPTURE", &captured)
            .env("ARGC", &argc)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(b"{\"hook_event_name\":\"Stop\"}")?;
                child.wait_with_output()
            })
            .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        assert_eq!(std::fs::read_to_string(argc).unwrap(), "2");
        assert_eq!(
            std::fs::read_to_string(captured).unwrap(),
            "{\"hook_event_name\":\"Stop\"}"
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    /// Run the generated script with a given environment and say what it invoked.
    fn hook_argv(agent: HookAgent, name: &str, env: &[(&str, &str)]) -> (String, String) {
        let temp = temp_dir(name);
        let fake_yinz = temp.join("yinz");
        let captured = temp.join("payload");
        let argv = temp.join("argv");
        std::fs::write(
            &fake_yinz,
            "#!/bin/sh\nprintf '%s' \"$*\" > \"$ARGV\"\ncat > \"$CAPTURE\"\n",
        )
        .unwrap();
        make_executable(&fake_yinz).unwrap();
        let hook = temp.join("yinz-hook.sh");
        std::fs::write(&hook, script(agent, &fake_yinz)).unwrap();
        make_executable(&hook).unwrap();

        let mut command = Command::new(&hook);
        command
            .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
            .env_remove("YINZ_PANE_REF")
            .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
            .env("CAPTURE", &captured)
            .env("ARGV", &argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command
            .spawn()
            .and_then(|mut child| {
                // A script with nothing to report exits without reading stdin, which is the
                // behaviour half of these cases are here to check - so losing the race to it
                // and getting EPIPE is a pass, not a failure. On a quiet machine the write
                // almost always wins, which is why this only ever showed up under load.
                match child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(b"{\"hook_event_name\":\"Stop\"}")
                {
                    Ok(()) => {},
                    Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {},
                    Err(err) => return Err(err),
                }
                child.wait_with_output()
            })
            .unwrap();
        assert!(output.status.success());

        let argv = std::fs::read_to_string(&argv).unwrap_or_default();
        let payload = std::fs::read_to_string(&captured).unwrap_or_default();
        std::fs::remove_dir_all(temp).unwrap();
        (argv, payload)
    }

    #[test]
    fn a_remote_pane_reports_through_its_broker() {
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let (argv, payload) = hook_argv(HookAgent::Claude, "remote-claude", &[(token, "tok")]);
        assert_eq!(argv, "--pty-broker report claude");
        assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");

        let (argv, _) = hook_argv(HookAgent::Codex, "remote-codex", &[(token, "tok")]);
        assert_eq!(argv, "agent codex-report");

        let (argv, payload) = hook_argv(HookAgent::Muse, "remote-muse", &[(token, "tok")]);
        assert_eq!(argv, "agent muse-report");
        assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");

        let (argv, payload) = hook_argv(HookAgent::Agy, "remote-agy", &[(token, "tok")]);
        assert_eq!(argv, "--pty-broker report agy");
        assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");
    }

    /// The order the two probes are made in is the behaviour: a remote pane carries a
    /// `YINZ_PANE_ID` too, and it is the *asking* session's, on a machine with no such
    /// session. Reading it would send every event to whatever session runs on this box.
    #[test]
    fn a_remote_pane_ignores_the_pane_id_it_also_carries() {
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let (argv, _) = hook_argv(
            HookAgent::Claude,
            "remote-both",
            &[(token, "tok"), ("YINZ_PANE_ID", "7")],
        );
        assert_eq!(argv, "--pty-broker report claude");
    }

    /// An empty token is no token: `env -u` is not the only way a variable goes away.
    #[test]
    fn neither_a_pane_id_nor_a_token_reports_nothing() {
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let (argv, _) = hook_argv(HookAgent::Claude, "no-pane", &[(token, "")]);
        assert!(argv.is_empty(), "{argv:?}");
    }

    #[test]
    fn writing_new_settings_only_reports_a_backup_when_it_made_one() {
        let temp = temp_dir("backup");
        let settings = temp.join(".claude/settings.json");

        assert!(!write_settings(&settings, &json!({ "model": "opus" })).unwrap());
        assert!(!settings.with_extension("json.bak").exists());

        assert!(write_settings(&settings, &json!({ "model": "sonnet" })).unwrap());
        assert!(settings.with_extension("json.bak").exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    fn fake_yinz(temp: &Path) -> PathBuf {
        let fake = temp.join("yinz");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&fake).unwrap();
        fake
    }

    fn project_files(temp: &Path) -> Files {
        Files {
            agent: HookAgent::Claude,
            script: temp.join("home/.claude/yinz").join(MARKER),
            user_settings: temp.join("home/.claude/settings.json"),
            project: Some(temp.join("project")),
        }
    }

    fn codex_files(temp: &Path) -> Files {
        Files {
            agent: HookAgent::Codex,
            script: temp.join("home/.codex/yinz").join(MARKER),
            user_settings: temp.join("home/.codex/hooks.json"),
            project: None,
        }
    }

    fn opencode_files(temp: &Path) -> Files {
        let plugin = temp
            .join("home/.config/opencode/plugin")
            .join(OPENCODE_PLUGIN_FILE);
        Files {
            agent: HookAgent::Opencode,
            script: plugin.clone(),
            user_settings: plugin,
            project: None,
        }
    }

    fn pi_files(temp: &Path) -> Files {
        let extension = temp
            .join("home/.pi/agent/extensions")
            .join(yinz_runtime::agent::pi::PI_EXTENSION_FILE);
        Files {
            agent: HookAgent::Pi,
            script: extension.clone(),
            user_settings: extension,
            project: None,
        }
    }

    fn muse_files(temp: &Path) -> Files {
        Files {
            agent: HookAgent::Muse,
            script: temp.join("home/.config/muse/yinz").join(MARKER),
            user_settings: temp.join("home/.config/muse/settings.json"),
            project: None,
        }
    }

    fn agy_files(temp: &Path) -> Files {
        Files {
            agent: HookAgent::Agy,
            script: temp.join("home/.gemini/yinz").join(MARKER),
            user_settings: temp.join("home/.gemini/config/hooks.json"),
            project: None,
        }
    }

    fn agy_project_files(temp: &Path) -> Files {
        Files {
            agent: HookAgent::Agy,
            script: temp.join("home/.gemini/yinz").join(MARKER),
            user_settings: temp.join("home/.gemini/config/hooks.json"),
            project: Some(temp.join("project")),
        }
    }

    fn events_in(path: &Path) -> Vec<String> {
        installed_events(&read_settings(path).unwrap())
    }

    #[test]
    fn a_project_install_lands_in_settings_local_json() {
        let temp = temp_dir("local");
        let files = project_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        let local = temp.join("project/.claude/settings.local.json");
        assert_eq!(events_in(&local).len(), CLAUDE_EVENTS.len());
        assert!(
            !temp.join("project/.claude/settings.json").exists(),
            "the committed file is not ours to write"
        );
        assert!(!files.user_settings.exists());
        assert!(files.script.exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn a_codex_install_lands_in_the_user_hooks_file() {
        let temp = temp_dir("codex user's home");
        let files = codex_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        assert_eq!(events_in(&files.user_settings).len(), CODEX_EVENTS.len());
        assert!(files.script.exists());
        assert!(!temp.join("home/.claude").exists());

        let settings = read_settings(&files.user_settings).unwrap();
        let command = settings["hooks"][CODEX_EVENTS[0]][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .env("YINZ_PANE_REF", "abcdef012345")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        std::fs::remove_dir_all(temp).unwrap();
    }

    /// A Muse install wires the user settings file and guarantees the `schema_version`
    /// Muse refuses to run without: created at 1 where missing, kept where present,
    /// and left behind by uninstall, which only takes the wiring out.
    #[test]
    fn a_muse_install_lands_in_the_user_settings_file() {
        let temp = temp_dir("muse user's home");
        let files = muse_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        assert_eq!(events_in(&files.user_settings).len(), MUSE_EVENTS.len());
        assert!(files.script.exists());
        assert!(!temp.join("home/.claude").exists());
        let settings = read_settings(&files.user_settings).unwrap();
        assert_eq!(settings["schema_version"], json!(1));
        // The wired command reports with neither pane variable set, and stays silent:
        // Muse reads stdout as an optional decision object.
        let command = settings["hooks"][MUSE_EVENTS[0]][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .env_remove("YINZ_PANE_REF")
            .env_remove(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());

        // Installing twice wires each event once, and keeps an existing version and
        // every unrelated setting.
        std::fs::write(
            &files.user_settings,
            serde_json::to_string_pretty(&json!({
                "schema_version": 2,
                "model": "muse-spark",
            }))
            .unwrap(),
        )
        .unwrap();
        install(&files, &fake_yinz(&temp), false).unwrap();
        let settings = read_settings(&files.user_settings).unwrap();
        assert_eq!(settings["schema_version"], json!(2));
        assert_eq!(settings["model"], json!("muse-spark"));
        assert_eq!(events_in(&files.user_settings).len(), MUSE_EVENTS.len());

        uninstall(&files).unwrap();
        let settings = read_settings(&files.user_settings).unwrap();
        assert!(installed_events(&settings).is_empty());
        assert_eq!(settings["schema_version"], json!(2));
        assert!(!files.script.exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn an_agy_install_lands_in_the_user_hooks_file() {
        let temp = temp_dir("agy user home");
        let files = agy_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        assert_eq!(
            files
                .installed_events(&read_settings(&files.user_settings).unwrap())
                .len(),
            AGY_EVENTS.len()
        );
        assert!(files.script.exists());

        let settings = read_settings(&files.user_settings).unwrap();
        assert!(settings.get("hooks").is_some());
        let pre_tool = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(pre_tool["matcher"], ".*");
        assert!(pre_tool["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("PreToolUse"));

        let pre_inv = &settings["hooks"]["PreInvocation"][0];
        assert!(pre_inv["command"]
            .as_str()
            .unwrap()
            .contains("PreInvocation"));

        let output = Command::new("sh")
            .arg(&files.script)
            .arg("PreToolUse")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{\"decision\":\"ask\"}\n");

        let output_with_pane = Command::new("sh")
            .arg(&files.script)
            .arg("PreToolUse")
            .env("YINZ_PANE_REF", "abcdef012345")
            .output()
            .unwrap();
        assert!(output_with_pane.status.success());
        assert_eq!(output_with_pane.stdout, b"{\"decision\":\"ask\"}\n");

        let output = Command::new("sh")
            .arg(&files.script)
            .arg("Stop")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{\"decision\":\"stop\"}\n");

        let output = Command::new("sh")
            .arg(&files.script)
            .arg("PreInvocation")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");

        uninstall(&files).unwrap();
        assert!(files
            .installed_events(&read_settings(&files.user_settings).unwrap())
            .is_empty());
        assert!(!files.script.exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn an_agy_project_install_lands_in_agents_hooks_json() {
        let temp = temp_dir("agy project");
        let files = agy_project_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        let local = temp.join("project/.agents/hooks.json");
        assert_eq!(
            files
                .installed_events(&read_settings(&local).unwrap())
                .len(),
            AGY_EVENTS.len()
        );
        assert!(files.script.exists());
        assert!(!files.user_settings.exists());

        uninstall(&files).unwrap();
        assert!(files
            .installed_events(&read_settings(&local).unwrap())
            .is_empty());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn installing_agy_hooks_preserves_existing_handlers_and_uninstall_retains_them() {
        let command = "/home/me/.gemini/yinz/yinz-hook.sh";
        let original = json!({
            "other_block": {
                "SomeEvent": [{ "command": "echo other" }]
            },
            "hooks": {
                "Stop": [
                    { "command": "/usr/local/bin/custom-stop.sh", "timeout": 10 }
                ],
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "command": "/usr/local/bin/tool-guard.sh", "timeout": 5 }
                        ]
                    }
                ],
                "CustomEvent": [
                    { "command": "/usr/local/bin/custom.sh" }
                ]
            }
        });

        let installed = install_agy_into(original.clone(), command).expect("installed agy hooks");

        // Other block and unrelated custom event are completely untouched
        assert_eq!(installed["other_block"], original["other_block"]);
        assert_eq!(
            installed["hooks"]["CustomEvent"],
            original["hooks"]["CustomEvent"]
        );

        // Custom Stop command is retained at index 0, our handler appended at index 1
        let stop = installed["hooks"]["Stop"].as_array().expect("array");
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["command"], "/usr/local/bin/custom-stop.sh");
        assert_eq!(stop[1]["command"], format!("{command} Stop"));

        // Custom PreToolUse matcher is retained at index 0, our .* matcher appended at index 1
        let pre_tool = installed["hooks"]["PreToolUse"].as_array().expect("array");
        assert_eq!(pre_tool.len(), 2);
        assert_eq!(pre_tool[0]["matcher"], "Bash");
        assert_eq!(pre_tool[1]["matcher"], ".*");
        assert_eq!(
            pre_tool[1]["hooks"][0]["command"],
            format!("{command} PreToolUse")
        );

        // Every other AGY event has our handler as a 1-element array
        let wired = installed_agy_events(&installed);
        assert_eq!(wired.len(), AGY_EVENTS.len());
        for event in AGY_EVENTS {
            assert!(wired.contains(&event.to_string()), "{event} not wired");
        }

        // Reinstalling is idempotent: neither custom hooks nor our hooks are duplicated
        let twice = install_agy_into(installed.clone(), command).expect("installed twice");
        assert_eq!(twice, installed);

        // Uninstalling removes only our hooks, leaving all custom handlers and blocks intact
        let uninstalled = uninstall_agy_from(installed);
        assert_eq!(uninstalled, original);
        assert!(installed_agy_events(&uninstalled).is_empty());
    }

    #[test]
    fn installing_agy_hooks_rejects_settings_shapes_it_cannot_merge() {
        assert!(install_agy_into(json!({ "hooks": [] }), "yinz-hook.sh").is_err());
        assert!(install_agy_into(
            json!({ "hooks": { "Stop": { "not": "an array" } } }),
            "yinz-hook.sh"
        )
        .is_err());
    }

    /// opencode's wiring is one plugin file and no settings merge, so installing twice is
    /// one file either way and uninstalling takes it with it.
    #[test]
    fn an_opencode_install_writes_a_versioned_plugin_and_uninstall_removes_it() {
        let temp = temp_dir("opencode-install");
        let files = opencode_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        let plugin = read_generated(&files.script).unwrap().unwrap();
        assert!(
            plugin.contains(&format!("yinz-opencode-plugin v{OPENCODE_PLUGIN_VERSION}")),
            "{plugin}"
        );
        assert!(plugin.contains("\"agent-report\""), "{plugin}");
        assert!(plugin.contains("\"--pty-broker\""), "{plugin}");
        assert!(
            !plugin.contains("__YINZ_BIN__")
                && !plugin.contains("__TOKEN_ENV__")
                && !plugin.contains("__VERSION__"),
            "every substitution is filled in"
        );
        assert!(!temp.join("home/.config/opencode/settings.json").exists());

        install(&files, &fake_yinz(&temp), false).unwrap();
        assert_eq!(read_generated(&files.script).unwrap().unwrap(), plugin);

        uninstall(&files).unwrap();
        assert!(read_generated(&files.script).unwrap().is_none());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn opencode_plugin_reports_in_order_and_tracks_turn_text() {
        let temp = tempfile::tempdir().unwrap();
        let plugin = temp.path().join("yinz.mjs");
        let harness = temp.path().join("regressions.mjs");
        std::fs::write(&plugin, opencode_plugin(Path::new("/test/yinz"))).unwrap();
        std::fs::write(&harness, include_str!("unit/opencode_plugin.mjs")).unwrap();
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        for remote in [false, true] {
            let mut command = Command::new("node");
            command
                .arg(&harness)
                .arg(&plugin)
                .env("YINZ_PANE_REF", "abcdef012345");
            if remote {
                command.env(token, "test-token");
            } else {
                command.env_remove(token);
            }
            let output = command
                .output()
                .expect("Node is required for the OpenCode plugin tests");
            assert!(
                output.status.success(),
                "OpenCode plugin regression failed (remote={remote}):\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    /// A plugin file we did not write is somebody else's: neither install nor uninstall may
    /// touch it, and both say so.
    #[test]
    fn opencode_leaves_a_plugin_file_it_did_not_write_alone() {
        let temp = temp_dir("opencode-foreign");
        let files = opencode_files(&temp);
        std::fs::create_dir_all(files.script.parent().unwrap()).unwrap();
        std::fs::write(&files.script, "// somebody else's plugin\n").unwrap();

        assert!(install(&files, &fake_yinz(&temp), false).is_err());
        assert!(uninstall(&files).is_err());
        assert_eq!(
            read_generated(&files.script).unwrap().unwrap(),
            "// somebody else's plugin\n"
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    /// The binary path is a JavaScript string, not a shell word: a folder name with a space
    /// or a quote in it must not write a plugin opencode cannot parse.
    #[test]
    fn the_opencode_plugin_bakes_a_json_quoted_binary_path() {
        let plugin = opencode_plugin(Path::new("/opt/my yinz/yinz"));
        assert!(plugin.contains("\"/opt/my yinz/yinz\""), "{plugin}");
    }

    /// pi's wiring is one extension file in pi's own extension directory, and it is the
    /// very file a spawned pane is handed inline - so installing twice writes the same
    /// bytes, and uninstalling takes the file with it.
    #[test]
    fn pi_extension_reports_lifecycle_and_recovers_across_reload() {
        let temp = tempfile::tempdir().unwrap();
        let extension = temp.path().join("reporter.mts");
        let harness = temp.path().join("harness.mjs");
        std::fs::write(&extension, pi_extension(Path::new("/test/yinz"))).unwrap();
        std::fs::write(&harness, include_str!("unit/pi_extension.mjs")).unwrap();
        for remote in [false, true] {
            let mut command = Command::new("node");
            command
                .arg(&harness)
                .arg(&extension)
                .env("YINZ_PANE_ID", "1");
            if remote {
                command.env("YINZ_REMOTE_PANE_TOKEN", "test-token");
            } else {
                command.env_remove("YINZ_REMOTE_PANE_TOKEN");
            }
            let output = command
                .output()
                .expect("Node is required for Pi extension tests");
            assert!(
                output.status.success(),
                "Pi reporter (remote={remote}): {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn claude_install_directory_honors_override_and_empty_fallback() {
        assert_eq!(
            agent_root(
                HookAgent::Claude,
                Some("/custom/claude".into()),
                None,
                None,
                None,
                || panic!("override does not need HOME")
            )
            .unwrap(),
            PathBuf::from("/custom/claude")
        );
        for value in [None, Some(OsString::new())] {
            assert_eq!(
                agent_root(HookAgent::Claude, value, None, None, None, || Ok(
                    PathBuf::from("/home/test")
                ))
                .unwrap(),
                PathBuf::from("/home/test/.claude")
            );
        }
    }

    #[test]
    fn claude_hook_files_resolve_inside_the_override() {
        if yinz_test_support::reexec_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", temp.path());
        let files = Files::resolve(HookAgent::Claude, false, None).unwrap();
        assert_eq!(files.script, temp.path().join("yinz").join(MARKER));
        assert_eq!(files.user_settings, temp.path().join("settings.json"));
    }

    #[test]
    fn pi_install_directory_override_does_not_need_home() {
        for path in ["/custom/pi", "relative/pi", "~user/pi", "./~/pi"] {
            assert_eq!(
                agent_root(HookAgent::Pi, None, None, Some(path.into()), None, || {
                    panic!("override does not need HOME")
                })
                .unwrap(),
                PathBuf::from(path)
            );
        }
        assert_eq!(
            agent_root(HookAgent::Pi, None, None, Some("".into()), None, || Ok(
                PathBuf::from("/home/test")
            ))
            .unwrap(),
            PathBuf::from("/home/test/.pi/agent")
        );
    }

    #[test]
    fn pi_install_directory_override_expands_tilde() {
        for (path, expected) in [
            ("~", "/home/test"),
            ("~/.config/pi", "/home/test/.config/pi"),
        ] {
            assert_eq!(
                agent_root(HookAgent::Pi, None, None, Some(path.into()), None, || Ok(
                    PathBuf::from("/home/test")
                ))
                .unwrap(),
                PathBuf::from(expected)
            );
            assert!(
                agent_root(HookAgent::Pi, None, None, Some(path.into()), None, || Err(
                    anyhow!("HOME is not set")
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn a_pi_install_writes_the_shared_extension_and_uninstall_removes_it() {
        let temp = temp_dir("pi-install");
        let files = pi_files(&temp);
        install(&files, &fake_yinz(&temp), false).unwrap();

        let extension = read_generated(&files.script).unwrap().unwrap();
        assert!(
            extension.contains(&format!(
                "{} v{}",
                yinz_runtime::agent::pi::PI_EXTENSION_MARKER,
                yinz_runtime::agent::pi::PI_EXTENSION_VERSION
            )),
            "{extension}"
        );
        // The routes the spawned copy keeps too: the pane report locally, the broker
        // remotely, and nothing at all outside a yinz pane.
        assert!(extension.contains("\"--kind\", \"pi\""), "{extension}");
        assert!(extension.contains("--pty-broker"), "{extension}");
        assert!(extension.contains("YINZ_PANE_ID"), "{extension}");
        assert!(
            !extension.contains("__YINZ_BIN__")
                && !extension.contains("__TOKEN_ENV__")
                && !extension.contains("__VERSION__"),
            "every substitution is filled in"
        );
        // pi needs no settings: nothing but the extension was written.
        assert!(!temp.join("home/.pi/agent/settings.json").exists());

        install(&files, &fake_yinz(&temp), false).unwrap();
        assert_eq!(read_generated(&files.script).unwrap().unwrap(), extension);

        uninstall(&files).unwrap();
        assert!(read_generated(&files.script).unwrap().is_none());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn pi_install_dry_run_preserves_files_and_real_install_keeps_a_backup() {
        let temp = tempfile::tempdir().unwrap();
        let files = pi_files(temp.path());
        install(&files, Path::new("/bin/yinz"), true).unwrap();
        assert!(!files.script.exists());
        install(&files, Path::new("/bin/yinz"), false).unwrap();
        let original = std::fs::read_to_string(&files.script).unwrap();
        install(&files, Path::new("/other/yinz"), true).unwrap();
        assert_eq!(std::fs::read_to_string(&files.script).unwrap(), original);
        install(&files, Path::new("/other/yinz"), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(files.script.with_extension("ts.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn pi_leaves_an_extension_it_did_not_write_alone() {
        let temp = temp_dir("pi-foreign");
        let files = pi_files(&temp);
        std::fs::create_dir_all(files.script.parent().unwrap()).unwrap();
        std::fs::write(&files.script, "// somebody else's extension\n").unwrap();

        assert!(install(&files, &fake_yinz(&temp), false).is_err());
        assert!(uninstall(&files).is_err());
        assert_eq!(
            read_generated(&files.script).unwrap().unwrap(),
            "// somebody else's extension\n"
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    /// The default install covers every agent, and pi has no project install: `.pi/`
    /// is repo content pi itself will not load without the user's trust.
    #[test]
    fn pi_is_part_of_the_default_install_and_never_a_project_one() {
        let every = selected_agents(None, false).unwrap();
        assert!(every.contains(&HookAgent::Pi));
        assert_eq!(
            selected_agents(None, true).unwrap(),
            vec![HookAgent::Claude]
        );
        assert!(selected_agents(Some(HookAgent::Pi), true).is_err());
        assert_eq!(
            agent_root(HookAgent::Pi, None, None, None, None, || Ok(PathBuf::from(
                "/home/me"
            )))
            .unwrap(),
            PathBuf::from("/home/me/.pi/agent")
        );
        assert!(Files::resolve(HookAgent::Pi, true, None).is_err());
    }

    #[test]
    fn a_custom_codex_home_does_not_require_home() {
        assert_eq!(
            agent_root(
                HookAgent::Codex,
                None,
                Some(OsString::from("/custom/codex")),
                None,
                None,
                || Err(anyhow!("HOME is unavailable"))
            )
            .unwrap(),
            PathBuf::from("/custom/codex")
        );
    }

    /// `export CODEX_HOME="$UNSET"` used to resolve to `""`, and an install into `""` writes
    /// `hooks.json` and `yinz/` into whichever folder the person was standing in. An empty
    /// variable is an unset one.
    #[test]
    fn an_empty_codex_home_falls_back_to_home_rather_than_to_the_current_folder() {
        assert_eq!(
            agent_root(
                HookAgent::Codex,
                None,
                Some(OsString::new()),
                None,
                None,
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/home/me/.codex")
        );
        assert!(agent_root(
            HookAgent::Codex,
            None,
            Some(OsString::new()),
            None,
            None,
            || Err(anyhow!("HOME is unavailable"))
        )
        .is_err());
    }

    /// opencode's config root is the one that follows `XDG_CONFIG_HOME`, and its empty
    /// value is an unset one the same way `CODEX_HOME`'s is.
    #[test]
    fn opencode_honours_xdg_config_home_and_falls_back_to_dot_config() {
        assert_eq!(
            agent_root(
                HookAgent::Opencode,
                None,
                None,
                None,
                Some(OsString::from("/xdg")),
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/xdg/opencode")
        );
        assert_eq!(
            agent_root(
                HookAgent::Opencode,
                None,
                None,
                None,
                Some(OsString::new()),
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/home/me/.config/opencode")
        );
        assert_eq!(
            agent_root(HookAgent::Opencode, None, None, None, None, || Ok(
                PathBuf::from("/home/me")
            ))
            .unwrap(),
            PathBuf::from("/home/me/.config/opencode")
        );
    }

    /// Muse's config root follows `XDG_CONFIG_HOME` the same way, under `muse`.
    #[test]
    fn muse_honours_xdg_config_home_and_falls_back_to_dot_config() {
        assert_eq!(
            agent_root(
                HookAgent::Muse,
                None,
                None,
                None,
                Some(OsString::from("/xdg")),
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/xdg/muse")
        );
        assert_eq!(
            agent_root(
                HookAgent::Muse,
                None,
                None,
                None,
                Some(OsString::new()),
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/home/me/.config/muse")
        );
        assert_eq!(
            agent_root(HookAgent::Muse, None, None, None, None, || Ok(
                PathBuf::from("/home/me")
            ))
            .unwrap(),
            PathBuf::from("/home/me/.config/muse")
        );
    }

    /// The default form of every one of these commands does both agents, so a broken
    /// `~/.claude/settings.json` - Claude goes first - must not leave Codex unwired and
    /// unmentioned. It still ends in an error, because something really did fail.
    #[test]
    fn one_agents_failure_does_not_stop_the_other() {
        let mut ran = Vec::new();
        let result = for_each_agent(vec![HookAgent::Claude, HookAgent::Codex], |agent| {
            ran.push(agent);
            match agent {
                HookAgent::Claude => Err(anyhow!("settings.json is not valid JSON")),
                _ => Ok(()),
            }
        });
        assert_eq!(ran, vec![HookAgent::Claude, HookAgent::Codex]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Claude Code failed - see above"
        );

        assert!(for_each_agent(vec![HookAgent::Claude, HookAgent::Codex], |_| Ok(())).is_ok());
    }

    #[test]
    fn user_commands_default_to_every_agent_but_project_commands_to_claude() {
        assert_eq!(
            selected_agents(None, false).unwrap(),
            vec![
                HookAgent::Claude,
                HookAgent::Codex,
                HookAgent::Opencode,
                HookAgent::Muse,
                HookAgent::Agy,
                HookAgent::Pi,
            ]
        );
        assert_eq!(
            selected_agents(None, true).unwrap(),
            vec![HookAgent::Claude]
        );
        assert_eq!(
            selected_agents(Some(HookAgent::Agy), true).unwrap(),
            vec![HookAgent::Agy]
        );
        assert!(selected_agents(Some(HookAgent::Codex), true).is_err());
        assert!(selected_agents(Some(HookAgent::Opencode), true).is_err());
        assert!(selected_agents(Some(HookAgent::Muse), true).is_err());
        assert!(selected_agents(Some(HookAgent::All), true).is_err());
    }

    #[test]
    fn a_project_install_moves_entries_out_of_the_committed_file() {
        let temp = temp_dir("legacy");
        let files = project_files(&temp);
        let legacy = temp.join("project/.claude/settings.json");
        let command = files.script.display().to_string();
        let old = install_into(json!({ "model": "opus" }), &command).unwrap();
        write_settings(&legacy, &old).unwrap();

        install(&files, &fake_yinz(&temp), false).unwrap();

        assert!(
            events_in(&legacy).is_empty(),
            "the old install is gone from the committed file"
        );
        assert_eq!(read_settings(&legacy).unwrap()["model"], "opus");
        assert_eq!(
            events_in(&temp.join("project/.claude/settings.local.json")).len(),
            CLAUDE_EVENTS.len()
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn a_project_uninstall_keeps_the_script_a_user_install_uses() {
        let temp = temp_dir("shared-script");
        let yinz = fake_yinz(&temp);
        let project = project_files(&temp);
        let user = Files {
            project: None,
            ..project_files(&temp)
        };
        install(&user, &yinz, false).unwrap();
        install(&project, &yinz, false).unwrap();

        uninstall(&project).unwrap();
        assert!(events_in(&project.settings()).is_empty());
        assert_eq!(events_in(&user.user_settings).len(), CLAUDE_EVENTS.len());
        assert!(
            project.script.exists(),
            "the user install still points at it"
        );

        uninstall(&user).unwrap();
        assert!(!user.script.exists());
        std::fs::remove_dir_all(temp).unwrap();
    }
}

#[cfg(test)]
mod upgrade_tests {
    use super::*;
    #[test]
    fn reinstall_replaces_legacy_reports_and_keeps_other_handlers() {
        for events in [CLAUDE_EVENTS, CODEX_EVENTS, MUSE_EVENTS] {
            let mut settings = json!({"hooks": {}, "model": "custom"});
            for event in events {
                settings["hooks"][*event] = json!([{"hooks": [
                    {"command": "'/home/me/hocus/hocus-hook.sh'", "type": "command"},
                    {"command": "user-handler", "type": "command"}
                ]}]);
            }
            let installed = install_events_into(settings, "yinz-hook.sh", events, 10).unwrap();
            let repeated =
                install_events_into(installed.clone(), "yinz-hook.sh", events, 10).unwrap();
            assert_eq!(installed, repeated);
            assert_eq!(installed["model"], "custom");
            for event in events {
                let text = installed["hooks"][*event].to_string();
                assert!(!text.contains("hocus-hook"));
                assert!(text.contains("user-handler"));
                assert_eq!(text.matches("yinz-hook.sh").count(), 1);
            }
        }
    }
    #[test]
    fn reinstall_removes_only_the_owned_legacy_opencode_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("yinz.js");
        let legacy = dir.path().join("hocus.js");
        std::fs::write(&legacy, "// hocus-opencode-plugin\n").unwrap();
        remove_legacy_opencode(&destination, "yinz-opencode-plugin", true).unwrap();
        assert!(legacy.exists());
        remove_legacy_opencode(&destination, "yinz-opencode-plugin", false).unwrap();
        assert!(!legacy.exists());
        std::fs::write(&legacy, "// user plugin\n").unwrap();
        remove_legacy_opencode(&destination, "yinz-opencode-plugin", false).unwrap();
        assert!(legacy.exists());
    }
}
