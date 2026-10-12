//! Persistent project environments and internal mother profiles. Launch never reads project KDL.
use kdl::{KdlDocument, KdlNode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{OpenOptionsExt, PermissionsExt},
    process::CommandExt,
};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;

pub const RUN_FLAG: &str = yinz_types::agent::BUBBLEWRAP_RUN_FLAG;
const HOME: &str = "/home/yinz";
const SYSTEM: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64"];
const ETC: &[&str] = &[
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/nsswitch.conf",
    "/etc/ssl/certs",
    "/etc/pki/tls/certs",
    "/etc/ca-certificates",
    "/etc/ld.so.cache",
    // Debian-family systems reach `cc`, `awk` and other `/usr/bin` tools through here.
    "/etc/alternatives",
];
/// Synthesized for the sandbox's single user, so `getpwuid` agrees with the private home.
const ACCOUNTS: &[&str] = &["/etc/passwd", "/etc/group"];
#[cfg(target_os = "linux")]
#[path = "claude_view.rs"]
mod claude_view;

use yinz_types::input::command::ClaudeSettings;

const LOG_LIMIT: u64 = 1024 * 1024;

/// Runtime probes may fall back locally only for a validated internal mother profile.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchCheck {
    Ready(Option<ClaudeSettings>),
    MotherFallback(String),
}

/// The launch program does not resolve on the sandbox's own `PATH`.
#[derive(Debug)]
pub struct ProgramNotFound;

impl std::fmt::Display for ProgramNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("program not found in the Bubblewrap environment")
    }
}

impl std::error::Error for ProgramNotFound {}

/// Invalid Claude configuration must not silently launch an unsandboxed agent.
#[derive(Debug)]
pub struct MotherSetupError;

impl std::fmt::Display for MotherSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cannot prepare Claude mother configuration")
    }
}

impl std::error::Error for MotherSetupError {}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ClaudeState {
    root: PathBuf,
    global: PathBuf,
    home: PathBuf,
}

impl ClaudeState {
    fn resolve() -> Result<Self> {
        let home = host_home()?;
        let override_root = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|p| !p.is_empty());
        let root = yinz_runtime::agent::paths::config_root(
            "claude",
            override_root.clone(),
            None,
            None,
            None,
            || Ok(home.clone()),
        )?;
        let root = if root.is_relative() {
            root.canonicalize()
                .context("resolve relative Claude config directory")?
        } else {
            root
        };
        let global = if override_root.is_some() {
            root.join(".claude.json")
        } else {
            home.join(".claude.json")
        };
        Ok(Self {
            root: resolved_path(&root)?,
            // Claude locks the selected filename, even when it is a symlink.
            global,
            home: resolved_path(&home)?,
        })
    }

    fn env(&self) -> Result<BTreeMap<String, String>> {
        let mut env = BTreeMap::from([
            ("DISABLE_AUTOUPDATER".into(), "1".into()),
            ("XDG_CACHE_HOME".into(), "/run/yinz-claude/cache".into()),
            ("XDG_STATE_HOME".into(), "/run/yinz-claude/state".into()),
        ]);
        for (name, path) in [("HOME", &self.home), ("CLAUDE_CONFIG_DIR", &self.root)] {
            env.insert(
                name.into(),
                path.to_str().context("Claude path is not UTF-8")?.into(),
            );
        }
        Ok(env)
    }

    fn settings_path(&self) -> Result<PathBuf> {
        let legacy = self.root.join(".config.json");
        if legacy
            .try_exists()
            .context("inspect legacy Claude settings")?
        {
            Ok(legacy)
        } else {
            Ok(self.global.clone())
        }
    }

    fn settings(&self) -> Result<ClaudeSettings> {
        settings_snapshot(|| {
            let path = self.settings_path()?;
            fs::read(&path).with_context(|| {
                format!(
                    "read host Claude settings {}; run Claude on the host to complete setup first",
                    path.display()
                )
            })
        })
    }

    fn trust_mother(&self, work: &Path) -> Result<()> {
        let work = work
            .canonicalize()
            .context("resolve Mother trust directory")?;
        let path = self.settings_path()?;
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let _lock = ClaudeConfigLock::acquire(PathBuf::from(lock_path), Duration::from_secs(2))?;
        let settings = settings_snapshot(|| {
            fs::read(&path)
                .with_context(|| format!("read Claude trust settings {}", path.display()))
        })?;
        let mut value: serde_json::Value =
            serde_json::from_str(&settings.0).context("parse Claude trust settings")?;
        if value
            .get("projects")
            .is_some_and(|projects| !projects.is_object())
        {
            bail!("Claude projects setting must be a JSON object");
        }
        if value.get("projects").is_none() {
            value["projects"] = serde_json::json!({});
        }
        let key = work.to_str().context("Mother path is not UTF-8")?;
        let projects = value["projects"]
            .as_object_mut()
            .context("Claude projects setting")?;
        let project = projects.entry(key).or_insert_with(|| serde_json::json!({}));
        let project = project
            .as_object_mut()
            .context("Claude project setting must be a JSON object")?;
        if project.get("hasTrustDialogAccepted") == Some(&serde_json::Value::Bool(true)) {
            return Ok(());
        }
        project.insert("hasTrustDialogAccepted".into(), true.into());
        // Preserve symlinked global settings while replacing their contents atomically.
        let destination = path
            .canonicalize()
            .context("resolve Claude trust settings")?;
        let parent = destination
            .parent()
            .context("Claude settings have no parent")?;
        let mut file =
            tempfile::NamedTempFile::new_in(parent).context("create Claude trust settings")?;
        serde_json::to_writer_pretty(&mut file, &value).context("write Claude trust settings")?;
        file.as_file()
            .sync_all()
            .context("sync Claude trust settings")?;
        file.persist(&destination)
            .context("replace Claude trust settings")?;
        Ok(())
    }
}

struct ClaudeConfigLock {
    path: PathBuf,
    stop: std::sync::mpsc::Sender<()>,
    heartbeat: Option<std::thread::JoinHandle<()>>,
}

impl ClaudeConfigLock {
    fn acquire(path: PathBuf, timeout: Duration) -> Result<Self> {
        let start = Instant::now();
        loop {
            let acquired = fs::create_dir(&path).or_else(|error| {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
                let metadata = match fs::metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return fs::create_dir(&path);
                    },
                    Err(error) => return Err(error),
                };
                // Match proper-lockfile's ten-second mtime expiry, including future timestamps.
                let expired = std::time::SystemTime::now()
                    .duration_since(metadata.modified()?)
                    .is_ok_and(|age| age > Duration::from_secs(10));
                if !expired {
                    return Err(error);
                }
                match fs::remove_dir(&path) {
                    Ok(()) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error),
                }
                fs::create_dir(&path)
            });
            match acquired {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        && start.elapsed() < timeout =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                },
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("acquire Claude config lock {}", path.display()))
                },
            }
        }
        let (stop, receiver) = std::sync::mpsc::channel();
        let heartbeat_path = path.clone();
        // Claude's proper-lockfile considers a lock stale after ten seconds.
        let heartbeat = std::thread::spawn(move || {
            while receiver.recv_timeout(Duration::from_secs(1))
                == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            {
                File::open(&heartbeat_path)
                    .and_then(|file| file.set_modified(std::time::SystemTime::now()))
                    .context("refresh Claude config lock")
                    .non_fatal();
            }
        });
        Ok(Self {
            path,
            stop,
            heartbeat: Some(heartbeat),
        })
    }
}

