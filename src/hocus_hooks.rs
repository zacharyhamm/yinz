//! `zellij hocus hooks` — wiring coding-agent hooks to the agent plugins.
//!
//! `hocus-agentd` decides whether an agent pane is busy, idle or blocked on the user
//! by reading its screen every five seconds and hashing what it finds, and — for the
//! last of those three — by asking a model about the pane's tail. That is the only
//! thing that works for an agent which says nothing about itself, and it is a guess
//! bought with about fifteen seconds and an HTTP round trip.
//!
//! Claude Code and Codex say. This command installs a small script as an agent hook so
//! that an agent running in a zellij pane reports its own state, and the daemon
//! stops guessing about that pane. Everything here is local file work: it writes a
//! script and merges some entries into a settings file, and never needs a session.
//!
//! The two halves that must agree with something else are marked where they appear:
//! the pipe's name and its argument, which are `hocus_proto::wire`'s, and the shape
//! of the settings block, which both agents share.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use zellij_utils::cli::{Hocus, HocusHookAgent, HocusHooks};

/// The events, the timeout and the handler shape live in `zellij_utils::agent`, so that this
/// installer and a spawned Claude Code pane's inline `--settings` cannot wire different events.
///
/// `pub(crate)` on the Claude table because the CLI tests count against it rather than against
/// a literal: it has grown once already, and a number here goes stale silently the next time
/// it does.
pub(crate) const CLAUDE_EVENTS: &[&str] = zellij_utils::agent::CLAUDE_HOOK_EVENTS;
const CODEX_EVENTS: &[&str] = zellij_utils::agent::CODEX_HOOK_EVENTS;

/// Bumped whenever [`script`] changes, or whenever the set of events wired around it does, so
/// `status` can say the installed copy is old. v4 was the seventeen-event table; v5 is the one
/// that names the agent it was written for.
const SCRIPT_VERSION: u32 = 5;

/// What marks an entry in the settings file as ours, and nothing else as ours.
///
/// Matched against the entry's `command`, so a person who moved the script or wrapped
/// it in something still has their entry replaced rather than duplicated.
const MARKER: &str = "hocus-hook.sh";

