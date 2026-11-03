//! Client-local update helpers and restart state. Downloads never run on the render thread.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{data::SessionConnection, update_client::*};

const MAX_HELPER_BYTES: u64 = 1024 * 1024;
const MAX_RESUME_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REQUESTS: usize = 64;

struct Job {
    request: ParticipantRequest,
    result: Option<Result<serde_json::Value, String>>,
    applied: bool,
    receiver: Receiver<Result<serde_json::Value, String>>,
}

/// Captured once before installation replaces the on-disk executable.
pub(crate) struct Updates {
    executable: Result<PathBuf, String>,
    data_dir: Option<PathBuf>,
    jobs: BTreeMap<String, Job>,
    published: Option<String>,
}

impl Updates {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        Self {
            executable: std::env::current_exe()
                .and_then(std::fs::canonicalize)
                .map_err(|error| format!("locate client executable: {error}")),
            data_dir,
            jobs: BTreeMap::new(),
            published: None,
        }
    }

    fn poll(&mut self, token: &str) -> Result<ClientReply> {
        let job = self
            .jobs
            .get_mut(token)
            .context("unknown update request token")?;
        if job.result.is_none() {
            match job.receiver.try_recv() {
                Ok(result) => job.result = Some(result),
                Err(TryRecvError::Empty) => return Ok(ClientReply::Pending),
                Err(TryRecvError::Disconnected) => {
                    job.result = Some(Err("update helper worker disconnected; inspect the operation journal before retrying".into()));
                },
            }
        }
        let result = job
            .result
            .as_ref()
            .context("update helper result unavailable")?
            .clone();
        if !job.applied && result.is_ok() {
            match &job.request {
                ParticipantRequest::Publish { operation } => {
                    self.published = Some(operation.clone())
                },
                ParticipantRequest::Rollback { operation }
                    if self.published.as_deref() == Some(operation) =>
                {
                    self.published = None
                },
                _ => {},
            }
        }
        // An observed failure is acknowledged too. It cannot publish an
        // installation, but must not permanently block a later recovered update.
        job.applied = true;
        result
            .map(ClientReply::Participant)
            .map_err(anyhow::Error::msg)
    }

    fn settle_previous(&mut self) -> Result<()> {
        let mut completed = Vec::new();
        for (token, job) in &mut self.jobs {
            if job.result.is_none() {
                match job.receiver.try_recv() {
                    Ok(result) => job.result = Some(result),
                    Err(TryRecvError::Empty) => {
                        bail!("another client update request is still running")
                    },
                    Err(TryRecvError::Disconnected) => {
                        job.result = Some(Err("update helper worker disconnected".into()));
                    },
                }
            }
            if !job.applied {
                completed.push(token.clone());
            }
        }
        // A coordinator may lose its poll or reply. Apply completed outcomes
        // before starting the next helper so a later old Publish reply cannot
        // overtake Rollback, and failed old work cannot block future restarts.
        // poll retains each result, including errors, for idempotent replies.
        for token in completed {
            let _ = self.poll(&token);
        }
        Ok(())
    }

    pub fn handle(&mut self, token: &str, request: ClientRequest) -> Result<ClientReply> {
        anyhow::ensure!(
            !token.is_empty() && token.len() <= 256,
            "invalid update request token"
        );
        match request {
            ClientRequest::Poll { token } => self.poll(&token),
            ClientRequest::Participant(request) => {
                if let Some(job) = self.jobs.get(token) {
                    anyhow::ensure!(
                        job.request == request,
                        "update token reused with a different request"
                    );
                    return self.poll(token);
                }
                anyhow::ensure!(
                    self.jobs.len() < MAX_REQUESTS,
                    "too many client update requests; reconnect before retrying"
                );
                self.settle_previous()?;
                let executable = self
                    .executable
                    .as_ref()
                    .map_err(|error| anyhow!(error.clone()))?
                    .clone();
                let invocation = ParticipantInvocation {
                    data_dir: self.data_dir.clone(),
                    executable: executable.clone(),
                    request: request.clone(),
                };
                let (sender, receiver) = mpsc::channel();
                std::thread::Builder::new()
                    .name("client-update".into())
                    .spawn(move || {
                        let result = participant(&executable, invocation)
                            .map_err(|error| format!("{error:#}"));
                        let _ = sender.send(result);
                    })
                    .context("start client update helper")?;
                self.jobs.insert(
                    token.into(),
                    Job {
                        request,
                        result: None,
                        applied: false,
                        receiver,
                    },
                );
                Ok(ClientReply::Pending)
            },
            ClientRequest::Restart { .. } | ClientRequest::Ready { .. } => {
                bail!("restart readiness must pass through the client render loop")
            },
        }
    }

    pub fn validate_restart(
        &self,
        operation: &str,
        executable: &Path,
        target: &SessionConnection,
    ) -> Result<()> {
        anyhow::ensure!(
            self.jobs
                .values()
                .all(|job| job.result.is_some() && job.applied),
            "client still has an unacknowledged update operation"
        );
        anyhow::ensure!(
            self.published.as_deref() == Some(operation),
            "client installation has not acknowledged publication for this update"
        );
        let expected = self
            .executable
            .as_ref()
            .map_err(|error| anyhow!(error.clone()))?;
        anyhow::ensure!(
            executable == expected,
            "restart executable differs from this client's prepared installation"
        );
        anyhow::ensure!(
            target
                .name
                .as_ref()
                .is_some_and(|name| !name.as_str().is_empty()),
            "restart target must name the migrated session"
        );
        anyhow::ensure!(
            target.layout.is_none() && target.cwd.is_none(),
            "an update cannot create a new session or change its layout"
        );
        let metadata =
            std::fs::metadata(executable).context("inspect replacement client executable")?;
        anyhow::ensure!(
            metadata.is_file() && metadata.mode() & 0o111 != 0,
            "replacement client is not executable"
        );
        Ok(())
    }
}

