//! Bind a Muse session to its Yinz pane by the session id its hooks carry.
//!
//! Muse runs hook commands with the environment filtered to a fixed allowlist, so a hook
//! inherits no pane identity to report to - the same routing problem Codex has, for a
//! different reason (there it is the shared daemon's environment). The launcher
//! (`yinz agent muse`) therefore records the pane's routing environment and its own
//! process id in a registry; the first `SessionStart` binds the session id it carries to
//! the registration its own ancestry passes through, and later hooks route by that
//! binding. A session the pane did not start - a subagent's, an observer's, a manual
//! run's - matches no live binding and is dropped, so a child turn can never flip its
//! pane's record.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use yinz_runtime::agent::registry;
use yinz_runtime::agent::report::ROUTE_ENV;
use yinz_runtime::errors::prelude::*;

fn directory() -> PathBuf {
    // Fixed at /tmp like the Codex registry: hooks inherit a filtered environment,
    // so TMPDIR and XDG directories cannot be trusted to agree between the pane
    // that launched the session and the hook reporting for it.
    Path::new("/tmp")
        .join(format!("yinz-{}", nix::unistd::Uid::current()))
        .join("muse")
}

#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    session_id: String,
    env: HashMap<String, String>,
}

struct Registration {
    registry: registry::Registration,
}

impl Registration {
    fn new(root: &Path, env: HashMap<String, String>, launcher_pid: u32) -> Result<Self> {
        let registry =
            registry::Registration::new(root, "session-", env).context("register Muse session")?;
        // The reporting process binds the session, and it never sees this memory.
        registry::write_json(&registry.dir.path().join("env.json"), &registry.env)
            .context("publish Muse pane environment")?;
        // Publish before spawning so the first hook can find its launcher in its ancestry.
        std::fs::write(
            registry.dir.path().join("launcher.pid"),
            launcher_pid.to_string(),
        )
        .context("record Muse launcher process")?;
        Ok(Self { registry })
    }

    /// Announce the pane's startup before its agent can speak for itself.
    ///
    /// Muse emits no `SessionStart` until its first turn begins, so a fresh pane
    /// would otherwise stay unverified until someone types into it - the same wait
    /// Codex's bridge announces its way out of. The hook wiring on disk is the
    /// verification: every event reporting through an executable script of ours
    /// proves the pane's first turn will be heard, not that its UI is ready. The
    /// screen layer requires an empty composer before prompting. Unpinned, because the session
    /// id only exists once that turn begins; the first submit binds it.
    fn announce(&self, yinz: &Path) {
        let ready =
            match hook_files().and_then(|(script, settings)| verify_hooks(&script, &settings)) {
                Ok(()) => true,
                Err(error) => {
                    Err::<(), _>(error)
                        .context("Muse pane remains unverified")
                        .non_fatal();
                    false
                },
            };
        let mut hook = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "",
            "source": "startup",
        });
        hook["_yinz_muse_hooks_ready"] = ready.into();
        if let Ok(cwd) = std::env::current_dir() {
            hook["cwd"] = cwd.to_string_lossy().into_owned().into();
        }
        forward(yinz, &self.registry.env, &hook.to_string())
            .context("announce Muse pane startup")
            .non_fatal();
    }
}

/// The reporting script `yinz hooks install` writes, beside the settings that wire it.
pub const HOOK_SCRIPT_NAME: &str = "yinz-hook.sh";

/// Muse's config root: `$XDG_CONFIG_HOME/muse`, else `~/.config/muse`.
pub fn config_root(
    xdg_config_home: Option<std::ffi::OsString>,
    home: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    yinz_runtime::agent::paths::config_root("muse", None, None, None, xdg_config_home, home)
}

/// The hook script and the settings document that wires it, resolved from the environment.
pub fn hook_files() -> Result<(PathBuf, PathBuf)> {
    let root = config_root(std::env::var_os("XDG_CONFIG_HOME"), || {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .context("HOME is not set, so there is no Muse config to read")
    })?;
    Ok((hook_script_path(&root), settings_path(&root)))
}

/// The reporting script beside a config root.
pub fn hook_script_path(root: &Path) -> PathBuf {
    root.join("yinz").join(HOOK_SCRIPT_NAME)
}

/// The settings document wiring the reporting script beside a config root.
pub fn settings_path(root: &Path) -> PathBuf {
    root.join("settings.json")
}

