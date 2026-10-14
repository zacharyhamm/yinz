//! Resolve only on the execution host, inside the managed launcher, immediately before exec.
//! No provider configuration or environment is retained in pane metadata.
mod child;
mod claude;
pub use child::{exit_status, AgentChild};
mod codex;

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::theme::*;

pub struct LaunchContext {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<OsString, OsString>,
    pub cwd: PathBuf,
    pub target: ExecutionTarget,
}

#[derive(Clone, Copy, Debug)]
pub enum ExecutionTarget {
    Local,
    Remote,
    Sandbox,
}

impl LaunchContext {
    pub fn current(executable: &Path, arguments: &[String]) -> Result<Self> {
        Ok(Self {
            executable: executable.to_owned(),
            arguments: arguments.to_vec(),
            environment: std::env::vars_os().collect(),
            cwd: std::env::current_dir().context("resolve agent working directory")?,
            target: if std::env::var_os("YINZ_REMOTE_PANE_TOKEN").is_some() {
                ExecutionTarget::Remote
            } else if std::env::var_os("YINZ_BUBBLEWRAP").is_some() {
                ExecutionTarget::Sandbox
            } else {
                ExecutionTarget::Local
            },
        })
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .env_clear()
            .envs(&self.environment)
            .current_dir(&self.cwd);
        command
    }
    fn home(&self) -> Result<PathBuf> {
        self.environment
            .get(OsStr::new("HOME"))
            .map(PathBuf::from)
            .context("agent HOME is unavailable")
    }
}

/// What a provider module verified: how its `--version` output names it, the release its
/// descriptor follows, and how it reads the current theme.
struct Provider {
    identifies: fn(&str) -> bool,
    verified: &'static str,
    resolve: fn(&LaunchContext, &mut AgentThemeSnapshot) -> Result<()>,
}

pub fn resolve(provider: &str, context: &LaunchContext) -> Option<AgentThemeSnapshot> {
    let descriptor = match provider {
        "claude" => claude::PROVIDER,
        "codex" => codex::PROVIDER,
        _ => return None,
    };
    let mut snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: provider.into(),
        provider_version: String::new(),
        theme: String::new(),
        provenance: vec![],
        availability: Availability::Available,
        definitions: vec![],
        collisions: vec![],
    };
    let mut version_note = None;
    let result = (|| {
        let mut command = context.command();
        command.arg("--version");
        let output = bounded_output(command, Duration::from_secs(3))?;
        // Wrapper notices may precede the version line, and the capture limit can split a
        // character. Only the identifying line matters, and its version is ASCII-filtered.
        let output = String::from_utf8_lossy(&output);
        // Identify the provider, not the release: later releases are assumed to keep the
        // configuration and theme formats, and resolution fails when they can't be read.
        let line = output
            .lines()
            .map(str::trim)
            .find(|line| (descriptor.identifies)(line))
            .with_context(|| format!("agent version output does not identify {provider}"))?;
        // Record a version identifier, never arbitrary output from a custom executable.
        snapshot.provider_version = line
            .split_whitespace()
            .map(|part| part.strip_prefix('v').unwrap_or(part))
            .find(|part| {
                part.bytes().next().is_some_and(|b| b.is_ascii_digit())
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
            })
            .filter(|part| part.len() <= 64)
            .unwrap_or("unknown")
            .to_owned();
        if snapshot.provider_version != descriptor.verified {
            version_note = Some(format!(
                "descriptor verified against {}, running {}",
                descriptor.verified, snapshot.provider_version
            ));
        }
        (descriptor.resolve)(context, &mut snapshot)
    })();
    if let Err(error) = result {
        snapshot.definitions.clear();
        snapshot.availability = Availability::Unavailable(format!("{error:#}"));
    }
    // An unverified release is never fully available, so the launcher always reports it.
    if let Some(note) = version_note {
        match &mut snapshot.availability {
            Availability::Available => snapshot.availability = Availability::Partial(note),
            Availability::Partial(detail) | Availability::Unavailable(detail) => {
                *detail = format!("{detail}; {note}");
            },
        }
    }
    snapshot.finish();
    Some(snapshot)
}

/// Starts only on a terminal: even probing a provider is inappropriate for piped output.
/// Drop runs on spawn/setup errors too, before the launcher reports an error to its shell.
pub struct LaunchGuard(Option<String>);

impl LaunchGuard {
    pub fn start(provider: &str, executable: &Path, arguments: &[String]) -> Self {
        if !std::io::stdout().is_terminal()
            || !["YINZ_PANE_REF", "YINZ_REMOTE_PANE_TOKEN", "YINZ_PANE_ID"]
                .iter()
                .any(|key| std::env::var_os(key).is_some())
        {
            return Self(None);
        }
        let result: Result<Option<String>> = (|| {
            let context = LaunchContext::current(executable, arguments)?;
            let Some(snapshot) = resolve(provider, &context) else {
                return Ok(None);
            };
            if !matches!(snapshot.availability, Availability::Available)
                || !snapshot.collisions.is_empty()
            {
                log::warn!(
                    "agent theme {provider}: {:?}; collisions: {:?}",
                    snapshot.availability,
                    snapshot.collisions
                );
            }
            let launch_id = uuid::Uuid::new_v4().to_string();
            emit(&AgentThemeEvent {
                version: EVENT_VERSION,
                launch_id: launch_id.clone(),
                snapshot: Some(snapshot),
            })?;
            Ok(Some(launch_id))
        })();
        match result {
            Ok(id) => Self(id),
            Err(error) => {
                log::warn!("agent theme unavailable: {error:#}");
                Self(None)
            },
        }
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if let Some(launch_id) = self.0.take() {
            emit(&AgentThemeEvent {
                version: EVENT_VERSION,
                launch_id,
                snapshot: None,
            })
            .context("end agent theme launch")
            .non_fatal();
        }
    }
}

