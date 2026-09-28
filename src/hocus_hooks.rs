//! `yinz hocus hooks` — wiring coding-agent hooks to the agent plugins.
//!
//! `hocus-agentd` decides whether an agent pane is busy, idle or blocked on the user
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
//! pipe's name and its argument, which are `hocus_proto::wire`'s, and the shape of the
//! settings block, which the JSON agents share.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use {yinz_cli::args::Hocus, yinz_cli::args::HocusHookAgent, yinz_cli::args::HocusHooks};

/// The events, the timeout and the handler shape live in `yinz_types::agent`, so that this
/// installer and a spawned Claude Code pane's inline `--settings` cannot wire different events.
///
/// `pub(crate)` on the Claude table because the CLI tests count against it rather than against
/// a literal: it has grown once already, and a number here goes stale silently the next time
/// it does.
pub(crate) const CLAUDE_EVENTS: &[&str] = yinz_types::agent::CLAUDE_HOOK_EVENTS;
const CODEX_EVENTS: &[&str] = yinz_types::agent::CODEX_HOOK_EVENTS;
const OPENCODE_EVENTS: &[&str] = yinz_types::agent::OPENCODE_HOOK_EVENTS;

/// Bumped whenever [`script`] changes, or whenever the set of events wired around it does, so
/// `status` can say the installed copy is old. v4 was the seventeen-event table; v5 is the one
/// that names the agent it was written for; v6 is the one with a remote route, picking it by
/// the pane token rather than reporting to a session that need not exist on this machine.
///
/// A missed bump is silent and total: `status` calls the old script current, and every event
/// on a machine installed before the change goes nowhere.
const SCRIPT_VERSION: u32 = 9;

/// The same discipline for the generated opencode plugin, which is one file rather than a
/// script plus a settings block. Bumped whenever [`opencode_plugin`] changes.
const OPENCODE_PLUGIN_VERSION: u32 = 4;

/// What marks an entry in the settings file as ours, and nothing else as ours.
///
/// Matched against the entry's `command`, so a person who moved the script or wrapped
/// it in something still has their entry replaced rather than duplicated.
const MARKER: &str = "hocus-hook.sh";

/// The opencode plugin's file name and the string that marks the file as ours.
const OPENCODE_PLUGIN_FILE: &str = "hocus.js";
const OPENCODE_MARKER: &str = "hocus-opencode-plugin";

