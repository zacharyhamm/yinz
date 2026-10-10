//! Explicitly accepted, persistent user-space environments. Launch never reads project KDL.
use kdl::{KdlDocument, KdlNode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use yinz_runtime::errors::prelude::*;

/// Protect variables that establish the sandbox boundary, including against spawn's env wrapper.
pub fn validate_spawn_env(env: &BTreeMap<String, String>) -> Result<()> {
    for name in env.keys() {
        if reserved_env(name) && !matches!(name.as_str(), "YINZ_AGENT_PANE" | "YINZ_SWAMP_OWNER") {
            bail!("Bubblewrap spawn cannot override {name}");
        }
    }
    Ok(())
}

pub const RUN_FLAG: &str = "--bubblewrap-run";
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
];

#[derive(Debug, Default, Serialize, Deserialize)]
struct Declaration {
    mounts: Vec<(PathBuf, PathBuf)>,
    paths: Vec<String>,
    env: BTreeMap<String, String>,
    inherit: Vec<String>,
    seeds: Vec<(PathBuf, PathBuf)>,
    setup: Vec<Vec<String>>,
    checks: Vec<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct Accepted {
    root: PathBuf,
    declaration: Declaration,
    ready: bool,
}

pub struct Environment {
    root: PathBuf,
    metadata: PathBuf,
    home: PathBuf,
}

fn platform() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("Bubblewrap agent environments require Linux");
    }
    let status = Command::new("bwrap")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .context("Bubblewrap is required; install bwrap")?;
    if !status.success() {
        bail!("bwrap --version failed");
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
    fn validate(&self, root: &Path) -> Result<()> {
        let mut destinations = vec![root.to_path_buf()];
        destinations.extend(SYSTEM.iter().chain(ETC).map(PathBuf::from));
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
                    let project = absolute(path)?
                        .canonicalize()
                        .context("resolve global Bubblewrap project")?;
                    if !roots.insert(project.clone()) {
                        bail!("duplicate global Bubblewrap project {}", project.display());
                    }
                    let body = node.children().context("project requires a block")?;
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

impl Environment {
    fn at(root: PathBuf, data: &Path) -> Self {
        let key: String = Sha256::digest(root.as_os_str().as_encoded_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            root,
            metadata: data.join("environments").join(&key),
            home: data.join("homes").join(key),
        }
    }

    pub fn for_project(project: &Path) -> Result<Self> {
        let root = crate::root(project).map_err(|error| anyhow!(error))?;
        let env = Self::at(root, &yinz_runtime::home::xdg_data_dir().join("bubblewrap"));
        env.validate_cwd(project)?;
        Ok(env)
    }

    fn validate_cwd(&self, cwd: &Path) -> Result<()> {
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
        for flag in ["--absolute-git-dir", "--git-common-dir"] {
            let output = Command::new("git")
                .arg("-C")
                .arg(&cwd)
                .args(["rev-parse", "--path-format=absolute", flag])
                .output()
                .context("locate Git metadata")?;
            if !output.status.success() {
                bail!("cannot resolve required Git metadata");
            }
            let path = PathBuf::from(
                String::from_utf8(output.stdout)
                    .context("decode Git path")?
                    .trim(),
            );
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
        fs::create_dir_all(&self.metadata).context("create Bubblewrap metadata directory")?;
        fs::set_permissions(&self.metadata, fs::Permissions::from_mode(0o700))
            .context("protect Bubblewrap metadata")?;
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
        if overlaps(&self.root, &metadata)
            || (self.home.exists()
                && overlaps(
                    &self.home.canonicalize().context("resolve private home")?,
                    &metadata,
                ))
            || declaration
                .mounts
                .iter()
                .any(|(source, _)| overlaps(source, &metadata))
        {
            bail!("Bubblewrap mounts must not expose environment management files");
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
        Ok(accepted)
    }

    pub fn preflight(&self) -> Result<()> {
        platform()?;
        let _lock = self.lock(false)?;
        let accepted = self.accepted()?;
        self.checks(&accepted.declaration, &self.root, None)
    }

    pub fn wrap(&self, command: &mut yinz_types::command::CommandToRun) -> Result<()> {
        if !command.target.is_local() {
            bail!("Bubblewrap cannot be combined with SSH or Firecracker");
        }
        self.validate_cwd(command.cwd.as_deref().context("missing launch directory")?)?;
        let mut args = vec![
            RUN_FLAG.into(),
            self.root.to_string_lossy().into_owned(),
            "--".into(),
            command.path.to_string_lossy().into_owned(),
        ];
        args.append(&mut command.args);
        command.path = std::env::current_exe().context("locate Yinz executable")?;
        command.args = args;
        Ok(())
    }

    fn command(&self, declaration: &Declaration, cwd: &Path, runtime: bool) -> Result<Command> {
        let mut command = Command::new("bwrap");
        command.args([
            "--unshare-all",
            "--share-net",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--clearenv",
        ]);
        for path in SYSTEM.iter().chain(ETC) {
            if Path::new(path).exists() {
                command.args(["--ro-bind", path, path]);
            }
        }
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
        command.arg("--bind").arg(&self.root).arg(&self.root);
        command.arg("--bind").arg(&self.home).arg(HOME);
        let mut mount_files = Vec::new();
        for (src, dst) in &declaration.mounts {
            let file = File::open(src).context("open accepted Bubblewrap source")?;
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
        // Pin accepted sources so a concurrent symlink swap cannot expose another host path.
        unsafe {
            command.pre_exec(move || {
                for file in &mount_files {
                    if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut paths = vec![
            "/run/yinz-bin".to_owned(),
            "/usr/local/bin".into(),
            "/usr/bin".into(),
            "/bin".into(),
        ];
        paths.extend(declaration.paths.iter().cloned());
        for (name, value) in &declaration.env {
            command.args(["--setenv", name, value]);
        }
        for name in &declaration.inherit {
            let value = std::env::var(name)
                .with_context(|| format!("missing inherited variable {name}"))?;
            command.args(["--setenv", name, &value]);
        }
        for (name, value) in [
            ("HOME", HOME),
            ("PATH", &paths.join(":")),
            ("TMPDIR", "/tmp"),
            ("XDG_RUNTIME_DIR", "/run/user"),
            ("XDG_CONFIG_HOME", "/home/yinz/.config"),
            ("XDG_CACHE_HOME", "/home/yinz/.cache"),
            ("XDG_DATA_HOME", "/home/yinz/.local/share"),
            ("YINZ_BUBBLEWRAP", "1"),
        ] {
            command.args(["--setenv", name, value]);
        }
        for name in ["TERM", "COLORTERM", "LANG"] {
            if declaration.env.contains_key(name)
                || declaration.inherit.iter().any(|key| key == name)
            {
                continue;
            }
            if let Ok(value) = std::env::var(name) {
                command.args(["--setenv", name, &value]);
            }
        }
        {
            let binary = std::env::current_exe().context("locate Yinz executable")?;
            command.arg("--ro-bind").arg(&binary).arg(&binary);
            command
                .arg("--ro-bind")
                .arg(&binary)
                .arg("/run/yinz-bin/yinz");
        }
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
            command.args(["--setenv", "YINZ_SOCKET_DIR", "/run/yinz-control"]);
            if let Ok(reference) = std::env::var("YINZ_PANE_REF") {
                if reference.len() != 12 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    bail!("invalid YINZ_PANE_REF");
                }
            }

            for name in [
                "YINZ_PANE_REF",
                "YINZ_PANE_ID",
                "YINZ_SESSION_NAME",
                "YINZ_SWAMP_OWNER",
                "YINZ_AGENT_PANE",
            ] {
                if let Ok(value) = std::env::var(name) {
                    command.args(["--setenv", name, &value]);
                }
            }
            let pi = yinz_runtime::agent::pi::pi_extension_path();
            if pi.exists() {
                command.arg("--ro-bind").arg(&pi).arg(&pi);
            }
        }
        command.arg("--chdir").arg(cwd);
        Ok(command)
    }

    fn checks(&self, declaration: &Declaration, cwd: &Path, log: Option<&File>) -> Result<()> {
        // Even an empty declaration must prove namespaces and mandatory mounts work.
        self.execute(declaration, cwd, &["/usr/bin/true".into()], log)?;
        for args in &declaration.checks {
            self.execute(declaration, cwd, args, log)?;
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
        if self.metadata.join("accepted.json").exists() {
            let bytes = fs::read(self.metadata.join("accepted.json"))
                .context("read previous declaration")?;
            let mut previous: Accepted =
                serde_json::from_slice(&bytes).context("decode previous declaration")?;
            previous.ready = false;
            self.save(&previous)?;
        }
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.metadata.join("prepare.log"))
            .context("open preparation log")?;
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
            fs::create_dir_all(&self.home).context("create private Bubblewrap home")?;
            fs::set_permissions(&self.home, fs::Permissions::from_mode(0o700))
                .context("protect Bubblewrap home")?;
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
            self.checks(&accepted.declaration, &self.root, Some(&log))?;
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

    pub fn run(&self, args: &[String]) -> Result<()> {
        platform()?;
        if args.is_empty() {
            bail!("missing sandbox command");
        }
        let lock = self.lock(false)?;
        let accepted = self.accepted()?;
        let cwd = std::env::current_dir().context("read launch directory")?;
        self.validate_cwd(&cwd)?;
        self.checks(&accepted.declaration, &cwd, None)?;
        let mut command = self.command(&accepted.declaration, &cwd, true)?;
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
        Err(command.exec()).context("start sandboxed agent")
    }
}

#[cfg(test)]
#[path = "unit/bubblewrap.rs"]
mod tests;