fn emit(event: &AgentThemeEvent) -> Result<()> {
    let bytes = serde_json::to_vec(event).context("encode agent theme event")?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        bail!("agent theme event exceeds metadata limit");
    }
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut output = std::io::stdout().lock();
    write!(output, "\x1b]777;yinz-agent-theme;{encoded}\x1b\\")
        .context("attach agent theme event to pane")?;
    output.flush().context("flush agent theme event")?;
    retain(event)
        .context("retain agent theme event")
        .non_fatal();
    Ok(())
}

fn retain(event: &AgentThemeEvent) -> Result<()> {
    use crate::pty_host::protocol::{self, HostToServerMsg, ServerToHostMsg};
    let reference = std::env::var("YINZ_AGENT_THEME_REF")
        .or_else(|_| std::env::var("YINZ_PANE_REF"))
        .context("no supervisor reference")?;
    if reference.len() != 12 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid supervisor reference");
    }
    let mut stream =
        std::os::unix::net::UnixStream::connect(protocol::socket_path(&reference.into()))
            .context("connect theme supervisor")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .context("bound theme acknowledgement")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .context("bound theme report")?;
    protocol::write_msg(&mut stream, &ServerToHostMsg::AgentTheme(event.clone()))
        .context("retain source palette")?;
    if !matches!(
        protocol::read_msg::<HostToServerMsg, _>(&mut stream)?,
        Some(HostToServerMsg::Status(_))
    ) {
        bail!("supervisor did not retain source palette");
    }
    Ok(())
}

fn bounded_output(mut command: Command, timeout: Duration) -> Result<Vec<u8>> {
    // A regular temporary file avoids pipe backpressure and bounds the bytes we read into memory.
    let output = tempfile::tempfile().context("create agent version capture")?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(output.try_clone().context("clone version capture")?)
        .stderr(Stdio::null())
        .spawn()
        .context("query agent version")?;
    let deadline = Instant::now() + timeout;
    let result = loop {
        match child.try_wait().context("wait for agent version")? {
            Some(status) if status.success() => break Ok(()),
            Some(_) => break Err(anyhow!("agent version query failed")),
            None if Instant::now() >= deadline => {
                break Err(anyhow!("agent version query timed out"))
            },
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    result?;
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    output
        .seek(SeekFrom::Start(0))
        .context("rewind version capture")?;
    let mut bytes = Vec::new();
    output
        .take(64 * 1024)
        .read_to_end(&mut bytes)
        .context("read agent version")?;
    Ok(bytes)
}

fn read_json(path: &Path) -> Result<Option<serde_json::Value>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("open agent theme configuration"),
    };
    serde_json::from_reader(file.take(1024 * 1024))
        .context("parse agent theme configuration")
        .map(Some)
}

fn terminal_defaults(definitions: &mut Vec<Definition>) {
    for (channel, role) in [
        (Channel::Foreground, Role::Foreground),
        (Channel::Background, Role::Background),
    ] {
        definitions.push(Definition {
            token: "terminal.default".into(),
            channel,
            color: SourceColor::Default,
            pair: None,
            target: Some(role.into()),
        });
    }
}

fn color(value: &str) -> Option<SourceColor> {
    if let Some(hex) = value.strip_prefix('#') {
        let expanded = if hex.len() == 3 {
            hex.chars().flat_map(|c| [c, c]).collect::<String>()
        } else {
            hex.to_owned()
        };
        if expanded.len() != 6 || !expanded.is_ascii() {
            return None;
        }
        return Some(SourceColor::Rgb(
            u8::from_str_radix(&expanded[0..2], 16).ok()?,
            u8::from_str_radix(&expanded[2..4], 16).ok()?,
            u8::from_str_radix(&expanded[4..6], 16).ok()?,
        ));
    }
    if let Some(rgb) = value.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let channels = rgb
            .split(',')
            .map(|s| s.trim().parse::<u8>().ok())
            .collect::<Option<Vec<_>>>()?;
        return match channels.as_slice() {
            [r, g, b] => Some(SourceColor::Rgb(*r, *g, *b)),
            _ => None,
        };
    }
    if let Some(index) = value
        .strip_prefix("ansi256(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return index.parse().ok().map(SourceColor::Index);
    }
    let name = value.strip_prefix("ansi:")?;
    [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "blackBright",
        "redBright",
        "greenBright",
        "yellowBright",
        "blueBright",
        "magentaBright",
        "cyanBright",
        "whiteBright",
    ]
    .iter()
    .position(|c| *c == name)
    .map(|index| SourceColor::Index(index as u8))
}

#[cfg(test)]
#[path = "unit/resolver_tests.rs"]
mod tests;

/// Preserve provider stdio and arguments, with palette lifetime tied to the child.
pub fn launch(provider: &str, program: &Path, arguments: &[String]) -> Result<i32> {
    let status = {
        let _theme = LaunchGuard::start(provider, program, arguments);
        // A named child drops before the theme guard. A tail-expression temporary
        // would restore the terminal only after the guard had written its end.
        let mut child = AgentChild::spawn(Command::new(program).args(arguments))?;
        child.wait()?
    };
    Ok(exit_status(status))
}
