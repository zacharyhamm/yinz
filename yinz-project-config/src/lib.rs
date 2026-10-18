//! Project configuration is data. Reading it never launches a configured command or grants permissions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yinz_types::configuration::Node;

#[cfg(unix)]
pub mod bubblewrap;

/// A stable directory name for per-checkout state: the SHA-256 of `root`'s path, in hex.
#[cfg(not(target_family = "wasm"))]
pub fn root_key(root: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(root.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub default_branch: Option<String>,
    /// Plugin-owned top-level nodes, excluding the host's `agent_exit` block.
    #[serde(default)]
    pub sections: BTreeMap<String, Node>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            default_branch: Some("main".into()),
            sections: BTreeMap::new(),
        }
    }
}

pub fn parse(source: &str) -> Result<ProjectConfig, String> {
    let doc: kdl::KdlDocument = source
        .parse()
        .map_err(|e| format!(".yinz/agent-exit.kdl: {e}"))?;
    let mut config = ProjectConfig::default();
    let mut agent_exit = None;
    for node in doc.nodes() {
        let name = node.name().value();
        let duplicate = if name == "agent_exit" {
            agent_exit.replace(node).is_some()
        } else {
            config
                .sections
                .insert(name.into(), section(node)?)
                .is_some()
        };
        if duplicate {
            return Err(format!(".yinz/agent-exit.kdl: duplicate section {name}"));
        }
    }
    let Some(node) = agent_exit else {
        return Ok(config);
    };
    if !node.entries().is_empty() || node.ty().is_some() {
        return Err("agent_exit takes a block, not arguments or a type".into());
    }
    let children = node.children().ok_or("agent_exit requires a block")?;
    if children.nodes().len() > 1 {
        return Err("agent_exit accepts only one default_branch".into());
    }
    if let Some(node) = children.nodes().first() {
        if node.name().value() != "default_branch"
            || node.children().is_some()
            || node.ty().is_some()
            || node.entries().len() != 1
        {
            return Err("agent_exit: expected default_branch \"branch\"".into());
        }
        let entry = &node.entries()[0];
        let branch = entry
            .value()
            .as_string()
            .ok_or("default_branch must be a string")?;
        if entry.name().is_some() || entry.ty().is_some() || !valid_branch(branch) {
            return Err("agent_exit: invalid default_branch".into());
        }
        config.default_branch = Some(branch.to_owned());
    }
    Ok(config)
}

fn section(node: &kdl::KdlNode) -> Result<Node, String> {
    fn value(value: &kdl::KdlValue) -> Result<serde_json::Value, String> {
        Ok(if let Some(value) = value.as_string() {
            value.into()
        } else if let Some(value) = value.as_bool() {
            value.into()
        } else if let Some(value) = value.as_i64() {
            value.into()
        } else if let Some(value) = value.as_f64() {
            if !value.is_finite() {
                return Err(".yinz/agent-exit.kdl: section numbers must be finite".into());
            }
            value.into()
        } else {
            serde_json::Value::Null
        })
    }
    Ok(Node {
        name: node.name().value().into(),
        arguments: node
            .entries()
            .iter()
            .filter(|e| e.name().is_none())
            .map(|e| value(e.value()))
            .collect::<Result<_, _>>()?,
        properties: node
            .entries()
            .iter()
            .filter_map(|e| {
                e.name()
                    .map(|n| value(e.value()).map(|v| (n.value().into(), v)))
            })
            .collect::<Result<_, _>>()?,
        children: node
            .children()
            .map(|c| c.nodes().iter().map(section).collect::<Result<_, _>>())
            .transpose()?
            .unwrap_or_default(),
    })
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with(['-', '/', '.'])
        && !branch.ends_with(['/', '.'])
        && !branch.contains([' ', '\\', '~', '^', ':', '?', '*', '['])
        && !branch.chars().any(char::is_control)
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.contains("//")
        && branch != "@"
        && branch != "HEAD"
        && branch
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: std::path::PathBuf,
    pub source: Option<String>,
    pub config: ProjectConfig,
    pub error: Option<String>,
}

#[cfg(not(target_family = "wasm"))]
pub fn root(directory: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::process::Command;
    let git = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()
        .map_err(|e| e.to_string())?;
    if !git.status.success() {
        return Err(format!(
            "No usable Git worktree: {}",
            String::from_utf8_lossy(&git.stderr)
        ));
    }
    let listing = String::from_utf8(git.stdout).map_err(|e| e.to_string())?;
    let primary = listing
        .split('\0')
        .next()
        .and_then(|line| line.strip_prefix("worktree "))
        .ok_or("Git returned no primary checkout")?;
    std::path::Path::new(primary)
        .canonicalize()
        .map_err(|e| e.to_string())
}