impl Drop for ClaudeConfigLock {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        fs::remove_dir(&self.path)
            .context("release Claude config lock")
            .non_fatal();
    }
}

fn settings_snapshot(mut read: impl FnMut() -> Result<Vec<u8>>) -> Result<ClaudeSettings> {
    // Claude sometimes truncates and rewrites settings instead of renaming them.
    for attempt in 0..5 {
        let result = (|| {
            let contents = read()?;
            let value: serde_json::Value =
                serde_json::from_slice(&contents).context("parse host Claude global settings")?;
            if !value.is_object() {
                bail!("host Claude global settings must be a JSON object");
            }
            Ok(ClaudeSettings(
                String::from_utf8(contents).context("decode Claude settings")?,
            ))
        })();
        match result {
            Ok(settings) => return Ok(settings),
            Err(error) if attempt == 4 => return Err(error).context(MotherSetupError),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    unreachable!("bounded settings retry returns on the last attempt")
}

fn host_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .context("HOME is not set")
}

/// Resolve existing ancestors without creating host state directories.
fn resolved_path(path: &Path) -> Result<PathBuf> {
    absolute(path.to_str().context("agent state path is not UTF-8")?)?;
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => Ok(
            resolved_path(path.parent().context("state path has no parent")?)?
                .join(path.file_name().context("state path has no filename")?),
        ),
        Err(error) => Err(error).context("resolve agent state path"),
    }
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Declaration {
    mounts: Vec<(PathBuf, PathBuf)>,
    paths: Vec<String>,
    env: BTreeMap<String, String>,
    inherit: Vec<String>,
    seeds: Vec<(PathBuf, PathBuf)>,
    setup: Vec<Vec<String>>,
    checks: Vec<Vec<String>>,
    #[serde(default)]
    mother: Option<Vec<PathBuf>>,
    #[serde(default)]
    mother_work: Option<PathBuf>,
    #[serde(default)]
    claude: Option<ClaudeState>,
}

#[derive(Serialize, Deserialize)]
struct Accepted {
    root: PathBuf,
    declaration: Declaration,
    ready: bool,
}

#[derive(Clone)]
pub struct Environment {
    root: PathBuf,
    metadata: PathBuf,
    home: PathBuf,
    settings: Option<ClaudeSettings>,
}

struct SandboxCommand {
    command: Command,
    #[cfg(target_os = "linux")]
    view: Option<claude_view::View>,
}

impl std::fmt::Debug for SandboxCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.command.fmt(f)
    }
}

impl std::ops::Deref for SandboxCommand {
    type Target = Command;
    fn deref(&self) -> &Command {
        &self.command
    }
}

impl std::ops::DerefMut for SandboxCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

/// Resolved once: after the running binary is replaced on disk, `current_exe()` names a
/// `(deleted)` path that can be neither bound nor executed.
fn yinz_binary() -> Result<&'static PathBuf> {
    yinz_runtime::own_binary::server_binary().context("locate Yinz executable")
}

/// Expose the Yinz executable at its host path and on the sandbox's own `PATH`.
fn bind_binary(command: &mut Command, root: &Path, binary: &Path) {
    // The checkout's own build is already visible through the writable project bind.
    // A file mount there would make replacing the binary fail with EBUSY.
    if !binary.starts_with(root) {
        command.arg("--ro-bind").arg(binary).arg(binary);
    }
    command
        .arg("--ro-bind")
        .arg(binary)
        .arg("/run/yinz-bin/yinz");
}

fn bind_mother_resolver(command: &mut Command, resolver: &Path) -> Result<()> {
    let mut path = resolver.to_path_buf();
    let mut mounted = Vec::new();
    // Bound traversal even if a host service changes a link while we read it.
    for _ in 0..40 {
        let parent = path.parent().context("DNS configuration has no parent")?;
        let directory = parent
            .canonicalize()
            .context("resolve mother DNS directory")?;
        if ["/run", "/tmp"]
            .iter()
            .any(|root| directory.starts_with(root))
        {
            if directory == Path::new("/run") || directory == Path::new("/tmp") {
                bail!(
                    "mother DNS configuration requires a dedicated directory: {}",
                    path.display()
                );
            }
            if !mounted.contains(&directory) {
                // Directory binds see atomic file replacements, unlike a resolver file bind.
                command.arg("--ro-bind").arg(&directory).arg(&directory);
                mounted.push(directory.clone());
            }
        }
        let alias = yinz_runtime::home::absolute_lexical(parent);
        if alias != directory && ["/run", "/tmp"].iter().any(|root| alias.starts_with(root)) {
            command.arg("--symlink").arg(&directory).arg(alias);
        }
        path = directory.join(
            path.file_name()
                .context("DNS configuration has no filename")?,
        );
        if !fs::symlink_metadata(&path)
            .context("inspect mother DNS configuration")?
            .is_symlink()
        {
            return Ok(());
        }
        path = directory.join(fs::read_link(&path).context("read mother DNS symlink")?);
    }
    bail!("too many symlinks in mother DNS configuration")
}

#[cfg(target_os = "linux")]
fn open_mount_source(src: &Path) -> Result<File> {
    // O_PATH pins sockets and execute-only files without reading or blocking on FIFOs.
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(src)
        .context("open accepted Bubblewrap source")
}

#[cfg(not(target_os = "linux"))]
fn open_mount_source(_src: &Path) -> Result<File> {
    bail!("Bubblewrap agent environments require Linux")
}

fn platform() -> Result<()> {
    // Only success is remembered. A later downgrade still fails closed: an older bwrap
    // rejects the options every launch passes.
    static SUPPORTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if SUPPORTED.get().is_some() {
        return Ok(());
    }
    if !cfg!(target_os = "linux") {
        bail!("Bubblewrap agent environments require Linux");
    }
    let output = Command::new("bwrap")
        .arg("--version")
        .output()
        .context("Bubblewrap 0.10.0 or newer is required; install or upgrade bwrap")?;
    validate_version(&output)?;
    let _ = SUPPORTED.set(());
    Ok(())
}

/// An append-only log, emptied once it outgrows `LOG_LIMIT` so repeated launches cannot
/// grow it without bound.
fn open_log(path: &Path) -> Result<File> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    log.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("protect {}", path.display()))?;
    if log.metadata().context("inspect Bubblewrap log")?.len() > LOG_LIMIT {
        log.set_len(0).context("truncate Bubblewrap log")?;
    }
    Ok(log)
}

/// `/etc/passwd` and `/etc/group` for the sandbox: the invoking user, at home in `HOME`.
fn accounts() -> [String; 2] {
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let name = std::env::var("USER")
        .ok()
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
        .unwrap_or_else(|| "yinz".to_owned());
    [
        format!(
            "{name}:x:{uid}:{gid}::{HOME}:/bin/sh\nnobody:x:65534:65534::/:/usr/sbin/nologin\n"
        ),
        format!("{name}:x:{gid}:\nnogroup:x:65534:\n"),
    ]
}

