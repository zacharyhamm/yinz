//! Non-blocking, execution-host palette discovery for foreground agents.
use super::{ExecutionTarget, LaunchContext};
use crate::process_start::start_token;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    hash::{Hash, Hasher},
    path::Path,
    sync::mpsc,
};
use sysinfo::{Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::theme::{DetectedAgentTheme, DETECTED_LAUNCH_PREFIX};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Identity {
    pid: u32,
    started: u64,
    argv: Vec<OsString>,
}

#[derive(Default)]
pub struct Detector {
    identity: Option<Identity>,
    state: Option<DetectedAgentTheme>,
    retained: Option<yinz_types::agent::theme::AgentThemeLaunch>,
    // One worker per observed interval; a replaced receiver discards stale results.
    pending: Option<mpsc::Receiver<Option<yinz_types::agent::theme::AgentThemeSnapshot>>>,
}

impl Identity {
    fn launch_prefix(&self) -> String {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hash);
        format!("{DETECTED_LAUNCH_PREFIX}{:016x}:", hash.finish())
    }
}

impl Detector {
    /// Resume only when the retained palette identifies the same execution-host process.
    pub fn adopt(&mut self, launch: yinz_types::agent::theme::AgentThemeLaunch) {
        self.retained = Some(launch);
    }

    /// Probe even idle panes: exit and foreground changes need no output to be observable.
    pub fn poll(
        &mut self,
        pid: Option<u32>,
        target: ExecutionTarget,
    ) -> Option<DetectedAgentTheme> {
        let mut system = System::new();
        if let Some(pid) = pid {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
                false,
                ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
            );
        }
        let process = pid.and_then(|pid| system.process(sysinfo::Pid::from_u32(pid)));
        // sysinfo's start time moves with each System's boot time; the token is exact.
        let identity = process.and_then(|p| {
            Some(Identity {
                pid: p.pid().as_u32(),
                started: start_token(p)?,
                argv: p.cmd().to_vec(),
            })
        });
        if self.identity != identity {
            self.identity = identity;
            self.state = None;
            self.pending = None;
            if let Some(process) = process {
                let retained = self.retained.take().filter(|launch| {
                    self.identity.as_ref().is_some_and(|identity| {
                        launch.launch_id.starts_with(&identity.launch_prefix())
                    })
                });
                match retained {
                    Some(launch) if !launch.snapshot.is_detection_pending() => {
                        self.state = Some(DetectedAgentTheme {
                            launch_id: launch.launch_id,
                            provider: launch.snapshot.provider.clone(),
                            snapshot: Some(launch.snapshot),
                        });
                    },
                    retained => {
                        self.begin(process, target)
                            .context("detect agent palette")
                            .non_fatal();
                        if let (Some(state), Some(launch)) = (&mut self.state, retained) {
                            state.launch_id = launch.launch_id;
                        }
                    },
                }
            }
        }
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(snapshot) => {
                    if let Some(state) = &mut self.state {
                        state.snapshot = snapshot;
                    }
                    self.pending = None;
                },
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {},
            }
        }
        self.state.clone()
    }

    fn begin(&mut self, process: &Process, target: ExecutionTarget) -> Result<()> {
        let argv: Vec<String> = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let Some((program, args)) = argv.split_first() else {
            return Ok(());
        };
        let Some(provider) =
            yinz_types::agent::kind_from_program_and_args(Path::new(program), args)
                .filter(|kind| matches!(kind.as_str(), "claude" | "codex"))
        else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
            // Do not map redirected output or resolve a different mount namespace
            // against this host's configuration paths.
            let root = std::fs::canonicalize(format!("/proc/{}/root", process.pid()))
                .context("inspect agent filesystem root")?;
            let agent_mounts = std::fs::metadata(format!("/proc/{}/ns/mnt", process.pid()))
                .context("inspect agent mount namespace")?;
            let own_mounts = std::fs::metadata("/proc/self/ns/mnt")
                .context("inspect resolver mount namespace")?;
            if root != Path::new("/")
                || (agent_mounts.dev(), agent_mounts.ino()) != (own_mounts.dev(), own_mounts.ino())
            {
                return Ok(());
            }
            let stdout_path = format!("/proc/{}/fd/1", process.pid());
            if !std::fs::metadata(&stdout_path)
                .context("inspect agent stdout")?
                .file_type()
                .is_char_device()
            {
                return Ok(());
            }
            let stdout = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
                .open(stdout_path)
                .context("inspect agent terminal")?;
            use std::io::IsTerminal;
            if !stdout.is_terminal() {
                return Ok(());
            }
        }
        let state = DetectedAgentTheme {
            launch_id: format!(
                "{}{}",
                self.identity
                    .as_ref()
                    .context("agent identity unavailable")?
                    .launch_prefix(),
                uuid::Uuid::new_v4()
            ),
            provider: provider.clone(),
            snapshot: None,
        };
        self.state = Some(state);
        let pid = process.pid();
        let identity = self.identity.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("detected-agent-theme".into())
            .spawn(move || {
                let result = (|| {
                    let mut system = System::new();
                    system.refresh_processes_specifics(
                        ProcessesToUpdate::Some(&[pid]),
                        false,
                        ProcessRefreshKind::nothing()
                            .with_cmd(UpdateKind::Always)
                            .with_exe(UpdateKind::Always)
                            .with_cwd(UpdateKind::Always)
                            .with_environ(UpdateKind::Always),
                    );
                    let p = system.process(pid).context("agent process disappeared")?;
                    if identity.as_ref()
                        != Some(&Identity {
                            pid: pid.as_u32(),
                            started: start_token(p).context("agent process disappeared")?,
                            argv: p.cmd().to_vec(),
                        })
                    {
                        bail!("agent process changed during palette discovery");
                    }
                    let environment: BTreeMap<OsString, OsString> = p
                        .environ()
                        .iter()
                        .filter_map(|entry| {
                            use std::os::unix::ffi::{OsStrExt, OsStringExt};
                            let bytes = entry.as_os_str().as_bytes();
                            let split = bytes.iter().position(|b| *b == b'=')?;
                            Some((
                                OsString::from_vec(bytes[..split].to_vec()),
                                OsString::from_vec(bytes[split + 1..].to_vec()),
                            ))
                        })
                        .collect();
                    if !environment.contains_key(std::ffi::OsStr::new("HOME")) {
                        bail!("agent environment is unavailable");
                    }
                    // Native providers use the actual executable. Interpreter launches use the
                    // recognized script, never the interpreter's unrelated --version output.
                    let (executable, arguments) =
                        if yinz_types::agent::kind_from_program_and_args(Path::new(&argv[0]), &[])
                            == Some(provider.clone())
                        {
                            (
                                p.exe().context("agent executable unavailable")?.to_owned(),
                                argv[1..].to_vec(),
                            )
                        } else {
                            let index = argv
                                .iter()
                                .position(|arg| {
                                    yinz_types::agent::kind_from_program_and_args(
                                        Path::new(arg),
                                        &[],
                                    ) == Some(provider.clone())
                                })
                                .context("agent script unavailable")?;
                            (
                                std::path::PathBuf::from(&argv[index]),
                                argv[index + 1..].to_vec(),
                            )
                        };
                    let cwd = p
                        .cwd()
                        .context("agent working directory unavailable")?
                        .to_owned();
                    let snapshot = super::resolve(
                        &provider,
                        &LaunchContext {
                            executable,
                            arguments,
                            environment,
                            cwd,
                            target,
                        },
                    );
                    if let Some(snapshot) = &snapshot {
                        if serde_json::to_vec(snapshot)
                            .context("encode detected palette")?
                            .len()
                            > yinz_types::agent::theme::MAX_SNAPSHOT_BYTES
                        {
                            bail!("detected palette exceeds metadata limit");
                        }
                        if !matches!(
                            snapshot.availability,
                            yinz_types::agent::theme::Availability::Available
                        ) || !snapshot.collisions.is_empty()
                        {
                            log::warn!(
                                "detected agent theme {provider}: {:?}; collisions: {:?}",
                                snapshot.availability,
                                snapshot.collisions
                            );
                        }
                    }
                    Ok(snapshot)
                })();
                match result {
                    Ok(snapshot) => {
                        let _ = sender.send(snapshot);
                    },
                    Err(error) => {
                        log::warn!("detected agent theme unavailable: {error:#}");
                        let _ = sender.send(Some(yinz_types::agent::theme::AgentThemeSnapshot {
                            version: yinz_types::agent::theme::SNAPSHOT_VERSION,
                            provider,
                            provider_version: String::new(),
                            theme: String::new(),
                            provenance: vec![],
                            definitions: vec![],
                            collisions: vec![],
                            availability: yinz_types::agent::theme::Availability::Unavailable(
                                format!("{error:#}"),
                            ),
                        }));
                    },
                }
            })
            .context("start detected agent theme resolver")?;
        self.pending = Some(receiver);
        Ok(())
    }
}

#[cfg(test)]
#[path = "unit/detected_tests.rs"]
mod tests;