pub(crate) fn hocus(command: Hocus) -> Result<()> {
    let Hocus::Hooks(hooks) = command;
    match hooks {
        HocusHooks::Install {
            agent,
            project,
            project_dir,
            dry_run,
        } => {
            let zellij = std::env::current_exe().context("cannot find this zellij binary")?;
            for_each_agent(
                selected_agents(agent, project || project_dir.is_some())?,
                |agent| {
                    install(
                        &Files::resolve(agent, project, project_dir.clone())?,
                        &zellij,
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
/// The two are wired independently - an unparseable `~/.claude/settings.json` has nothing to
/// say about Codex - and the default form of each of these commands does both. Stopping at the
/// first error would leave the second agent untouched and unmentioned, which is the one thing a
/// command that promises "both" must not do. Every failure is named where it happened, and the
/// run still ends non-zero, so a setup script still notices.
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
            "Codex hooks are user-level only; use --agent claude with --project"
        ));
    }
    Ok(match agent {
        HocusHookAgent::Claude => vec![HocusHookAgent::Claude],
        HocusHookAgent::Codex => vec![HocusHookAgent::Codex],
        HocusHookAgent::All => vec![HocusHookAgent::Claude, HocusHookAgent::Codex],
    })
}

/// The script an agent runs, with this binary's own path baked in.
///
/// Baked rather than looked up because a hook inherits the environment `claude` was
/// started with, which is not always one where `zellij` is on the path — a desktop
/// launcher, a login shell that sets the path in a file an interactive shell reads.
/// The lookup stays as the fallback for the case the baked path stops existing,
/// which is what an upgrade that moves the binary looks like.
fn script(agent: HocusHookAgent, zellij: &Path) -> String {
    let zellij = zellij.display();
    let (name, kind, no_pane, output, output_rule) = match agent {
        HocusHookAgent::Claude => (
            "Claude Code",
            "",
            "exit 0",
            "",
            "it never writes to stdout, which some events inject into context",
        ),
        HocusHookAgent::Codex => (
            "Codex",
            " --kind codex",
            "printf '{}\\n'; exit 0",
            "printf '{}\\n'",
            "it writes only neutral JSON, which Codex requires from Stop hooks",
        ),
        HocusHookAgent::All => unreachable!("all is expanded before a script is generated"),
    };
    format!(
        r#"#!/bin/sh
# hocus-hook v{SCRIPT_VERSION} - generated by `zellij hocus hooks install`. Do not edit;
# `zellij hocus hooks install` rewrites this file, and `--uninstall` removes it.
#
# {name} runs this on each wired hook event, with that event's JSON on stdin.
# It hands the JSON to the session, which carries what it means on the pane and
# broadcasts the event itself to the plugins (`zellij action agent-report`): the
# pane reports its own busy/waiting/idle state instead of being guessed at from its
# screen, and what a record has no room for - the agent's own last message, the prompt
# behind it - reaches whoever wants it.
#
# Two rules this file keeps, both of which matter:
#   - {output_rule}.
#   - it always exits 0. A hook that fails must not interfere with the agent it is
#     only reporting on.

# Not in a zellij pane, so there is no pane for the daemon to attach this to.
[ -n "$ZELLIJ_PANE_ID" ] || {{ {no_pane}; }}

ZELLIJ_BIN='{zellij}'
[ -x "$ZELLIJ_BIN" ] || ZELLIJ_BIN=zellij

# The event travels on stdin rather than as an argument, clear of execve's per-argument
# limit; the pane is $ZELLIJ_PANE_ID, which the action reads itself.
"$ZELLIJ_BIN" action agent-report --hook-json{kind} >/dev/null 2>&1
{output}

exit 0
"#
    )
}

/// One handler entry, as both agents' settings spell it.
fn handler(command: &str, timeout: u64) -> Value {
    zellij_utils::agent::hook_handler(command, timeout)
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
        zellij_utils::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
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
    home: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    Ok(match agent {
        HocusHookAgent::Claude => home()?.join(".claude"),
        HocusHookAgent::Codex => match non_empty(codex_home) {
            Some(root) => root,
            None => home()?.join(".codex"),
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
        if agent == HocusHookAgent::Codex && (project || project_dir.is_some()) {
            return Err(anyhow!("Codex hooks are user-level only"));
        }
        let project = match project_dir {
            Some(dir) => Some(dir),
            None if project => {
                Some(std::env::current_dir().context("cannot tell which project this is")?)
            },
            None => None,
        };
        let root = agent_root(agent, std::env::var_os("CODEX_HOME"), home)?;
        let user_settings = match agent {
            HocusHookAgent::Claude => root.join("settings.json"),
            HocusHookAgent::Codex => root.join("hooks.json"),
            HocusHookAgent::All => unreachable!("all is expanded before resolving files"),
        };
        Ok(Files {
            agent,
            script: root.join("hocus").join(MARKER),
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

    /// The committed `.claude/settings.json` an older zellij wrote a project install into.
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
            HocusHookAgent::All => unreachable!("all is expanded before resolving files"),
        }
    }

    fn name(&self) -> &'static str {
        agent_name(self.agent)
    }

    fn timeout(&self) -> u64 {
        match self.agent {
            HocusHookAgent::Claude => zellij_utils::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
            HocusHookAgent::Codex => zellij_utils::agent::CODEX_HOOK_TIMEOUT_SECONDS,
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

pub(crate) fn install(files: &Files, zellij: &Path, dry_run: bool) -> Result<()> {
    let script = script(files.agent, zellij);
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
    println!("run `zellij hocus hooks status` to check");
    if files.agent == HocusHookAgent::Codex {
        println!("open `/hooks` inside Codex to review and trust the installed hooks");
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

/// What is wired, and the things that would quietly stop it working.
fn status(files: &Files) -> Result<()> {
    println!("{}:", files.name());
    match std::fs::read_to_string(&files.script) {
        Ok(text) if text.contains(&format!("hocus-hook v{SCRIPT_VERSION} ")) => {
            println!("script:  {} (current)", files.script.display());
        },
        Ok(_) => println!(
            "script:  {} (older than this zellij - run `install` again)",
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
    }
    Ok(())
}

/// Whether an agent started in this shell would reach a pane at all.
///
/// Outside [`status`] because it is about the shell rather than about an agent: a report
/// covering both of them would otherwise say it twice.
fn pane_line() {
    match std::env::var("ZELLIJ_PANE_ID") {
        Ok(pane_id) => println!("pane:    this shell is zellij pane {pane_id}"),
        Err(_) => println!(
            "pane:    this shell is not in a zellij pane, so an agent started here \
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
            "zellij-hocus-hooks-{name}-{}-{}",
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
        let fake_zellij = temp.join("zellij");
        let hook = temp.join("hocus-hook.sh");
        let captured = temp.join("payload");
        let argc = temp.join("argc");
        std::fs::write(
            &fake_zellij,
            "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
        )
        .unwrap();
        make_executable(&fake_zellij).unwrap();
        std::fs::write(&hook, script(HocusHookAgent::Claude, &fake_zellij)).unwrap();
        make_executable(&hook).unwrap();

        let payload = vec![b'x'; 200 * 1024];
        let mut child = Command::new(&hook)
            .env("ZELLIJ_PANE_ID", "7")
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
    fn generated_codex_script_names_codex_and_returns_neutral_json() {
        let temp = temp_dir("codex-payload");
        let fake_zellij = temp.join("zellij");
        let captured = temp.join("payload");
        let argc = temp.join("argc");
        std::fs::write(
            &fake_zellij,
            "#!/bin/sh\nprintf '%s' \"$#\" > \"$ARGC\"\ncat > \"$CAPTURE\"\n",
        )
        .unwrap();
        make_executable(&fake_zellij).unwrap();
        let hook = temp.join("hocus-hook.sh");
        std::fs::write(&hook, script(HocusHookAgent::Codex, &fake_zellij)).unwrap();
        make_executable(&hook).unwrap();

        let output = Command::new(&hook)
            .env("ZELLIJ_PANE_ID", "7")
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
        assert_eq!(std::fs::read_to_string(argc).unwrap(), "5");
        assert_eq!(
            std::fs::read_to_string(captured).unwrap(),
            "{\"hook_event_name\":\"Stop\"}"
        );
        std::fs::remove_dir_all(temp).unwrap();
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

    fn fake_zellij(temp: &Path) -> PathBuf {
        let fake = temp.join("zellij");
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

    fn events_in(path: &Path) -> Vec<String> {
        installed_events(&read_settings(path).unwrap())
    }

    #[test]
    fn a_project_install_lands_in_settings_local_json() {
        let temp = temp_dir("local");
        let files = project_files(&temp);
        install(&files, &fake_zellij(&temp), false).unwrap();

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
        install(&files, &fake_zellij(&temp), false).unwrap();

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
            .env("ZELLIJ_PANE_ID", "7")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn a_custom_codex_home_does_not_require_home() {
        assert_eq!(
            agent_root(
                HocusHookAgent::Codex,
                Some(OsString::from("/custom/codex")),
                || Err(anyhow!("HOME is unavailable"))
            )
            .unwrap(),
            PathBuf::from("/custom/codex")
        );
    }

    /// `export CODEX_HOME="$UNSET"` used to resolve to `""`, and an install into `""` writes
    /// `hooks.json` and `hocus/` into whichever folder the person was standing in - and says it
    /// worked. An empty variable is an unset one.
    #[test]
    fn an_empty_codex_home_falls_back_to_home_rather_than_to_the_current_folder() {
        assert_eq!(
            agent_root(HocusHookAgent::Codex, Some(OsString::new()), || Ok(
                PathBuf::from("/home/me")
            ))
            .unwrap(),
            PathBuf::from("/home/me/.codex")
        );
        assert!(
            agent_root(HocusHookAgent::Codex, Some(OsString::new()), || Err(
                anyhow!("HOME is unavailable")
            ))
            .is_err()
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
    fn user_commands_default_to_both_agents_but_project_commands_to_claude() {
        assert_eq!(
            selected_agents(None, false).unwrap(),
            vec![HocusHookAgent::Claude, HocusHookAgent::Codex]
        );
        assert_eq!(
            selected_agents(None, true).unwrap(),
            vec![HocusHookAgent::Claude]
        );
        assert!(selected_agents(Some(HocusHookAgent::Codex), true).is_err());
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

        install(&files, &fake_zellij(&temp), false).unwrap();

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
        let zellij = fake_zellij(&temp);
        let project = project_files(&temp);
        let user = Files {
            project: None,
            ..project_files(&temp)
        };
        install(&user, &zellij, false).unwrap();
        install(&project, &zellij, false).unwrap();

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