/// Whether every Muse hook event reports through an executable script of ours.
///
/// The wiring read is the install's own: a handler whose command is the quoted script path
/// reports to this binary, and a group without a matcher fires on every occurrence.
/// Anything else - unwired, filtered, unparseable - leaves the pane unverified rather
/// than misread.
fn verify_hooks(script: &Path, settings: &Path) -> Result<()> {
    let metadata = std::fs::metadata(script).context("inspect Muse reporting script")?;
    if !metadata.is_file() {
        return Err(anyhow!("Muse reporting script is not a regular file"));
    }
    nix::unistd::access(script, nix::unistd::AccessFlags::X_OK)
        .context("Muse reporting script is not executable")?;
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(settings).context("read Muse hook settings")?,
    )
    .context("parse Muse hook settings")?;
    let command = format!("'{}'", script.display().to_string().replace('\'', r"'\''"));
    for event in yinz_types::agent::MUSE_HOOK_EVENTS {
        let wired = settings["hooks"][event].as_array().is_some_and(|groups| {
            groups.iter().any(|group| {
                group.get("matcher").is_none_or(serde_json::Value::is_null)
                    && group["hooks"].as_array().is_some_and(|handlers| {
                        handlers.iter().any(|handler| {
                            handler["type"] == "command"
                                && handler["command"].as_str() == Some(command.as_str())
                        })
                    })
            })
        });
        if !wired {
            return Err(anyhow!(
                "Muse {event} has no Yinz reporting handler without a matcher"
            ));
        }
    }
    Ok(())
}

/// The binding a live launcher published for `session_id`, if one did.
///
/// Registrations are created and removed concurrently with hooks; entries that fail
/// to read are skipped rather than failing the report. A dead launcher cannot own a
/// session, even after an unclean exit, so only leases still held count.
fn binding_for(root: &Path, session_id: &str) -> Result<Option<Binding>> {
    registry::latest_binding(root, |binding: &Binding| binding.session_id == session_id)
        .context("read Muse pane registry")
}

/// Bind `session_id` to the live registration `ancestors` passes through.
///
/// The first `SessionStart` wins: a subagent announcing its own session must not
/// steal the pane's binding. A later start replaces it only when it says the
/// session itself changed - a resume, a fork, or a clear - rather than began.
fn bind_session(
    root: &Path,
    ancestors: &HashSet<u32>,
    session_id: &str,
    source: Option<&str>,
) -> Result<bool> {
    if session_id.is_empty() || session_id.len() > 256 {
        return Err(anyhow!("invalid Muse session id"));
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("read Muse pane registry"),
    };
    for entry in entries {
        let entry = entry.context("read Muse session registration")?;
        let dir = entry.path();
        let matched = (|| -> Result<bool> {
            if !registry::lease_held(&dir).context("check Muse session lease")? {
                return Ok(false);
            }
            let launcher: u32 = std::fs::read_to_string(dir.join("launcher.pid"))
                .ok()
                .and_then(|pid| pid.trim().parse().ok())
                .unwrap_or(0);
            if launcher == 0 || !ancestors.contains(&launcher) {
                return Ok(false);
            }
            if let Some((binding, _)) =
                registry::read_binding::<Binding>(&dir).context("read Muse pane binding")?
            {
                if binding.session_id == session_id {
                    return Ok(true);
                }
                if !matches!(source, Some("resume" | "fork" | "clear")) {
                    return Ok(true);
                }
            }
            let env: HashMap<String, String> = serde_json::from_str(
                &std::fs::read_to_string(dir.join("env.json"))
                    .context("read Muse pane environment")?,
            )
            .context("parse Muse pane environment")?;
            let binding = Binding {
                session_id: session_id.into(),
                env,
            };
            registry::write_json(&dir.join("binding.json"), &binding)
                .context("publish Muse pane binding")?;
            Ok(true)
        })();
        if matched.unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ancestor_pids() -> HashSet<u32> {
    let mut ancestors = HashSet::new();
    let mut system = sysinfo::System::new();
    let refresh = sysinfo::ProcessRefreshKind::nothing();
    let mut pid = Some(sysinfo::Pid::from_u32(std::process::id()));
    for _ in 0..128 {
        let Some(current) = pid else { break };
        ancestors.insert(current.as_u32());
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[current]),
            true,
            refresh,
        );
        pid = system.process(current).and_then(|process| process.parent());
    }
    ancestors
}