fn participant(executable: &Path, invocation: ParticipantInvocation) -> Result<serde_json::Value> {
    let mut input = tempfile::tempfile().context("create participant input")?;
    serde_json::to_writer(&mut input, &invocation).context("encode participant input")?;
    anyhow::ensure!(
        input.metadata().context("inspect participant input")?.len() <= MAX_HELPER_BYTES,
        "oversized participant input"
    );
    input
        .seek(SeekFrom::Start(0))
        .context("rewind participant input")?;
    let output = tempfile::tempfile().context("create participant output")?;
    let errors = tempfile::tempfile().context("create participant diagnostics")?;
    let inventory = matches!(invocation.request, ParticipantRequest::Inventory);
    let mut child = Command::new(executable)
        .arg(PARTICIPANT_ARG)
        .process_group(0)
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(
            output.try_clone().context("duplicate participant output")?,
        ))
        .stderr(Stdio::from(
            errors
                .try_clone()
                .context("duplicate participant diagnostics")?,
        ))
        .spawn()
        .context("launch participant helper")?;
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    let result = (|| loop {
        let too_large = [&output, &errors].into_iter().any(|file| {
            file.metadata()
                .map_or(true, |meta| meta.len() > MAX_HELPER_BYTES)
        });
        anyhow::ensure!(
            !too_large,
            "participant response exceeded its limit; inspect the operation journal"
        );
        if let Some(status) = child.try_wait().context("wait for participant helper")? {
            let stdout = read_bounded(output, MAX_HELPER_BYTES)?;
            if !status.success() {
                bail!(
                    "participant helper failed: {}",
                    String::from_utf8_lossy(&read_bounded(errors, MAX_HELPER_BYTES)?).trim()
                );
            }
            let mut result = serde_json::from_slice::<Result<serde_json::Value, String>>(&stdout)
                .context("decode participant reply")?
                .map_err(anyhow::Error::msg)?;
            if inventory {
                stamp_running_version(&mut result)?;
            }
            return Ok(result);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "participant helper timed out; inspect the operation journal before retrying"
        );
        std::thread::park_timeout(Duration::from_millis(20));
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn stamp_running_version(inventory: &mut serde_json::Value) -> Result<()> {
    let inventory = inventory
        .as_object_mut()
        .context("participant inventory is not an object")?;
    inventory.insert(
        "running_version".into(),
        serde_json::Value::String(yinz_runtime::consts::VERSION.into()),
    );
    Ok(())
}

fn read_bounded(mut file: File, maximum: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))
        .context("rewind update file")?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .context("read update file")?;
    anyhow::ensure!(
        bytes.len() as u64 <= maximum,
        "update file exceeded its limit"
    );
    Ok(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Resume {
    pub operation: String,
    pub source_client: yinz_types::data::ClientId,
    pub cli_args: yinz_cli::args::CliArgs,
    #[serde(with = "config_kdl")]
    pub config: yinz_config::config::Config,
    pub options: yinz_types::input::options::Options,
    pub remote_host: Option<String>,
    pub target: SessionConnection,
}

mod config_kdl {
    use serde::{Deserialize, Deserializer, Serializer};
    use yinz_config::config::Config;

    pub fn serialize<S: Serializer>(config: &Config, serializer: S) -> Result<S::Ok, S::Error> {
        let text = config.to_string(false).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Config, D::Error> {
        let text = String::deserialize(deserializer)?;
        Config::from_kdl(&text, None).map_err(serde::de::Error::custom)
    }
}

pub(crate) struct Restart {
    pub executable: PathBuf,
    pub resume: Resume,
}

impl Restart {
    /// Called only after all terminal modes have been restored. A failed exec leaves the old
    /// client alive, and the caller reconnects it instead of claiming replacement succeeded.
    pub fn exec(&self) -> Result<()> {
        let mut file = tempfile::Builder::new()
            .prefix(".yinz-client-resume-")
            .tempfile()
            .context("create private client resume file")?;
        serde_json::to_writer(&mut file, &self.resume).context("write client resume state")?;
        file.flush().context("flush client resume state")?;
        file.as_file()
            .sync_all()
            .context("sync client resume state")?;
        anyhow::ensure!(
            file.as_file()
                .metadata()
                .context("inspect client resume state")?
                .len()
                <= MAX_RESUME_BYTES,
            "client resume state exceeded its limit"
        );
        let error = Command::new(&self.executable)
            .arg(RESUME_ARG)
            .arg(file.path())
            .exec();
        // The temporary file remains owned here on failure and is removed by Drop.
        Err(error).context("replace attached client executable")
    }
}

/// Entry point for the hidden resume command. Only private, regular, same-user files are read.
pub fn resume_client(path: &Path) -> Result<i32> {
    use std::os::unix::fs::OpenOptionsExt;
    anyhow::ensure!(
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".yinz-client-resume-")),
        "invalid client resume file name"
    );
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .context("open client resume state")?;
    let metadata = file.metadata().context("inspect client resume state")?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == nix::unistd::geteuid().as_raw()
            && metadata.mode() & 0o077 == 0,
        "client resume state must be a private file owned by this user"
    );
    let resume: Resume = serde_json::from_slice(&read_bounded(file, MAX_RESUME_BYTES)?)
        .context("decode client resume state")?;
    std::fs::remove_file(path).context("remove consumed client resume state")?;
    run_resume(resume, true)
}