fn validate_version(output: &std::process::Output) -> Result<()> {
    if !output.status.success() {
        bail!("bwrap --version failed; install Bubblewrap 0.10.0 or newer");
    }
    let text = std::str::from_utf8(&output.stdout).context("malformed Bubblewrap version")?;
    let version = text
        .trim()
        .strip_prefix("bubblewrap ")
        .context("malformed Bubblewrap version; expected bubblewrap 0.10.0 or newer")?;
    let numeric = version
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .next()
        .unwrap_or("");
    let parts = numeric
        .split('.')
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("malformed Bubblewrap version")?;
    let [major, minor, patch] = parts.as_slice() else {
        bail!("malformed Bubblewrap version; expected three numeric components");
    };
    if (*major, *minor, *patch) < (0, 10, 0) {
        bail!("Bubblewrap 0.10.0 or newer is required; upgrade bwrap (found {numeric})");
    }
    Ok(())
}

fn absolute(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || value.contains('\0')
    {
        bail!("expected an absolute path without . or ..: {value:?}");
    }
    Ok(path)
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn arguments(node: &KdlNode) -> Result<Vec<String>> {
    if node.ty().is_some() {
        bail!("typed Bubblewrap declarations are not supported");
    }
    node.entries()
        .iter()
        .map(|entry| {
            if entry.name().is_some() || entry.ty().is_some() {
                bail!("expected positional strings");
            }
            let value = entry
                .value()
                .as_string()
                .context("expected string argument")?;
            if value.contains('\0') {
                bail!("NUL in declaration");
            }
            Ok(value.to_owned())
        })
        .collect()
}

fn reserved_env(name: &str) -> bool {
    name.starts_with("YINZ_")
        || matches!(
            name,
            "HOME"
                | "PATH"
                | "TMPDIR"
                | "XDG_RUNTIME_DIR"
                | "XDG_CONFIG_HOME"
                | "XDG_CACHE_HOME"
                | "XDG_DATA_HOME"
                | "CARGO_TARGET_DIR"
        )
}

fn env_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with(|c: char| c.is_ascii_digit())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || reserved_env(name)
    {
        bail!("invalid or reserved environment variable: {name}");
    }
    Ok(())
}

fn parse(doc: &KdlDocument, root: &Path, host_home: &Path) -> Result<Declaration> {
    let mut declaration = Declaration::default();
    for node in doc.nodes() {
        if node.children().is_some() {
            bail!("{} does not take a block", node.name());
        }
        let args = arguments(node)?
            .into_iter()
            .map(|value| {
                value
                    .replace("${HOST_HOME}", &host_home.to_string_lossy())
                    .replace("${REPOSITORY}", &root.to_string_lossy())
                    .replace("${SANDBOX_HOME}", HOME)
            })
            .collect::<Vec<_>>();
        let source = |value: &str| -> Result<PathBuf> {
            root.join(value)
                .canonicalize()
                .with_context(|| format!("missing Bubblewrap source {value:?}"))
        };
        match (node.name().value(), args.as_slice()) {
            ("ro-bind", [src, dst]) => declaration.mounts.push((source(src)?, absolute(dst)?)),
            ("seed", [src, dst]) => {
                let dst = absolute(dst)?;
                if dst == Path::new(HOME) || !dst.starts_with(HOME) {
                    bail!("seed destination must be inside {HOME}");
                }
                let src = source(src)?;
                if !src.is_file() {
                    bail!("seed source must be a file");
                }
                if declaration
                    .seeds
                    .iter()
                    .any(|(_, previous)| overlaps(previous, &dst))
                {
                    bail!("conflicting seed destination");
                }
                declaration.seeds.push((src, dst));
            },
            ("path", [path]) => {
                absolute(path)?;
                if path.contains(':') {
                    bail!("path must be one directory");
                }
                declaration.paths.push(path.clone());
            },
            ("env", [name, value]) => {
                env_name(name)?;
                if declaration
                    .env
                    .insert(name.clone(), value.clone())
                    .is_some()
                    || declaration.inherit.contains(name)
                {
                    bail!("duplicate env {name}");
                }
            },
            ("inherit-env", [name]) => {
                env_name(name)?;
                if declaration.env.contains_key(name) || declaration.inherit.contains(name) {
                    bail!("duplicate env {name}");
                }
                declaration.inherit.push(name.clone());
            },
            ("setup", [program, ..]) if !program.is_empty() => declaration.setup.push(args),
            ("check", [program, ..]) if !program.is_empty() => declaration.checks.push(args),
            _ => bail!(
                "unknown or malformed Bubblewrap declaration: {}",
                node.name()
            ),
        }
    }
    declaration.validate(root)?;
    Ok(declaration)
}

impl Declaration {
    fn validate_spawn_env(&self, env: &BTreeMap<String, String>) -> Result<()> {
        for name in env.keys() {
            if (reserved_env(name)
                || (self.claude.is_some()
                    && matches!(
                        name.as_str(),
                        "XDG_STATE_HOME" | "CLAUDE_CONFIG_DIR" | "DISABLE_AUTOUPDATER"
                    )))
                && !matches!(name.as_str(), "YINZ_AGENT_PANE" | "YINZ_SWAMP_OWNER")
            {
                bail!("Bubblewrap spawn cannot override {name}");
            }
        }
        Ok(())
    }

    fn validate(&self, root: &Path) -> Result<()> {
        let mut destinations = vec![root.to_path_buf()];
        destinations.extend(SYSTEM.iter().chain(ETC).chain(ACCOUNTS).map(PathBuf::from));
        destinations.extend([HOME, "/run", "/tmp", "/dev", "/proc"].map(PathBuf::from));
        for (src, dst) in &self.mounts {
            if src
                .canonicalize()
                .with_context(|| format!("resolve Bubblewrap source {}", src.display()))?
                != *src
            {
                bail!(
                    "Bubblewrap source changed through a symlink; prepare again: {}",
                    src.display()
                );
            }
            if destinations.iter().any(|previous| overlaps(previous, dst)) {
                bail!(
                    "conflicting or reserved mount destination {}",
                    dst.display()
                );
            }
            destinations.push(dst.clone());
        }
        Ok(())
    }
}