#[cfg(not(target_family = "wasm"))]
pub fn access(
    directory: &std::path::Path,
    write: Option<(&str, Option<&str>)>,
) -> Result<Snapshot, String> {
    access_root(&root(directory)?, write)
}

/// Access the root already resolved and authorized by the host. Do not resolve Git again
/// after the permission check: repository configuration can change between calls.
#[cfg(not(target_family = "wasm"))]
pub fn access_root(
    root: &std::path::Path,
    write: Option<(&str, Option<&str>)>,
) -> Result<Snapshot, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::{fs, io::Read, os::unix::fs::OpenOptionsExt};
    let directory = root.join(".yinz");
    if write.is_some() {
        match fs::create_dir(&directory) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(e) => return Err(format!("Cannot create .yinz: {e}")),
        }
    }
    let directory = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(&directory)
    {
        Ok(file) => Some(file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && write.is_none() => None,
        Err(e) => {
            return Err(format!(
                "Cannot open .yinz directory (symlinks are refused): {e}"
            ))
        },
    };
    let read = || -> Result<Option<String>, String> {
        let Some(directory) = directory.as_ref() else {
            return Ok(None);
        };
        // SAFETY: the live directory descriptor and literal C string remain valid for openat.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                c"agent-exit.kdl".as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(format!(
                "Cannot read .yinz/agent-exit.kdl (must be a regular file, not a symlink): {e}"
            ));
        }
        // SAFETY: openat returned a new owned descriptor.
        let file = unsafe { fs::File::from_raw_fd(fd) };
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err(".yinz/agent-exit.kdl must be a regular file".into());
        }
        let mut source = String::new();
        file.take(65537)
            .read_to_string(&mut source)
            .map_err(|e| e.to_string())?;
        if source.len() > 65536 {
            return Err(".yinz/agent-exit.kdl exceeds 64 KiB".into());
        }
        Ok(Some(source))
    };
    if let Some((source, expected)) = write {
        if source.len() > 65536 {
            return Err(".yinz/agent-exit.kdl exceeds 64 KiB".into());
        }
        parse(source)?;
        use std::io::Write;
        let git = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .map_err(|e| format!("Cannot resolve Git metadata: {e}"))?;
        if !git.status.success() {
            return Err(format!(
                "Cannot resolve Git metadata: {}",
                String::from_utf8_lossy(&git.stderr)
            ));
        }
        let gitdir = std::path::PathBuf::from(
            String::from_utf8(git.stdout)
                .map_err(|e| e.to_string())?
                .strip_suffix('\n')
                .ok_or("Git returned no path terminator")?,
        );
        // Keep this inode: unlinking it would let concurrent writers lock different files.
        // The OS releases the lock even when a writer is killed.
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(gitdir.join("agent-exit.kdl.lock"))
            .map_err(|e| format!("Cannot lock .yinz/agent-exit.kdl for writing: {e}"))?;
        lock.try_lock()
            .map_err(|e| format!("Cannot lock .yinz/agent-exit.kdl for writing: {e}"))?;
        if read()?.as_deref() != expected {
            return Err(
                ".yinz/agent-exit.kdl changed; read it again before approving a new write".into(),
            );
        }
        let mut temp = tempfile::NamedTempFile::new_in(gitdir).map_err(|e| e.to_string())?;
        temp.write_all(source.as_bytes())
            .map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(temp.path().as_os_str().as_bytes())
            .map_err(|e| e.to_string())?;
        let directory = directory
            .as_ref()
            .ok_or("Missing configuration directory")?;
        // SAFETY: both paths and the directory descriptor are live for this atomic rename.
        if unsafe {
            libc::renameat(
                libc::AT_FDCWD,
                from.as_ptr(),
                directory.as_raw_fd(),
                c"agent-exit.kdl".as_ptr(),
            )
        } != 0
        {
            return Err(format!(
                "Cannot atomically replace .yinz/agent-exit.kdl: {}",
                std::io::Error::last_os_error()
            ));
        }
        directory.sync_all().map_err(|e| e.to_string())?;
    }
    let source = read()?;
    let (config, error) = match parse(source.as_deref().unwrap_or("")) {
        Ok(config) => (config, None),
        Err(error) => (ProjectConfig::default(), Some(error)),
    };
    Ok(Snapshot {
        root: root.to_path_buf(),
        source,
        config,
        error,
    })
}

#[cfg(test)]
#[path = "unit/config.rs"]
mod tests;