pub(crate) fn run_resume(resume: Resume, replaced: bool) -> Result<i32> {
    let os =
        crate::os_input_output::get_client_os_input().context("open resumed client terminal")?;
    run_resume_with_os(resume, replaced, Box::new(os))
}

pub(crate) fn run_resume_with_os(
    mut resume: Resume,
    replaced: bool,
    os: Box<dyn crate::os_input_output::ClientOsApi>,
) -> Result<i32> {
    resume.cli_args.update_resume = replaced.then(|| resume.operation.clone());
    // Keep the local resume marker on exec failure too, so startup uses the
    // captured configuration. Without update_resume it advertises no readiness.
    resume.cli_args.update_source_client = Some(resume.source_client.get());
    let name = resume
        .target
        .name
        .clone()
        .context("client resume requires a session name")?;
    crate::remote_sessions::validate_name(name.as_str())?;
    let (mut info, mut cancellation) = if let Some(host) = &resume.remote_host {
        let cancellation = crate::remote_sessions::StartupCancellation::new()?;
        // Updating must only reattach: an absent endpoint must never resurrect commands.
        let forward = crate::remote_sessions::connect(
            host,
            name.as_str(),
            false,
            &cancellation.cancelled,
            &resume.config.env,
        )?;
        let socket = forward.socket().to_owned();
        crate::remote_sessions::set_forward(Some(forward));
        (
            crate::ClientInfo::Remote {
                host: host.clone(),
                name,
                socket,
                cancelled: cancellation.cancelled.clone(),
            },
            Some(cancellation),
        )
    } else {
        anyhow::ensure!(
            matches!(
                yinz_session::sessions::session_status(&name)
                    .map_err(std::io::Error::from)
                    .context("probe migrated session")?,
                yinz_ipc::probe::SessionProbe::Alive
            ),
            "migrated session is not available; refusing to restart its commands"
        );
        (
            crate::ClientInfo::Attach(name, resume.options.clone()),
            None,
        )
    };
    loop {
        let outcome = crate::start_client(
            os.clone(),
            resume.cli_args.clone(),
            resume.config.clone(),
            resume.options.clone(),
            info,
            resume.target.tab_position,
            resume.target.pane_id,
            false,
            false,
            cancellation.take(),
        );
        if outcome.exit_status != 0 {
            return Ok(outcome.exit_status);
        }
        let Some(target) = outcome.reconnect_to_session else {
            return Ok(0);
        };
        // Subsequent user-requested session switches retain their ordinary create behavior.
        let next = crate::session_switch::legacy_reconnect(
            target.clone(),
            resume.remote_host.clone(),
            &resume.options,
            &*os,
            &resume.config.env,
        )
        .map_err(|outcome| anyhow!(outcome.exit_msg))?;
        (info, cancellation) = next;
        resume.target = target;
        resume.cli_args.update_resume = None;
        resume.cli_args.update_source_client = None;
    }
}

#[cfg(test)]
#[path = "unit/update.rs"]
mod tests;