fn selected(root: &Path, config_dir: Option<&Path>, host_home: &Path) -> Result<Declaration> {
    let mut selected = None;
    if let Some(dir) = config_dir {
        let path = dir.join("bubblewrap.kdl");
        match fs::read_to_string(&path) {
            Ok(source) => {
                let doc: KdlDocument = source.parse().context("parse global bubblewrap.kdl")?;
                let mut roots = std::collections::HashSet::new();
                for node in doc.nodes() {
                    let args = arguments(node)?;
                    let [path] = args.as_slice() else {
                        bail!("global project takes one absolute repository path");
                    };
                    if node.name().value() != "project" {
                        bail!("global bubblewrap.kdl only accepts project blocks");
                    }
                    let body = node.children().context("project requires a block")?;
                    let path = absolute(path)?;
                    let project = match path.canonicalize() {
                        Ok(project) => project,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            eprintln!("Skipping missing Bubblewrap project {}", path.display());
                            continue;
                        },
                        Err(error) => {
                            return Err(error).context("resolve global Bubblewrap project")
                        },
                    };
                    if !roots.insert(project.clone()) {
                        bail!("duplicate global Bubblewrap project {}", project.display());
                    }
                    if project == root {
                        selected = Some(parse(body, root, host_home)?);
                    }
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error).context("read global bubblewrap.kdl"),
        }
    }
    if let Some(declaration) = selected {
        return Ok(declaration);
    }
    let source = fs::read_to_string(root.join(".yinz/bubblewrap.kdl"))
        .context("read project .yinz/bubblewrap.kdl; configure it before preparation")?;
    parse(
        &source.parse().context("parse project bubblewrap.kdl")?,
        root,
        host_home,
    )
}

/// A session name is hashed, never interpreted as a path. Contents survive agent exits.
pub fn mother_directory(cache: &Path, session: &str) -> Result<PathBuf> {
    if session.is_empty() {
        bail!("Mother requires a session name");
    }
    let parent = cache
        .join("mother")
        .join(crate::root_key(Path::new(session)));
    for path in [cache.join("mother"), parent.clone(), parent.join("work")] {
        if path.is_symlink() {
            bail!("Mother directory must not be a symlink: {}", path.display());
        }
        yinz_runtime::shared::ensure_secure_dir(&path)
            .context("create private mother directory")?;
    }
    parent
        .join("work")
        .canonicalize()
        .context("resolve mother directory")
}

fn private_mounts(command: &mut Command) {
    command.args([
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/run",
        "--dir",
        "/run/user",
        "--dir",
        "/run/yinz-bin",
    ]);
}

fn declared_environment(
    declaration: &Declaration,
    setenv: &mut impl FnMut(&str, &str) -> Result<()>,
) -> Result<()> {
    for (name, value) in &declaration.env {
        setenv(name, value)?;
    }
    for name in &declaration.inherit {
        let value =
            std::env::var(name).with_context(|| format!("missing inherited variable {name}"))?;
        setenv(name, &value)?;
    }
    Ok(())
}

/// The agent's writable directories, its configuration root first.
fn mother_state(kind: &str) -> Result<Vec<PathBuf>> {
    // shortcut: only mapped state layouts are sandboxed, extend this when another provider's paths are known.
    if !matches!(kind, "codex" | "pi" | "opencode") {
        bail!("no supported writable state layout for agent {kind}");
    }
    let home = host_home()?;
    let root = yinz_runtime::agent::paths::config_root(
        kind,
        None,
        std::env::var_os("CODEX_HOME"),
        std::env::var_os("PI_CODING_AGENT_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
        || Ok(home.clone()),
    )?;
    let mut paths = vec![root];
    if kind == "opencode" {
        for (variable, default) in [
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_STATE_HOME", ".local/state"),
            ("XDG_CACHE_HOME", ".cache"),
        ] {
            paths.push(
                std::env::var_os(variable)
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(default))
                    .join("opencode"),
            );
        }
    }
    Ok(paths)
}

impl Environment {
    /// Prepare an internal profile without reading project configuration or requiring Git.
    /// Runtime probes belong to the PTY spawn, where mother failures can fall back locally.
    pub fn prepare_mother(&self, kind: &str) -> Result<Self> {
        if kind == "claude" {
            let profile = self.accept_mother(kind).context(MotherSetupError)?;
            ClaudeState::resolve()
                .and_then(|state| state.trust_mother(&self.root))
                .context(MotherSetupError)?;
            if !cfg!(target_os = "linux") {
                platform()?;
            }
            return Ok(profile);
        }
        platform()?;
        self.accept_mother(kind)
    }

    fn accept_mother(&self, kind: &str) -> Result<Self> {
        // History records this target; switching providers must leave earlier profiles intact.
        let root = self.root.with_file_name(format!("profile-{kind}"));
        if root.is_symlink() {
            bail!(
                "Mother profile directory must not be a symlink: {}",
                root.display()
            );
        }
        yinz_runtime::shared::ensure_secure_dir(&root)
            .context("create private mother profile directory")?;
        let profile = Self::at_root(&root);
        if kind == "claude" {
            return self.accept_claude(profile);
        }
        let mut state = Vec::new();
        for path in mother_state(kind)? {
            absolute(path.to_str().context("agent state path is not UTF-8")?)?;
            self.validate_mother_state(&path)?;
            profile.validate_mother_state(&path)?;
            if !path.exists() {
                yinz_runtime::shared::ensure_secure_dir(&path)
                    .context("create agent state directory")?;
            }
            let path = path
                .canonicalize()
                .context("resolve agent state directory")?;
            self.validate_mother_state(&path)?;
            profile.validate_mother_state(&path)?;
            if !path.is_dir() {
                bail!("agent state must be a directory: {}", path.display());
            }
            state.push(path);
        }
        let accepted = Accepted {
            root: profile.root.clone(),
            declaration: Declaration {
                mother: Some(state),
                mother_work: Some(self.root.clone()),
                ..Default::default()
            },
            ready: true,
        };
        for exclusive in [false, true] {
            let _lock = profile.lock(exclusive)?;
            profile.validate_storage(&accepted.declaration)?;
            if profile
                .accepted()
                .is_ok_and(|previous| previous.declaration == accepted.declaration)
            {
                return Ok(profile);
            }
            if exclusive {
                profile.save(&accepted)?;
            }
        }
        Ok(profile)
    }

    fn accept_claude(&self, profile: Self) -> Result<Self> {
        let claude = ClaudeState::resolve()?;
        let accepted = Accepted {
            root: profile.root.clone(),
            declaration: Declaration {
                env: claude.env()?,
                mother: Some(vec![profile.home.join("projects")]),
                mother_work: Some(self.root.clone()),
                claude: Some(claude),
                ..Default::default()
            },
            ready: true,
        };
        for exclusive in [false, true] {
            let _lock = profile.lock(exclusive)?;
            profile.validate_storage(&accepted.declaration)?;
            profile.validate_claude(&accepted.declaration)?;
            if profile
                .accepted()
                .is_ok_and(|previous| previous.declaration == accepted.declaration)
            {
                return Ok(profile);
            }
            if exclusive {
                for path in [&profile.home, &profile.home.join("projects")] {
                    if path.is_symlink() {
                        bail!("private Claude state must not be a symlink");
                    }
                    yinz_runtime::shared::ensure_secure_dir(path)
                        .context("create private Claude transcripts")?;
                }
                profile.save(&accepted)?;
            }
        }
        Ok(profile)
    }