pub(crate) fn hocus(command: Hocus) -> Result<()> {
    let hooks = match command {
        Hocus::Codex { command, args } => {
            std::process::exit(yinz_codex::launch(&command, &args)?);
        },
        Hocus::CodexReport => {
            return yinz_codex::report(&std::env::current_exe().context("find this yinz binary")?);
        },
        Hocus::Hooks(hooks) => hooks,
        Hocus::Skills(yinz_cli::args::HocusSkills::Install {
            agent,
            dry_run,
            force,
        }) => {
            return for_each_agent(selected_agents(Some(agent), false)?, |agent| {
                let root = agent_root(
                    agent,
                    std::env::var_os("CODEX_HOME"),
                    std::env::var_os("XDG_CONFIG_HOME"),
                    home,
                )?;
                crate::hocus_skills::install(
                    &root.join("skills/yinz-notes-todos/SKILL.md"),
                    dry_run,
                    force,
                )
            });
        },
    };
    match hooks {
        HocusHooks::Install {
            agent,
            project,
            project_dir,
            dry_run,
        } => {
            let yinz = std::env::current_exe().context("cannot find this yinz binary")?;
            for_each_agent(
                selected_agents(agent, project || project_dir.is_some())?,
                |agent| {
                    install(
                        &Files::resolve(agent, project, project_dir.clone())?,
                        &yinz,
                        dry_run,
                    )
                },
            )
        },
        HocusHooks::Uninstall {
            agent,
            project,
            project_dir,
        } => for_each_agent(
            selected_agents(agent, project || project_dir.is_some())?,
            |agent| uninstall(&Files::resolve(agent, project, project_dir.clone())?),
        ),
        HocusHooks::Status { agent } => {
            let mut first = true;
            let reported = for_each_agent(selected_agents(agent, false)?, |agent| {
                // Two reports run together otherwise, and the second agent's heading reads
                // as another line of the first one's.
                if !std::mem::take(&mut first) {
                    println!();
                }
                let files = if agent == HocusHookAgent::Claude {
                    Files::resolve(agent, true, None)
                        .or_else(|_| Files::resolve(agent, false, None))?
                } else {
                    Files::resolve(agent, false, None)?
                };
                status(&files)
            });
            // A fact about this shell rather than about either agent: said once, and said
            // even when a settings file could not be read.
            pane_line();
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
fn for_each_agent(
    agents: Vec<HocusHookAgent>,
    mut run: impl FnMut(HocusHookAgent) -> Result<()>,
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
fn agent_name(agent: HocusHookAgent) -> &'static str {
    match agent {
        HocusHookAgent::Claude => "Claude Code",
        HocusHookAgent::Codex => "Codex",
        HocusHookAgent::Opencode => "opencode",
        HocusHookAgent::All => unreachable!("all is expanded before an agent is named"),
    }
}

fn selected_agents(agent: Option<HocusHookAgent>, project: bool) -> Result<Vec<HocusHookAgent>> {
    let agent = agent.unwrap_or(if project {
        HocusHookAgent::Claude
    } else {
        HocusHookAgent::All
    });
    if project && agent != HocusHookAgent::Claude {
        return Err(anyhow!(
            "only Claude Code has a machine-local project file; use --agent claude with --project"
        ));
    }
    Ok(match agent {
        HocusHookAgent::Claude => vec![HocusHookAgent::Claude],
        HocusHookAgent::Codex => vec![HocusHookAgent::Codex],
        HocusHookAgent::Opencode => vec![HocusHookAgent::Opencode],
        HocusHookAgent::All => vec![
            HocusHookAgent::Claude,
            HocusHookAgent::Codex,
            HocusHookAgent::Opencode,
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
fn script(agent: HocusHookAgent, yinz: &Path) -> String {
    if agent == HocusHookAgent::Codex {
        let yinz = yinz.to_string_lossy().replace('\'', "'\\''");
        return format!(
            r#"#!/bin/sh
# hocus-hook v{SCRIPT_VERSION} - generated by `yinz hocus hooks install`.
# Codex hooks run in a shared daemon. The conversation id, not that daemon's
# inherited environment, selects the pane registered by `yinz hocus codex`.
YINZ_BIN='{yinz}'
[ -x "$YINZ_BIN" ] || YINZ_BIN=yinz
"$YINZ_BIN" hocus codex-report >/dev/null 2>&1
printf '{{}}\n'
exit 0
"#
        );
    }
    let yinz = yinz.display();
    let (name, wire_kind, kind, no_pane, output, output_rule) = match agent {
        HocusHookAgent::Claude => (
            "Claude Code",
            "claude",
            "",
            "exit 0",
            "",
            "it never writes to stdout, which some events inject into context",
        ),
        HocusHookAgent::Codex => unreachable!("Codex uses the conversation router above"),
        HocusHookAgent::Opencode => {
            unreachable!("opencode runs a plugin, not a shell script; see `opencode_plugin`")
        },
        HocusHookAgent::All => unreachable!("all is expanded before a script is generated"),
    };
    // Spelled from the constant the broker reads, so the two halves of the remote route
    // cannot drift apart: a variable name that does not match is silently no route at all.
    let token_env = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    format!(
        r#"#!/bin/sh
# hocus-hook v{SCRIPT_VERSION} - generated by `yinz hocus hooks install`. Do not edit;
# `yinz hocus hooks install` rewrites this file, and `--uninstall` removes it.
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

# The remote token is tested first, and that order is the whole point: a remote pane
# carries a ${token_env} *and* a $YINZ_PANE_ID, and the pane id it carries is the
# asking session's, on a machine that has no such session. Reading it here would send
# every event to whatever session happens to run on this box, under a pane id that
# means something else. Neither set: not a yinz pane at all, and nothing to report to.
[ -n "${token_env}" ] || [ -n "$YINZ_PANE_ID" ] || {{ {no_pane}; }}

YINZ_BIN='{yinz}'
[ -x "$YINZ_BIN" ] || YINZ_BIN=yinz

# Either way the event travels on stdin rather than as an argument, clear of execve's
# per-argument limit.
if [ -n "${token_env}" ]; then
    # A remote pane - ssh or Firecracker. There is no session on this machine to tell,
    # so the event goes to the broker that owns the pty, which names the pane by the
    # token it put here and relays it on the pane's own channel to the pane's server.
    "$YINZ_BIN" --pty-broker report {wire_kind} >/dev/null 2>&1
else
    # A local pane: the pane is $YINZ_PANE_ID, which the action reads itself.
    "$YINZ_BIN" action agent-report --hook-json{kind} >/dev/null 2>&1
fi
{output}

exit 0
"#
    )
}

/// The opencode plugin `install` writes, with this binary's own path baked in.
///
/// opencode is extended with a JS module rather than a shell hook, so this is the one
/// agent whose wiring is a file it loads instead of an entry it runs. The plugin is a
/// thin transport: it forwards opencode's own events and interception hooks, and the
/// reading of them is `yinz_types::agent::opencode`, in the session, so opencode and
/// the other two agents cannot disagree about what an event meant.
///
/// The path is baked for the same reason the shell script's is: the server inherits the
/// environment the pane's shell was started with, which is not always one where `yinz`
/// is on the path. There is no fallback to a `yinz` from PATH because opencode gives
/// the plugin no shell; a moved binary is an `install` away from being rewritten, and
/// `status` is what says the path has stopped existing.
fn opencode_plugin(yinz: &Path) -> String {
    let yinz = serde_json::to_string(&yinz.display().to_string()).expect("a path serializes");
    // Spelled from the constant the broker reads, so the two halves of the remote route
    // cannot drift apart: a variable name that does not match is silently no route at all.
    let token_env = serde_json::to_string(yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV)
        .expect("a name serializes");
    OPENCODE_PLUGIN_TEMPLATE
        .replace("__YINZ_BIN__", &yinz)
        .replace("__TOKEN_ENV__", &token_env)
        .replace("__VERSION__", &OPENCODE_PLUGIN_VERSION.to_string())
}

/// [`opencode_plugin`]'s body, with the three substitutions left to be filled in.
///
/// A template rather than a `format!` because the body is JavaScript, whose braces would
/// all have to be doubled; and because a missed substitution has to fail a test rather
/// than write a plugin opencode cannot parse.
const OPENCODE_PLUGIN_TEMPLATE: &str = r#"// hocus-opencode-plugin v__VERSION__ - generated by `yinz hocus hooks install`.
// Do not edit; `yinz hocus hooks install` rewrites this file, and `--uninstall`
// removes it.
//
// opencode loads this plugin from `~/.config/opencode/plugin/hocus.js`. It forwards
// opencode's own event stream - and the interception hooks that are not events - to
// whoever owns the pane: `yinz action agent-report --hook-json --kind opencode` for a
// local pane, `yinz --pty-broker report opencode` for a remote one. The session reads
// the events (`yinz-types/src/agent/opencode.rs`) and carries what they mean on the
// pane, so the pane reports its own busy/waiting/idle state instead of being guessed at
// from its screen.
//
// Two rules this file keeps, both of which matter:
//   - it never throws. A report that fails must not interfere with the agent it is only
//     reporting on.
//   - callbacks never wait. Reports queue in order, starting after the prior child closes.

import { spawn } from "node:child_process"

const YINZ_BIN = __YINZ_BIN__
const PANE_TOKEN_ENV = __TOKEN_ENV__
const PANE_TOKEN = process.env[PANE_TOKEN_ENV]
const PANE_ID = process.env.YINZ_PANE_ID

let reports = Promise.resolve()

function report(payload, isParent) {
  // The remote token is tested first, and that order is the whole point: a remote pane
  // carries a token *and* a pane id, and the pane id it carries is the asking session's,
  // on a machine that has no such session. Neither set: not a yinz pane at all, and
  // nothing to report to.
  if (!PANE_TOKEN && !PANE_ID) return
  const args = PANE_TOKEN
    ? ["--pty-broker", "report", "opencode"]
    : ["action", "agent-report", "--hook-json", "--kind", "opencode"]
  let serialized
  try {
    serialized = JSON.stringify(payload)
  } catch {
    return
  }
  reports = reports.then(async () => {
    if (!await isParent()) return
    return new Promise((resolve) => {
      let child
      try {
        child = spawn(YINZ_BIN, args, { stdio: ["pipe", "ignore", "ignore"] })
      } catch {
        resolve()
        return
      }
      // Node emits close after error, including when spawning fails.
      child.on("error", () => {})
      child.on("close", resolve)
      child.stdin.on("error", () => {})
      try {
        child.stdin.end(serialized)
      } catch {
        child.kill()
      }
    })
  }).catch(() => {})
}

export const Hocus = async ({ directory, client }) => {
  // The folder the server is working in. Stamped on every event because an opencode
  // record is complete every time it is set, and an event that omitted the folder
  // would clear one an earlier event had set.
  const cwd = directory
  // Cache whether each session is a parent, including sessions resumed after restart.
  // Classify in report order so metadata lookups cannot reorder pane state changes.
  const sessions = new Map()
  const textParts = new Map()
  const lastAssistant = new Map()

  const send = (type, properties, info) => {
    const sessionID = properties.sessionID
    const parent = info ? !info.parentID : undefined
    report({ type, properties: { cwd, ...properties } }, async () => {
      if (!sessionID) return false
      if (parent !== undefined && !sessions.has(sessionID)) sessions.set(sessionID, parent)
      if (!sessions.has(sessionID)) {
        const { data, error } = await client.session.get({ path: { id: sessionID } })
        // Failed lookups suppress this report, but the next event can retry.
        if (error || data?.id !== sessionID) return false
        sessions.set(sessionID, !data.parentID)
      }
      const isParent = sessions.get(sessionID)
      if (type === "session.deleted") sessions.delete(sessionID)
      return isParent
    })
  }

  const forget = (sessionID) => {
    textParts.delete(sessionID)
    lastAssistant.delete(sessionID)
  }

  return {
    event: async ({ event }) => {
      const p = event.properties ?? {}
      switch (event.type) {
        case "session.created":
        case "session.deleted":
          send(event.type, { sessionID: p.info?.id }, p.info)
          if (event.type === "session.deleted") forget(p.info?.id)
          return
        case "session.idle": {
          const last = lastAssistant.get(p.sessionID)
          send("session.idle", {
            sessionID: p.sessionID,
            lastAssistantMessage: last?.text,
            turnID: last?.messageID,
          })
          return
        }
        case "session.error": {
          const last = lastAssistant.get(p.sessionID)
          send("session.error", {
            sessionID: p.sessionID,
            lastAssistantMessage: last?.text,
            turnID: last?.messageID,
          })
          return
        }
        case "session.compacted":
          send("session.compacted", { sessionID: p.sessionID })
          return
        case "message.part.updated": {
          const part = p.part
          if (part?.type !== "text" || part.synthetic || part.ignored) return
          const messages = textParts.get(part.sessionID) ?? new Map()
          const parts = messages.get(part.messageID) ?? new Map()
          parts.set(part.id, part.text ?? "")
          messages.set(part.messageID, parts)
          textParts.set(part.sessionID, messages)
          return
        }
        case "message.updated": {
          const info = p.info
          if (info?.role !== "assistant" || !info.time?.completed) return
          const parts = textParts.get(info.sessionID)?.get(info.id)
          const text = parts ? [...parts.values()].join("") : ""
          // Once consumed, the parts are dropped: the message's text lives on in
          // `lastAssistant` until the next turn replaces it.
          textParts.get(info.sessionID)?.delete(info.id)
          if (!text) return
          lastAssistant.set(info.sessionID, { messageID: info.id, text })
          send("assistant.message", {
            sessionID: info.sessionID,
            messageID: info.id,
            text,
          })
          return
        }
        case "permission.asked":
        case "permission.updated":
          send(event.type, { ...p })
          return
        case "permission.replied":
          send("permission.replied", { sessionID: p.sessionID })
          return
        default:
          return
      }
    },
    "chat.message": async (input, output) => {
      lastAssistant.delete(input.sessionID)
      textParts.delete(input.sessionID)
      const text = (output.parts ?? [])
        .filter((part) => part.type === "text" && !part.synthetic && !part.ignored)
        .map((part) => part.text ?? "")
        .join(" ")
        .trim()
      send("chat.message", {
        sessionID: input.sessionID,
        messageID: output.message?.id,
        prompt: text || undefined,
      })
    },
    "tool.execute.before": async (input, output) => {
      send("tool.execute.before", {
        sessionID: input.sessionID,
        callID: input.callID,
        tool: input.tool,
        args: output.args,
      })
    },
    "tool.execute.after": async (input) => {
      send("tool.execute.after", {
        sessionID: input.sessionID,
        callID: input.callID,
        tool: input.tool,
        args: input.args,
      })
    },
  }
}
"#;

/// One handler entry, as both agents' settings spell it.
fn handler(command: &str, timeout: u64) -> Value {
    yinz_types::agent::hook_handler(command, timeout)
}

/// Whether one handler entry is one we wrote.
fn is_ours(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(MARKER))
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

/// A variable that is set to nothing is one that is not set.
///
/// `export CODEX_HOME="$SOMETHING_UNSET"` leaves `Some("")` behind, and an empty root turns
/// every path built on it into a relative one - which is to say, into whichever folder the
/// person happened to be standing in when they ran `install`.
fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn home() -> Result<PathBuf> {
    non_empty(std::env::var_os("HOME"))
        .ok_or_else(|| anyhow!("HOME is not set, so there is no agent config to install into"))
}

fn agent_root(
    agent: HocusHookAgent,
    codex_home: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    Ok(match agent {
        HocusHookAgent::Claude => home()?.join(".claude"),
        HocusHookAgent::Codex => match non_empty(codex_home) {
            Some(root) => root,
            None => home()?.join(".codex"),
        },
        // opencode reads its config from `$XDG_CONFIG_HOME/opencode`, and falls back to
        // `~/.config/opencode` where that is unset, the way opencode itself does.
        HocusHookAgent::Opencode => match non_empty(xdg_config_home) {
            Some(root) => root.join("opencode"),
            None => home()?.join(".config/opencode"),
        },
        HocusHookAgent::All => unreachable!("all is expanded before resolving files"),
    })
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// The files a command touches, resolved once from the flags and the environment so that
/// everything below is plain path work - and runs against a temporary folder in a test.
pub(crate) struct Files {
    pub(crate) agent: HocusHookAgent,
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
    fn resolve(
        agent: HocusHookAgent,
        project: bool,
        project_dir: Option<PathBuf>,
    ) -> Result<Files> {
        if matches!(agent, HocusHookAgent::Codex | HocusHookAgent::Opencode)
            && (project || project_dir.is_some())
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
            std::env::var_os("CODEX_HOME"),
            std::env::var_os("XDG_CONFIG_HOME"),
            home,
        )?;
        // For Claude Code and Codex the generated artefact is a script and the wiring is a
        // settings document beside it. opencode loads a plugin file, so the file *is* the
        // wiring: one path is both, and `settings()` is never asked for it.
        let (script, user_settings) = match agent {
            HocusHookAgent::Claude => (root.join("hocus").join(MARKER), root.join("settings.json")),
            HocusHookAgent::Codex => (root.join("hocus").join(MARKER), root.join("hooks.json")),
            HocusHookAgent::Opencode => {
                let plugin = root.join("plugin").join(OPENCODE_PLUGIN_FILE);
                (plugin.clone(), plugin)
            },
            HocusHookAgent::All => unreachable!("all is expanded before resolving files"),
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
            Some(dir) => {
                debug_assert_eq!(self.agent, HocusHookAgent::Claude);
                dir.join(".claude").join("settings.local.json")
            },
            None => self.user_settings.clone(),
        }
    }

    /// The committed `.claude/settings.json` an older yinz wrote a project install into.
    fn legacy_project_settings(&self) -> Option<PathBuf> {
        (self.agent == HocusHookAgent::Claude)
            .then_some(self.project.as_ref())
            .flatten()
            .map(|dir| dir.join(".claude").join("settings.json"))
    }

    fn events(&self) -> &'static [&'static str] {
        match self.agent {
            HocusHookAgent::Claude => CLAUDE_EVENTS,
            HocusHookAgent::Codex => CODEX_EVENTS,
            HocusHookAgent::Opencode => OPENCODE_EVENTS,
            HocusHookAgent::All => unreachable!("all is expanded before resolving files"),
        }
    }

    fn name(&self) -> &'static str {
        agent_name(self.agent)
    }

    fn timeout(&self) -> u64 {
        match self.agent {
            HocusHookAgent::Claude => yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
            HocusHookAgent::Codex => yinz_types::agent::CODEX_HOOK_TIMEOUT_SECONDS,
            HocusHookAgent::Opencode => yinz_types::agent::OPENCODE_HOOK_TIMEOUT_SECONDS,
            HocusHookAgent::All => unreachable!("all is expanded before resolving files"),
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
    let temporary = path.with_extension("json.hocus-tmp");
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
/// The same temporary-and-rename as [`write_settings`], because the opencode plugin is
/// loaded by a running server: a half-written file would be a plugin that does not parse.
fn write_generated(path: &Path, text: &str) -> Result<bool> {
    let folder = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder to write into", path.display()))?;
    std::fs::create_dir_all(folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    let backed_up = path.exists();
    if backed_up {
        let backup = path.with_extension("js.bak");
        std::fs::copy(path, &backup).with_context(|| {
            format!("cannot back {} up to {}", path.display(), backup.display())
        })?;
    }
    let temporary = path.with_extension("js.hocus-tmp");
    std::fs::write(&temporary, text)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(backed_up)
}

/// opencode's install: one plugin file, and no settings merge, because the file *is* the
/// wiring.
fn install_opencode(files: &Files, yinz: &Path, dry_run: bool) -> Result<()> {
    let plugin = opencode_plugin(yinz);
    if let Some(existing) = read_generated(&files.script)? {
        if !existing.contains(OPENCODE_MARKER) {
            return Err(anyhow!(
                "{} exists and was not written by yinz; move it aside first",
                files.script.display()
            ));
        }
    }
    if dry_run {
        println!("would write {}:\n", files.script.display());
        println!("{plugin}");
        return Ok(());
    }
    let backed_up = write_generated(&files.script, &plugin)?;
    println!("wrote {}", files.script.display());
    println!(
        "wired {} events through the opencode plugin",
        files.events().len()
    );
    if backed_up {
        println!(
            "kept the previous plugin as {}",
            files.script.with_extension("js.bak").display()
        );
    }
    println!("restart opencode so it loads the plugin");
    println!("run `yinz hocus hooks status` to check");
    Ok(())
}

pub(crate) fn install(files: &Files, yinz: &Path, dry_run: bool) -> Result<()> {
    if files.agent == HocusHookAgent::Opencode {
        return install_opencode(files, yinz, dry_run);
    }
    let script = script(files.agent, yinz);
    let settings_path = files.settings();
    let settings = read_settings(&settings_path)?;
    let command = shell_quote(&files.script);
    let updated = install_events_into(settings, &command, files.events(), files.timeout())?;
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
        println!(
            "{}",
            serde_json::to_string_pretty(updated.get("hooks").unwrap_or(&Value::Null))?
        );
        if let Some((path, settings)) = legacy {
            println!(
                "\nwould move {} events out of {}",
                installed_events(&settings).len(),
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
        let moved = installed_events(&settings).len();
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
    println!("run `yinz hocus hooks status` to check");
    if files.agent == HocusHookAgent::Codex {
        println!("open `/hooks` inside Codex to review and trust the installed hooks");
        println!(
            "launch with `yinz hocus codex -- [codex arguments]` for shared-daemon pane tracking"
        );
    } else {
        println!("open `/hooks` inside Claude Code to inspect the installed hooks");
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
    if files.agent == HocusHookAgent::Opencode {
        return uninstall_opencode(files);
    }
    let settings_path = files.settings();
    let settings = read_settings(&settings_path)?;
    let removed = installed_events(&settings).len();
    if removed > 0 {
        write_settings(&settings_path, &uninstall_from(settings))?;
    }
    println!("removed {removed} events from {}", settings_path.display());
    if let Some((path, settings)) = files.wired_legacy_project_settings()? {
        let removed = installed_events(&settings).len();
        write_settings(&path, &uninstall_from(settings))?;
        println!("removed {removed} events from {}", path.display());
    }

    // The script is one file for the whole machine: a project uninstall leaves it to the
    // user-level install that still points at it.
    if files.project.is_some()
        && !installed_events(&read_settings(&files.user_settings)?).is_empty()
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

/// Remove the opencode plugin, if it is one we wrote.
fn uninstall_opencode(files: &Files) -> Result<()> {
    match read_generated(&files.script)? {
        Some(text) if text.contains(OPENCODE_MARKER) => {
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
    if files.agent == HocusHookAgent::Opencode {
        println!("{}:", files.name());
        match read_generated(&files.script)? {
            Some(text)
                if text.contains(&format!("hocus-opencode-plugin v{OPENCODE_PLUGIN_VERSION}")) =>
            {
                println!("plugin:  {} (current)", files.script.display());
            },
            Some(_) => println!(
                "plugin:  {} (older than this yinz - run `install` again)",
                files.script.display()
            ),
            None => println!("plugin:  {} (missing)", files.script.display()),
        }
        return Ok(());
    }
    println!("{}:", files.name());
    match std::fs::read_to_string(&files.script) {
        Ok(text) if text.contains(&format!("hocus-hook v{SCRIPT_VERSION} ")) => {
            println!("script:  {} (current)", files.script.display());
        },
        Ok(_) => println!(
            "script:  {} (older than this yinz - run `install` again)",
            files.script.display()
        ),
        Err(_) => println!("script:  {} (missing)", files.script.display()),
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
                println!("{heading:<9}{} ({error:#})", path.display());
                continue;
            },
        };
        let events = installed_events(&settings);
        wired_anywhere += usize::from(!events.is_empty());
        println!(
            "{heading:<9}{} - {} of {} events wired",
            path.display(),
            events.len(),
            files.events().len()
        );
        if label == "legacy" {
            println!(
                "            an older project install; `install --project` moves it to \
                 settings.local.json"
            );
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
            println!("            missing: {}", missing.join(", "));
            println!("            run `install` again to add them");
        }
        // The two ways a correctly written hook never runs.
        if settings.get("disableAllHooks") == Some(&Value::Bool(true)) {
            println!("            disableAllHooks is on: nothing here runs");
        }
        if settings.get("allowManagedHooksOnly") == Some(&Value::Bool(true)) {
            println!("            allowManagedHooksOnly is on: nothing here runs");
        }
    }
    if wired_anywhere > 1 {
        println!(
            "note:    wired in more than one file. Claude Code merges hooks rather than \
             overriding, so every event fires twice"
        );
    }

    if files.agent == HocusHookAgent::Codex && wired_anywhere > 0 {
        println!("trust:   review the installed hooks with `/hooks` inside Codex");
        println!(
            "launch:  `yinz hocus codex -- [codex arguments]` (automatic for Yinz agent launches)"
        );
    }
    Ok(())
}

/// Whether an agent started in this shell would reach a pane at all.
///
/// Outside [`status`] because it is about the shell rather than about an agent: a report
/// covering both of them would otherwise say it twice.
fn pane_line() {
    // Same order the script keeps, and for the same reason: a remote pane carries both,
    // and only the token names the pane the report will actually reach.
    let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
    if std::env::var_os(token).is_some_and(|value| !value.is_empty()) {
        println!("pane:    this shell is a remote yinz pane, reporting through its broker");
        return;
    }
    match std::env::var("YINZ_PANE_ID") {
        Ok(pane_id) => println!("pane:    this shell is yinz pane {pane_id}"),
        Err(_) => println!(
            "pane:    this shell is not in a yinz pane, so an agent started here \
             would report nothing"
        ),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "yinz-hocus-hooks-{name}-{}-{}",
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
        let command = "/home/me/.claude/hocus/hocus-hook.sh";
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
        let hook = temp.join("hocus-hook.sh");
        let captured = temp.join("payload");
        let argc = temp.join("argc");
        std::fs::write(
            &fake_yinz,
            "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
        )
        .unwrap();
        make_executable(&fake_yinz).unwrap();
        std::fs::write(&hook, script(HocusHookAgent::Claude, &fake_yinz)).unwrap();
        make_executable(&hook).unwrap();

        let payload = vec![b'x'; 200 * 1024];
        let mut child = Command::new(&hook)
            .env("YINZ_PANE_ID", "7")
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
        let hook = temp.join("hocus-hook.sh");
        std::fs::write(&hook, script(HocusHookAgent::Codex, &fake_yinz)).unwrap();
        make_executable(&hook).unwrap();

        let output = Command::new(&hook)
            .env_remove("YINZ_PANE_ID")
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
    fn hook_argv(agent: HocusHookAgent, name: &str, env: &[(&str, &str)]) -> (String, String) {
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
        let hook = temp.join("hocus-hook.sh");
        std::fs::write(&hook, script(agent, &fake_yinz)).unwrap();
        make_executable(&hook).unwrap();

        let mut command = Command::new(&hook);
        command
            .env_remove("YINZ_PANE_ID")
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

    /// A pane on the far side of an ssh or Firecracker transport has no session to tell,
    /// so the script hands the event to that machine's broker instead.
    #[test]
    fn a_remote_pane_reports_through_its_broker() {
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let (argv, payload) = hook_argv(HocusHookAgent::Claude, "remote-claude", &[(token, "tok")]);
        assert_eq!(argv, "--pty-broker report claude");
        assert_eq!(payload, "{\"hook_event_name\":\"Stop\"}");

        let (argv, _) = hook_argv(HocusHookAgent::Codex, "remote-codex", &[(token, "tok")]);
        assert_eq!(argv, "hocus codex-report");
    }

    /// The order the two probes are made in is the behaviour: a remote pane carries a
    /// `YINZ_PANE_ID` too, and it is the *asking* session's, on a machine with no such
    /// session. Reading it would send every event to whatever session runs on this box.
    #[test]
    fn a_remote_pane_ignores_the_pane_id_it_also_carries() {
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let (argv, _) = hook_argv(
            HocusHookAgent::Claude,
            "remote-both",
            &[(token, "tok"), ("YINZ_PANE_ID", "7")],
        );
        assert_eq!(argv, "--pty-broker report claude");
    }

    /// An empty token is no token: `env -u` is not the only way a variable goes away.
    #[test]
    fn neither_a_pane_id_nor_a_token_reports_nothing() {
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        let (argv, _) = hook_argv(HocusHookAgent::Claude, "no-pane", &[(token, "")]);
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
            agent: HocusHookAgent::Claude,
            script: temp.join("home/.claude/hocus").join(MARKER),
            user_settings: temp.join("home/.claude/settings.json"),
            project: Some(temp.join("project")),
        }
    }

    fn codex_files(temp: &Path) -> Files {
        Files {
            agent: HocusHookAgent::Codex,
            script: temp.join("home/.codex/hocus").join(MARKER),
            user_settings: temp.join("home/.codex/hooks.json"),
            project: None,
        }
    }

    fn opencode_files(temp: &Path) -> Files {
        let plugin = temp
            .join("home/.config/opencode/plugin")
            .join(OPENCODE_PLUGIN_FILE);
        Files {
            agent: HocusHookAgent::Opencode,
            script: plugin.clone(),
            user_settings: plugin,
            project: None,
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
            .env("YINZ_PANE_ID", "7")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        std::fs::remove_dir_all(temp).unwrap();
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
            plugin.contains(&format!("hocus-opencode-plugin v{OPENCODE_PLUGIN_VERSION}")),
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
        let plugin = temp.path().join("hocus.mjs");
        let harness = temp.path().join("regressions.mjs");
        std::fs::write(&plugin, opencode_plugin(Path::new("/test/yinz"))).unwrap();
        std::fs::write(&harness, include_str!("unit/opencode_plugin.mjs")).unwrap();
        let token = yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV;
        for remote in [false, true] {
            let mut command = Command::new("node");
            command.arg(&harness).arg(&plugin).env("YINZ_PANE_ID", "1");
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

    #[test]
    fn a_custom_codex_home_does_not_require_home() {
        assert_eq!(
            agent_root(
                HocusHookAgent::Codex,
                Some(OsString::from("/custom/codex")),
                None,
                || Err(anyhow!("HOME is unavailable"))
            )
            .unwrap(),
            PathBuf::from("/custom/codex")
        );
    }

    /// `export CODEX_HOME="$UNSET"` used to resolve to `""`, and an install into `""` writes
    /// `hooks.json` and `hocus/` into whichever folder the person was standing in. An empty
    /// variable is an unset one.
    #[test]
    fn an_empty_codex_home_falls_back_to_home_rather_than_to_the_current_folder() {
        assert_eq!(
            agent_root(HocusHookAgent::Codex, Some(OsString::new()), None, || Ok(
                PathBuf::from("/home/me")
            ))
            .unwrap(),
            PathBuf::from("/home/me/.codex")
        );
        assert!(
            agent_root(HocusHookAgent::Codex, Some(OsString::new()), None, || Err(
                anyhow!("HOME is unavailable")
            ))
            .is_err()
        );
    }

    /// opencode's config root is the one that follows `XDG_CONFIG_HOME`, and its empty
    /// value is an unset one the same way `CODEX_HOME`'s is.
    #[test]
    fn opencode_honours_xdg_config_home_and_falls_back_to_dot_config() {
        assert_eq!(
            agent_root(
                HocusHookAgent::Opencode,
                None,
                Some(OsString::from("/xdg")),
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/xdg/opencode")
        );
        assert_eq!(
            agent_root(
                HocusHookAgent::Opencode,
                None,
                Some(OsString::new()),
                || Ok(PathBuf::from("/home/me"))
            )
            .unwrap(),
            PathBuf::from("/home/me/.config/opencode")
        );
        assert_eq!(
            agent_root(HocusHookAgent::Opencode, None, None, || Ok(PathBuf::from(
                "/home/me"
            )))
            .unwrap(),
            PathBuf::from("/home/me/.config/opencode")
        );
    }

    /// The default form of every one of these commands does both agents, so a broken
    /// `~/.claude/settings.json` - Claude goes first - must not leave Codex unwired and
    /// unmentioned. It still ends in an error, because something really did fail.
    #[test]
    fn one_agents_failure_does_not_stop_the_other() {
        let mut ran = Vec::new();
        let result = for_each_agent(
            vec![HocusHookAgent::Claude, HocusHookAgent::Codex],
            |agent| {
                ran.push(agent);
                match agent {
                    HocusHookAgent::Claude => Err(anyhow!("settings.json is not valid JSON")),
                    _ => Ok(()),
                }
            },
        );
        assert_eq!(ran, vec![HocusHookAgent::Claude, HocusHookAgent::Codex]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Claude Code failed - see above"
        );

        assert!(
            for_each_agent(vec![HocusHookAgent::Claude, HocusHookAgent::Codex], |_| Ok(
                ()
            ))
            .is_ok()
        );
    }

    #[test]
    fn user_commands_default_to_every_agent_but_project_commands_to_claude() {
        assert_eq!(
            selected_agents(None, false).unwrap(),
            vec![
                HocusHookAgent::Claude,
                HocusHookAgent::Codex,
                HocusHookAgent::Opencode
            ]
        );
        assert_eq!(
            selected_agents(None, true).unwrap(),
            vec![HocusHookAgent::Claude]
        );
        assert!(selected_agents(Some(HocusHookAgent::Codex), true).is_err());
        assert!(selected_agents(Some(HocusHookAgent::Opencode), true).is_err());
        assert!(selected_agents(Some(HocusHookAgent::All), true).is_err());
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