/// Run Muse in this pane, registering it so its hooks route back here.
///
/// Outside a pane there is nothing to route to, so the program runs directly.
/// Inside one, the registration outlives the session exactly: it is removed when
/// the agent exits, and a crash leaves the lease unlocked, which reads as gone.
pub fn launch(program: &Path, args: &[String]) -> Result<i32> {
    let pane = ["YINZ_PANE_REF", "YINZ_PANE_ID", "YINZ_REMOTE_PANE_TOKEN"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if !pane {
        return Command::new(program)
            .args(args)
            .status()
            .map(exit_code)
            .context("run Muse");
    }
    let env: HashMap<String, String> = ROUTE_ENV
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).into(), value))
        })
        .collect();
    let registration = Registration::new(&directory(), env, std::process::id())?;
    let mut child = Command::new(program)
        .args(args)
        .spawn()
        .context("run Muse")?;
    if let Ok(yinz) = std::env::current_exe() {
        registration.announce(&yinz);
    }
    child.wait().map(exit_code).context("wait for Muse")
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map_or(1, |signal| 128 + signal))
}

/// Report a Muse hook to the pane its session is bound to.
///
/// `SessionStart` binds first, by the hook's own ancestry; a start from a session
/// no live registration owns - a subagent's, an observer's, a manual run's - binds
/// nothing. What stays unbound is dropped, silently and successfully: hooks have no
/// one to tell, and a hook that fails its agent's run is worse than a report that
/// never lands.
pub fn report(yinz: &Path) -> Result<()> {
    yinz_runtime::agent::report::traced_report("muse", |trace| {
        // Unbounded like the Codex router's: the broadcast cap bounds the
        // normalized event, not the provider's own payload, and a prompt `agent
        // prompt` accepted arrives here whole inside `UserPromptSubmit`.
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .context("read Muse hook")?;
        if let Ok(mut trace) = trace.lock() {
            trace.payload_bytes = raw.len();
            trace.enter("muse_route");
        }
        let event: serde_json::Value =
            serde_json::from_str(&raw).context("invalid Muse hook JSON")?;
        if let Ok(mut trace) = trace.lock() {
            trace.event = event["hook_event_name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
        }
        let session_id = event["session_id"]
            .as_str()
            .context("Muse hook has no session id")?;
        if session_id.is_empty() || session_id.len() > 256 {
            return Err(anyhow!("invalid Muse session id"));
        }
        if event["hook_event_name"].as_str() == Some("SessionStart") {
            bind_session(
                &directory(),
                &ancestor_pids(),
                session_id,
                event["source"].as_str(),
            )?;
        }
        let binding = binding_for(&directory(), session_id)?;
        let Some(binding) = binding else {
            if let Ok(trace) = trace.lock() {
                trace.record("muse_route", trace.started.elapsed(), "unbound");
            }
            return Ok(());
        };
        forward(yinz, &binding.env, &prune_for_forward(raw)?)
    })
}

/// The hook as it travels on: whole up to the broadcast cap, pruned above it.
///
/// A prompt `agent prompt` accepted (up to 1 MiB) arrives whole inside
/// `UserPromptSubmit`, and a final message is likewise unbounded; dropping
/// either here would cost the turn its acknowledgement or its completion and
/// leave the pane busy. The shared prune keeps the fingerprint the
/// acknowledgement is matched by and the turn identity the completion needs,
/// and what it forwards still fits the remote report's intake.
fn prune_for_forward(raw: String) -> Result<String> {
    yinz_runtime::agent::hooks::prune_for_forward("muse", raw).context("invalid Muse hook JSON")
}

/// Hand the hook to the session the binding names, wherever it lives.
///
/// A bound remote token sends the event over the broker's own channel, where it is
/// trusted because it came over the transport rather than out of the pane's bytes;
/// anything else is a local session's socket directory and goes to `action`.
fn forward(yinz: &Path, env: &HashMap<String, String>, raw: &str) -> Result<()> {
    let mut child = yinz_runtime::agent::report::forwarding_command(yinz, "muse", env)
        .spawn()
        .context("deliver Muse hook")?;
    child
        .stdin
        .take()
        .context("deliver Muse hook")?
        .write_all(raw.as_bytes())
        .context("deliver Muse hook")?;
    if child.wait().context("deliver Muse hook")?.success() {
        Ok(())
    } else {
        Err(anyhow!("Muse hook delivery failed"))
    }
}

#[cfg(test)]
#[path = "unit/muse_tests.rs"]
mod tests;