    fn validate_claude(&self, declaration: &Declaration) -> Result<()> {
        let Some(claude) = &declaration.claude else {
            return Ok(());
        };
        if declaration.mother.as_deref() != Some(&[self.home.join("projects")])
            || declaration.env != claude.env()?
        {
            bail!("invalid Claude isolation profile; prepare mother again");
        }
        let work = self.writable_root(declaration);
        self.validate_mother_state(&self.home)?;
        if overlaps(&self.home, work) {
            bail!("private Claude storage overlaps the working directory");
        }
        let metadata = self
            .metadata
            .canonicalize()
            .context("resolve Claude profile metadata")?;
        self.validate_mother_state(&claude.root)?;
        if resolved_path(&claude.root)? != claude.root
            || overlaps(&claude.root, work)
            || overlaps(&claude.root, &self.home)
            || overlaps(&claude.root, &metadata)
        {
            bail!(
                "unsafe or changed Claude state path: {}",
                claude.root.display()
            );
        }
        if !claude.root.is_dir() {
            bail!("Claude config directory is missing; run Claude on the host to complete setup first");
        }
        for path in [
            &claude.global,
            &claude.root.join(".config.json"),
            &claude.root.join(".claude.json"),
            &claude.root.join("projects"),
        ] {
            let resolved = resolved_path(path)?;
            if overlaps(&resolved, work)
                || overlaps(&resolved, &self.home)
                || overlaps(&resolved, &metadata)
            {
                bail!("unsafe or changed Claude settings path: {}", path.display());
            }
        }
        if resolved_path(&self.home)? != self.home
            || resolved_path(&self.home.join("projects"))? != self.home.join("projects")
        {
            bail!("private Claude transcripts changed through a symlink");
        }
        Ok(())
    }

    /// Preserve project targets; a mother fork needs the destination provider's state mounts.
    pub fn for_agent(&self, kind: &str, cwd: Option<&Path>) -> Result<Self> {
        let _lock = self.lock(false)?;
        let accepted = self.accepted()?;
        let Some(state) = &accepted.declaration.mother else {
            return Ok(Self::at_root(&self.root));
        };
        let cwd = cwd.context("Mother agent requires a working directory")?;
        self.validate_launch_cwd(cwd, &accepted.declaration)?;
        if kind == "claude" {
            let desired = ClaudeState::resolve()?;
            if accepted.declaration.claude.as_ref() == Some(&desired) {
                return Ok(Self::at_root(&self.root));
            }
            drop(_lock);
            return Self::at_root(self.writable_root(&accepted.declaration)).prepare_mother(kind);
        }
        let desired = mother_state(kind)?
            .into_iter()
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect::<Vec<_>>();
        // Legacy profiles have no mother_work; retain them when their mounts already match.
        if *state == desired && accepted.declaration.claude.is_none() {
            return Ok(Self::at_root(&self.root));
        }
        drop(_lock);
        Self::at_root(self.writable_root(&accepted.declaration)).prepare_mother(kind)
    }

    fn writable_root<'a>(&'a self, declaration: &'a Declaration) -> &'a Path {
        declaration.mother_work.as_deref().unwrap_or(&self.root)
    }

    fn validate_mother_state(&self, path: &Path) -> Result<()> {
        let home = host_home()?;
        let home = home.canonicalize().context("resolve host home")?;
        if !path.is_absolute()
            || path == Path::new("/tmp")
            || home.starts_with(path)
            || overlaps(path, &self.root)
            || overlaps(path, &self.metadata)
            || [
                "/run", "/proc", "/dev", "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64",
            ]
            .iter()
            .any(|reserved| overlaps(path, Path::new(reserved)))
        {
            bail!("unsafe mother agent state directory: {}", path.display());
        }
        Ok(())
    }

    fn validate_launch_cwd(&self, cwd: &Path, declaration: &Declaration) -> Result<()> {
        if let Some(paths) = &declaration.mother {
            let work = self.writable_root(declaration);
            if cwd.canonicalize().context("resolve mother cwd")? != work {
                bail!("mother must launch in its dedicated directory");
            }
            for path in paths {
                self.validate_mother_state(path)?;
                if overlaps(path, work) {
                    bail!("mother agent state must not overlap its working directory");
                }
                if path.canonicalize().context("resolve agent state")? != *path {
                    bail!("agent state path changed; prepare mother again");
                }
            }
            Ok(())
        } else {
            self.validate_cwd(cwd)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn at(root: PathBuf, data: &Path) -> Self {
        let key = crate::root_key(&root);
        let data = resolved_path(data).unwrap_or_else(|_| data.to_path_buf());
        Self {
            root,
            metadata: data.join("environments").join(&key),
            home: data.join("homes").join(key),
            settings: None,
        }
    }

    pub fn for_project(project: &Path) -> Result<Self> {
        let root = crate::root(project).map_err(|error| anyhow!(error))?;
        let env = Self::at_root(&root);
        env.validate_cwd(project)?;
        Ok(env)
    }

    /// The environment of a primary checkout or a recorded mother profile target.
    /// Nothing is trusted from `root` itself: a launch still requires its accepted record.
    pub fn at_root(root: &Path) -> Self {
        // Environments are keyed by canonical path. One that does not resolve keeps its
        // spelling and finds no accepted record.
        Self::at(
            root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            &yinz_runtime::home::xdg_data_dir().join("bubblewrap"),
        )
    }

    /// Reject a launch directory outside the checkout, or one whose Git metadata is.
    pub fn validate_cwd(&self, cwd: &Path) -> Result<()> {
        let cwd = cwd
            .canonicalize()
            .context("resolve Bubblewrap launch directory")?;
        if !cwd.starts_with(&self.root) {
            bail!("Bubblewrap checkout must be inside {}", self.root.display());
        }
        if overlaps(&self.root, Path::new(HOME))
            || self.root == Path::new("/")
            || self.root.starts_with("/run")
            || self.root.starts_with("/proc")
            || self.root.starts_with("/dev")
            || SYSTEM.iter().any(|p| overlaps(&self.root, Path::new(p)))
        {
            bail!("repository conflicts with a mandatory sandbox mount");
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .args([
                "rev-parse",
                "--path-format=absolute",
                "--absolute-git-dir",
                "--git-common-dir",
            ])
            .output()
            .context("locate Git metadata")?;
        if !output.status.success() {
            bail!("cannot resolve required Git metadata");
        }
        let output = String::from_utf8(output.stdout).context("decode Git paths")?;
        let paths: Vec<_> = output.lines().collect();
        if paths.len() != 2 || paths.iter().any(|path| !Path::new(path).is_absolute()) {
            bail!("Git returned invalid metadata paths");
        }
        for path in paths {
            let path = Path::new(path);
            if !path
                .canonicalize()
                .context("resolve Git metadata")?
                .starts_with(&self.root)
            {
                bail!("required Git metadata is outside the primary checkout");
            }
        }
        Ok(())
    }

    fn lock(&self, exclusive: bool) -> Result<File> {
        yinz_runtime::shared::ensure_secure_dir(&self.metadata)
            .context("create Bubblewrap metadata directory")?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.metadata.join("lock"))
            .context("open Bubblewrap environment lock")?;
        if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        }
        .context("Bubblewrap environment is busy; preparation excludes running agents")?;
        Ok(file)
    }

    fn validate_storage(&self, declaration: &Declaration) -> Result<()> {
        let metadata = self
            .metadata
            .canonicalize()
            .context("resolve environment metadata")?;
        for source in [&self.root, &self.home]
            .into_iter()
            .chain(declaration.mounts.iter().map(|(source, _)| source))
            .chain(declaration.mother.iter().flatten())
            .chain(declaration.mother_work.iter())
        {
            if overlaps(&resolved_path(source)?, &metadata) {
                bail!("Bubblewrap mounts must not expose environment management files");
            }
        }
        Ok(())
    }

    fn save(&self, accepted: &Accepted) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.metadata)
            .context("stage accepted declaration")?;
        serde_json::to_writer(file.as_file_mut(), accepted)
            .context("encode accepted declaration")?;
        file.as_file_mut()
            .sync_all()
            .context("sync accepted declaration")?;
        file.persist(self.metadata.join("accepted.json"))
            .context("save accepted declaration")?;
        Ok(())
    }

    fn accepted(&self) -> Result<Accepted> {
        let source = fs::read(self.metadata.join("accepted.json"))
            .context("environment is not prepared; run yinz bubblewrap prepare")?;
        let accepted: Accepted =
            serde_json::from_slice(&source).context("read accepted declaration")?;
        if accepted.root != self.root || !accepted.ready {
            bail!("environment is not ready; run yinz bubblewrap prepare");
        }
        accepted.declaration.validate(&self.root)?;
        self.validate_storage(&accepted.declaration)?;
        self.validate_claude(&accepted.declaration)?;
        Ok(accepted)
    }

    pub fn preflight(&self) -> Result<()> {
        platform()?;
        let _lock = self.lock(false)?;
        self.accepted()?;
        Ok(())
    }

    /// Protect the accepted profile's boundary against spawn's env wrapper.
    pub fn validate_spawn_env(&self, env: &BTreeMap<String, String>) -> Result<()> {
        let _lock = self.lock(false)?;
        self.accepted()?.declaration.validate_spawn_env(env)
    }

    fn project_command(
        &self,
        command: &mut Command,
        declaration: &Declaration,
        setenv: &mut impl FnMut(&str, &str) -> Result<()>,
    ) -> Result<Vec<File>> {
        let mut data_files = Vec::new();
        for (path, contents) in ACCOUNTS.iter().zip(accounts()) {
            let mut file = tempfile::tempfile().context("create sandbox account file")?;
            file.write_all(contents.as_bytes())
                .context("write sandbox account file")?;
            command
                .arg("--ro-bind-data")
                .arg(file.as_raw_fd().to_string())
                .arg(path);
            data_files.push(file);
        }
        for path in SYSTEM.iter().chain(ETC) {
            if Path::new(path).exists() {
                command.args(["--ro-bind", path, path]);
            }
        }
        private_mounts(command);
        command.arg("--bind").arg(&self.root).arg(&self.root);
        command.arg("--bind").arg(&self.home).arg(HOME);
        declared_environment(declaration, setenv)?;
        let mut paths = vec![
            "/run/yinz-bin".to_owned(),
            "/usr/local/bin".into(),
            "/usr/bin".into(),
            "/bin".into(),
        ];
        paths.extend(declaration.paths.iter().cloned());
        for (name, value) in [
            ("HOME", HOME),
            ("PATH", &paths.join(":")),
            ("XDG_CONFIG_HOME", "/home/yinz/.config"),
            ("XDG_CACHE_HOME", "/home/yinz/.cache"),
            ("XDG_DATA_HOME", "/home/yinz/.local/share"),
        ] {
            setenv(name, value)?;
        }
        Ok(data_files)
    }

    fn mother_command(
        &self,
        command: &mut Command,
        declaration: &Declaration,
        state: &[PathBuf],
        setenv: &mut impl FnMut(&str, &str) -> Result<()>,
        view: Option<&Path>,
    ) -> Result<Vec<File>> {
        self.validate_claude(declaration)
            .context(MotherSetupError)?;
        let data_files = Vec::new();
        command.args(["--ro-bind", "/", "/"]);
        private_mounts(command);
        if Path::new("/etc/resolv.conf").exists() {
            bind_mother_resolver(command, Path::new("/etc/resolv.conf"))?;
        }
        let work = self.writable_root(declaration);
        command.arg("--bind").arg(work).arg(work);
        for path in state {
            command.arg("--bind").arg(path).arg(path);
        }
        if let Some(claude) = &declaration.claude {
            command
                .arg("--bind")
                .arg(view.context("missing Claude configuration view")?)
                .arg(&claude.root);
            command.args([
                "--dir",
                "/run/yinz-claude/cache/claude-cli-nodejs",
                "--dir",
                "/run/yinz-claude/cache/claude/staging",
                "--dir",
                "/run/yinz-claude/state/claude/locks",
            ]);
            command
                .arg("--bind")
                .arg(self.home.join("projects"))
                .arg(claude.root.join("projects"));
        }
        for (name, value) in std::env::vars_os() {
            let (Some(name), Some(value)) = (name.to_str(), value.to_str()) else {
                continue;
            };
            if !name.starts_with("YINZ_") {
                setenv(name, value)?;
            }
        }
        declared_environment(declaration, setenv)?;
        setenv(
            "PATH",
            &format!(
                "/run/yinz-bin:{}",
                std::env::var("PATH").unwrap_or_default()
            ),
        )?;
        Ok(data_files)
    }

    fn command(
        &self,
        declaration: &Declaration,
        cwd: &Path,
        runtime: bool,
    ) -> Result<SandboxCommand> {
        #[cfg(target_os = "linux")]
        let view = declaration
            .claude
            .as_ref()
            .map(|claude| {
                let settings = self
                    .settings
                    .as_ref()
                    .context("missing Claude launch settings snapshot")
                    .context(MotherSetupError)?;
                claude_view::View::mount(&claude.root, settings.0.as_bytes())
            })
            .transpose()?;
        #[cfg(target_os = "linux")]
        let view_path = view.as_ref().map(|view| view.path());
        #[cfg(not(target_os = "linux"))]
        let view_path = None;
        let mut command = Command::new("bwrap");
        #[cfg(target_os = "linux")]
        if declaration.mother.is_some() {
            claude_view::guard_setup(&mut command)?;
        }
        command.args([
            "--unshare-all",
            "--share-net",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--clearenv",
        ]);
        let mut environment_args =
            tempfile::tempfile().context("create Bubblewrap environment arguments")?;
        environment_args
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("restrict Bubblewrap environment arguments")?;
        command
            .arg("--args")
            .arg(environment_args.as_raw_fd().to_string());
        let mut setenv = |name: &str, value: &str| -> Result<()> {
            if name.contains('\0') || value.contains('\0') {
                bail!("NUL in Bubblewrap environment argument");
            }
            for arg in ["--setenv", name, value] {
                environment_args
                    .write_all(arg.as_bytes())
                    .context("write Bubblewrap environment argument")?;
                environment_args
                    .write_all(&[0])
                    .context("terminate Bubblewrap environment argument")?;
            }
            Ok(())
        };
        let mut data_files = match &declaration.mother {
            Some(state) => {
                self.mother_command(&mut command, declaration, state, &mut setenv, view_path)?
            },
            None => self.project_command(&mut command, declaration, &mut setenv)?,
        };
        let mut mount_files = Vec::new();
        for (src, dst) in &declaration.mounts {
            let file = open_mount_source(src)?;
            let fd = file.as_raw_fd();
            if fs::read_link(format!("/proc/self/fd/{fd}")).context("verify opened mount source")?
                != *src
            {
                bail!(
                    "Bubblewrap source changed through a symlink; prepare again: {}",
                    src.display()
                );
            }
            command.arg("--ro-bind-fd").arg(fd.to_string()).arg(dst);
            mount_files.push(file);
        }
        for (name, value) in [
            ("TMPDIR", "/tmp"),
            ("XDG_RUNTIME_DIR", "/run/user"),
            ("YINZ_BUBBLEWRAP", "1"),
        ] {
            setenv(name, value)?;
        }
        for name in ["TERM", "COLORTERM", "LANG"] {
            if declaration.env.contains_key(name)
                || declaration.inherit.iter().any(|key| key == name)
            {
                continue;
            }
            if let Ok(value) = std::env::var(name) {
                setenv(name, &value)?;
            }
        }
        bind_binary(
            &mut command,
            self.writable_root(declaration),
            yinz_binary()?,
        );
        if runtime {
            let session = std::env::var("YINZ_SESSION_NAME")
                .context("Bubblewrap launch requires a Yinz session")?;
            if session.is_empty()
                || Path::new(&session).components().count() != 1
                || session == "."
                || session == ".."
            {
                bail!("invalid Yinz session name");
            }
            let destination = Path::new("/run/yinz-control")
                .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
            command
                .arg("--ro-bind")
                .arg(&*yinz_runtime::consts::YINZ_SOCK_DIR)
                .arg(destination);
            setenv("YINZ_SOCKET_DIR", "/run/yinz-control")?;
            if let Ok(reference) = std::env::var("YINZ_PANE_REF") {
                if reference.len() != 12 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    bail!("invalid YINZ_PANE_REF");
                }
            }

            for name in [
                "YINZ_PANE_REF",
                "YINZ_AGENT_THEME_REF",
                "YINZ_PANE_ID",
                "YINZ_SESSION_NAME",
                "YINZ_SWAMP_OWNER",
                "YINZ_AGENT_PANE",
            ] {
                if let Ok(value) = std::env::var(name) {
                    setenv(name, &value)?;
                }
            }
            let pi = yinz_runtime::agent::pi::pi_extension_path();
            if pi.exists() {
                command.arg("--ro-bind").arg(&pi).arg(&pi);
            }
        }
        // Retain private arguments and pinned mount sources until exec, inheriting them only in the child.
        unsafe {
            command.pre_exec(move || {
                environment_args.rewind()?;
                for file in &mut data_files {
                    file.rewind()?;
                }
                for file in std::iter::once(&environment_args)
                    .chain(&data_files)
                    .chain(&mount_files)
                {
                    if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        command.arg("--chdir").arg(cwd);
        Ok(SandboxCommand {
            command,
            #[cfg(target_os = "linux")]
            view,
        })
    }

    fn checks(
        &self,
        declaration: &Declaration,
        cwd: &Path,
        log: &File,
        deadline: Instant,
    ) -> Result<()> {
        // Probe the actual view inside the namespace, including its private write path.
        let probe = if declaration.claude.is_some() {
            vec!["/bin/sh".into(), "-eu".into(), "-c".into(),
                "cat \"$CLAUDE_CONFIG_DIR/.claude.json\" >/dev/null; p=$(mktemp \"$CLAUDE_CONFIG_DIR/.yinz-probe-XXXXXX\"); rm \"$p\"".into()]
        } else {
            vec!["/usr/bin/true".into()]
        };
        for args in std::iter::once(probe).chain(declaration.checks.iter().cloned()) {
            let result = (|| {
                if Instant::now() >= deadline {
                    bail!("check deadline expired");
                }
                let mut command = self.command(declaration, cwd, declaration.mother.is_some())?;
                command
                    .arg("--")
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(log.try_clone().context("clone check log")?)
                    .stderr(log.try_clone().context("clone check log")?)
                    .process_group(0);
                wait_check(&mut command, deadline)
            })();
            result.with_context(|| format!("Bubblewrap check failed: {}", args[0]))?;
        }
        Ok(())
    }

    fn execute(
        &self,
        declaration: &Declaration,
        cwd: &Path,
        args: &[String],
        log: Option<&File>,
    ) -> Result<()> {
        let mut command = self.command(declaration, cwd, false)?;
        command.arg("--").args(args).stdin(Stdio::null());
        if let Some(log) = log {
            command
                .stdout(log.try_clone().context("clone preparation log")?)
                .stderr(log.try_clone().context("clone preparation log")?);
        }
        if !command
            .status()
            .context("run Bubblewrap command")?
            .success()
        {
            bail!(
                "Bubblewrap command failed: {}; inspect preparation logs",
                args[0]
            );
        }
        Ok(())
    }

    pub fn prepare(&self, config_dir: Option<&Path>) -> Result<()> {
        let _lock = self.lock(true)?;
        // Invalidate before accepting or executing anything, including malformed replacement KDL.
        self.save(&Accepted {
            root: self.root.clone(),
            declaration: Declaration::default(),
            ready: false,
        })?;
        let mut log =
            open_log(&self.metadata.join("prepare.log")).context("open preparation log")?;
        writeln!(log, "Preparation {:?}", std::time::SystemTime::now())
            .context("write preparation log")?;
        let result: Result<()> = (|| {
            platform()?;
            let host_home = std::env::var_os("HOME").context("host HOME is not set")?;
            let declaration = selected(&self.root, config_dir, Path::new(&host_home))?;
            let mut accepted = Accepted {
                root: self.root.clone(),
                declaration,
                ready: false,
            };
            self.validate_storage(&accepted.declaration)?;
            self.save(&accepted)?;
            yinz_runtime::shared::ensure_secure_dir(&self.home)
                .context("create private Bubblewrap home")?;
            for (src, dst) in &accepted.declaration.seeds {
                let status = self
                    .command(&accepted.declaration, &self.root, false)?
                    .arg("--ro-bind")
                    .arg(src)
                    .arg("/run/yinz-seed")
                    .args([
                        "--",
                        "/usr/bin/install",
                        "-D",
                        "-m",
                        "600",
                        "/run/yinz-seed",
                    ])
                    .arg(dst)
                    .stdout(log.try_clone().context("clone log")?)
                    .stderr(log.try_clone().context("clone log")?)
                    .status()
                    .context("seed sandbox home")?;
                if !status.success() {
                    bail!("seeding failed; inspect {}", self.metadata.display());
                }
            }
            for args in &accepted.declaration.setup {
                self.execute(&accepted.declaration, &self.root, args, Some(&log))?;
            }
            self.checks(
                &accepted.declaration,
                &self.root,
                &log,
                Instant::now() + Duration::from_secs(10),
            )?;
            accepted.ready = true;
            self.save(&accepted)?;
            println!(
                "Prepared {} (logs: {})",
                self.root.display(),
                self.metadata.join("prepare.log").display()
            );
            Ok(())
        })();
        if let Err(error) = &result {
            writeln!(log, "Failed: {error:#}").context("record preparation failure")?;
        }
        result.with_context(|| {
            format!(
                "Bubblewrap preparation log: {}",
                self.metadata.join("prepare.log").display()
            )
        })
    }

    /// Everything a launch must pass before its pane's process exists: the platform, the
    /// accepted declaration, the launch directory, the declared checks, and `program`
    /// resolving on the sandbox's own `PATH`. Run by the server so a failure is the
    /// spawner's answer; [`Self::run`] repeats only what guards the exec itself.
    ///
    /// Mother probe failures request local fallback; project lookup failures report [`ProgramNotFound`].
    pub fn check_launch(&self, cwd: &Path, program: &Path) -> Result<LaunchCheck> {
        let _lock = self.lock(false)?;
        let accepted = self.accepted()?;
        self.validate_launch_cwd(cwd, &accepted.declaration)?;
        let mut launch = self.clone();
        launch.settings = accepted
            .declaration
            .claude
            .as_ref()
            .map(ClaudeState::settings)
            .transpose()?;
        match launch.launch_probes(&accepted.declaration, cwd, program) {
            Ok(()) => Ok(LaunchCheck::Ready(launch.settings)),
            Err(error)
                if accepted.declaration.mother.is_some() && !error.is::<MotherSetupError>() =>
            {
                Ok(LaunchCheck::MotherFallback(format!("{error:#}")))
            },
            Err(error) => Err(error),
        }
    }

    fn launch_probes(&self, declaration: &Declaration, cwd: &Path, program: &Path) -> Result<()> {
        platform()?;
        let path = self.metadata.join("launch.log");
        let log = open_log(&path).context("open launch check log")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        self.checks(declaration, cwd, &log, deadline)
            .with_context(|| format!("Launch checks failed; inspect {}", path.display()))?;
        let mut command = self.command(declaration, cwd, declaration.mother.is_some())?;
        command
            .args([
                "--",
                "/bin/sh",
                "-c",
                "command -v -- \"$1\" >/dev/null",
                "sh",
            ])
            .arg(program)
            .stdin(Stdio::null())
            .stdout(log.try_clone().context("clone check log")?)
            .stderr(log.try_clone().context("clone check log")?)
            .process_group(0);
        // The checks above proved the sandbox itself starts, so a failure here is the lookup's.
        if !wait_status(&mut command, deadline)
            .context("resolve the launch program")?
            .success()
        {
            return Err(anyhow!(ProgramNotFound))
                .with_context(|| format!("{} is not on the sandbox PATH", program.display()));
        }
        Ok(())
    }

    pub fn run(&self, args: &[String]) -> Result<()> {
        if args.is_empty() {
            bail!("missing sandbox command");
        }
        let lock = self.lock(false)?;
        let accepted = self.accepted()?;
        let cwd = std::env::current_dir().context("read launch directory")?;
        self.validate_launch_cwd(&cwd, &accepted.declaration)?;
        let mut launch = self.clone();
        if let Some(claude) = &accepted.declaration.claude {
            launch.settings = Some(
                match std::env::var(yinz_types::input::command::CLAUDE_SETTINGS_FD) {
                    Ok(fd) => {
                        use std::os::fd::FromRawFd;
                        let fd: i32 = fd.parse().context("invalid settings descriptor")?;
                        if fd < 3 {
                            bail!("invalid settings descriptor");
                        }
                        let mut file = unsafe { File::from_raw_fd(fd) };
                        let mut contents = String::new();
                        file.read_to_string(&mut contents)
                            .context("read launch settings snapshot")?;
                        ClaudeSettings(contents)
                    },
                    Err(_) => claude.settings()?,
                },
            );
        }
        let mut command = launch.command(&accepted.declaration, &cwd, true)?;
        // Bubblewrap retains this shared lease through its PID namespace's lifetime.
        let fd = lock.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } == -1 {
            return Err(std::io::Error::last_os_error()).context("inherit environment lease");
        }
        command
            .arg("--sync-fd")
            .arg(fd.to_string())
            .arg("--")
            .args(args);
        #[cfg(target_os = "linux")]
        if command.view.is_some() {
            use std::os::unix::process::ExitStatusExt;
            let result = supervise(&mut command);
            drop(command);
            let status = result?;
            if let Some(signal) = status.signal() {
                unsafe {
                    libc::signal(signal, libc::SIG_DFL);
                    libc::raise(signal);
                }
                std::process::exit(128 + signal);
            }
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(command.exec()).context("start sandboxed agent")
    }
}

#[cfg(target_os = "linux")]
fn supervise(sandbox: &mut SandboxCommand) -> Result<std::process::ExitStatus> {
    let mut signals = signal_hook::iterator::Signals::new([
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
    ])
    .context("watch Claude sandbox signals")?;
    let mut child = sandbox.spawn().context("start sandboxed Claude mother")?;
    let mut terminating = None;
    let result = (|| loop {
        if let Some(status) = child.try_wait().context("wait for Claude sandbox")? {
            break Ok(status);
        }
        if sandbox.view.as_ref().is_some_and(|view| view.finished()) {
            child
                .kill()
                .context("stop sandbox after Claude filesystem service failure")?;
            child
                .wait()
                .context("reap sandbox after Claude filesystem service failure")?;
            break Err(anyhow!(
                "Claude configuration filesystem service stopped; sandbox terminated"
            ));
        }
        for signal in signals.pending() {
            if unsafe { libc::kill(child.id() as i32, signal) } == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error).context("forward Claude sandbox signal");
                }
            }
            if matches!(signal, libc::SIGHUP | libc::SIGTERM) {
                terminating.get_or_insert(Instant::now());
            }
        }
        if terminating.is_some_and(|start| start.elapsed() >= Duration::from_secs(2)) {
            child
                .kill()
                .context("terminate unresponsive Claude sandbox")?;
        }
        std::thread::sleep(Duration::from_millis(10));
    })();
    if result.is_err() {
        child
            .kill()
            .context("stop failed Claude sandbox")
            .non_fatal();
        child
            .wait()
            .context("reap failed Claude sandbox")
            .non_fatal();
    }
    result
}

fn wait_check(command: &mut Command, deadline: Instant) -> Result<()> {
    let status = wait_status(command, deadline)?;
    if !status.success() {
        bail!("check exited with {status}");
    }
    Ok(())
}

/// Run a check to completion, killing its process group at `deadline`.
fn wait_status(command: &mut Command, deadline: Instant) -> Result<std::process::ExitStatus> {
    let mut child = command.spawn().context("start check")?;
    let failure = loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(
                    Duration::from_millis(10)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            },
            Ok(None) => break anyhow!("check timed out"),
            Err(error) => break anyhow!(error).context("wait for check"),
        }
    };
    let killed = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let kill_error = (killed == -1).then(std::io::Error::last_os_error);
    let reaped = child.wait().context("reap failed check");
    if let Some(error) = kill_error {
        if error.raw_os_error() != Some(libc::ESRCH) {
            reaped?;
            return Err(error).context(format!("{failure:#}; terminate check process group"));
        }
    }
    reaped?;
    Err(failure)
}

#[cfg(test)]
#[path = "unit/bubblewrap.rs"]
mod tests;
