//! Workflow configuration is data. Reading it never launches a command or grants permissions.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub ci: Option<Vec<String>>,
    pub default_branch: Option<String>,
    pub agent: Option<crate::data::AgentSpawnSpec>,
}

pub fn parse(source: &str) -> Result<ProjectConfig, String> {
    let doc: kdl::KdlDocument = source.parse().map_err(|e| format!(".zellij.kdl: {e}"))?;
    let mut config = ProjectConfig::default();
    if doc.nodes().is_empty() {
        return Ok(config);
    }
    if doc.nodes().len() != 1 || doc.nodes()[0].name().value() != "agent_exit" {
        return Err(".zellij.kdl must contain only one agent_exit block".into());
    }
    let node = &doc.nodes()[0];
    if !node.entries().is_empty() || node.ty().is_some() {
        return Err("agent_exit takes a block, not arguments or a type".into());
    }
    let children = node.children().ok_or("agent_exit requires a block")?;
    let mut seen = std::collections::BTreeSet::new();
    for node in children.nodes() {
        let name = node.name().value();
        if !seen.insert(name) || node.children().is_some() || node.ty().is_some() {
            return Err(format!("agent_exit: duplicate or malformed {name}"));
        }
        let mut args = Vec::new();
        let mut properties = std::collections::BTreeMap::new();
        for entry in node.entries() {
            let value = entry
                .value()
                .as_string()
                .ok_or_else(|| format!("{name}: all values must be strings"))?;
            if entry.ty().is_some() || value.contains('\0') {
                return Err(format!("{name}: types and NUL bytes are not allowed"));
            }
            if let Some(key) = entry.name() {
                if properties.insert(key.value(), value).is_some() {
                    return Err(format!("{name}: duplicate property {}", key.value()));
                }
            } else {
                args.push(value.to_owned());
            }
        }
        match name {
            "ci" if properties.is_empty() && !args.is_empty() && !args[0].trim().is_empty() => config.ci = Some(args),
            "default_branch" if properties.is_empty() && args.len() == 1 && valid_branch(&args[0]) => config.default_branch = args.pop(),
            "agent" if args.is_empty() && properties.keys().all(|k| ["kind", "model", "effort"].contains(k)) => {
                if properties.values().any(|v| v.trim().is_empty()) {
                    return Err("agent: settings must not be blank".into());
                }
                let spec = crate::data::AgentSpawnSpec {
                    kind: properties.get("kind").map(|v| v.to_string()),
                    model: properties.get("model").map(|v| v.to_string()),
                    effort: properties.get("effort").map(|v| v.to_string()),
                    ..Default::default()
                };
                if let Some(kind) = spec.kind.as_deref() {
                    if !crate::agent::launch::OFFERED.contains(&kind) {
                        return Err(format!("agent kind {kind:?}: use codex, claude, or cursor"));
                    }
                    crate::agent::launch::plan(&spec)?;
                } else if spec.effort.is_some() && !crate::agent::launch::OFFERED.iter().any(|kind| {
                    crate::agent::launch::plan(&crate::data::AgentSpawnSpec { kind: Some((*kind).into()), ..spec.clone() }).is_ok()
                }) {
                    return Err("agent effort is unsupported; specify a supported kind, model, and effort".into());
                }
                config.agent = Some(spec);
            },
            _ => return Err(format!("agent_exit: invalid {name}; expected ci \"program\" \"arg\", default_branch \"branch\", or agent kind=\"codex\" model=\"...\" effort=\"medium\"")),
        }
    }
    Ok(config)
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
}

#[cfg(not(target_family = "wasm"))]
pub fn root(directory: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::process::Command;
    let git = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| e.to_string())?;
    if !git.status.success() {
        return Err(format!(
            "No usable Git worktree: {}",
            String::from_utf8_lossy(&git.stderr)
        ));
    }
    let root = std::path::PathBuf::from(
        String::from_utf8(git.stdout)
            .map_err(|e| e.to_string())?
            .strip_suffix('\n')
            .ok_or("Git returned no path terminator")?,
    );
    root.canonicalize().map_err(|e| e.to_string())
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
    use std::{fs, io::Read, os::unix::fs::OpenOptionsExt};
    let path = root.join(".zellij.kdl");
    let read = || -> Result<Option<String>, String> {
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "Cannot read .zellij.kdl (must be a regular file, not a symlink): {e}"
                ))
            },
        };
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err(".zellij.kdl must be a regular file".into());
        }
        let mut source = String::new();
        file.take(65537)
            .read_to_string(&mut source)
            .map_err(|e| e.to_string())?;
        if source.len() > 65536 {
            return Err(".zellij.kdl exceeds 64 KiB".into());
        }
        Ok(Some(source))
    };
    if let Some((source, expected)) = write {
        if source.len() > 65536 {
            return Err(".zellij.kdl exceeds 64 KiB".into());
        }
        parse(source)?;
        let lock_path = root.join(".zellij.kdl.lock");
        let mut lock = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|e| format!("Cannot lock .zellij.kdl for writing: {e}"))?;
        let result = (|| {
            use std::io::Write;
            if read()?.as_deref() != expected {
                return Err(
                    ".zellij.kdl changed; read it again before approving a new write".into(),
                );
            }
            lock.write_all(source.as_bytes())
                .map_err(|e| e.to_string())?;
            lock.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&lock_path, &path).map_err(|e| e.to_string())
        })();
        let _ = fs::remove_file(&lock_path);
        result?;
    }
    let source = read()?;
    let config = parse(source.as_deref().unwrap_or(""))?;
    Ok(Snapshot {
        root: root.to_path_buf(),
        source,
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_workflow_settings_without_executing_them() {
        let config = parse("agent_exit { ci \"cargo\" \"x\" \"ci\" \"check\"; agent kind=\"codex\" effort=\"medium\"; default_branch \"trunk\"; }").unwrap();
        assert_eq!(config.ci.unwrap(), ["cargo", "x", "ci", "check"]);
        assert_eq!(config.default_branch.as_deref(), Some("trunk"));
        for source in [
            "hooks {}",
            "agent_exit { ci; }",
            "agent_exit { ci 1; }",
            "agent_exit { agent effort=\"unsupported\"; }",
            "agent_exit { default_branch \"HEAD\"; }",
            "agent_exit { default_branch \"--bad\"; }",
            "agent_exit { agent kind=\"unknown\"; }",
            "agent_exit { ci \"x\"; ci \"y\"; }",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }
    #[test]
    fn stale_writes_and_symlinks_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .arg("init")
            .arg(dir.path())
            .output()
            .unwrap()
            .status
            .success());
        let source = "agent_exit { ci \"true\"; }";
        access(dir.path(), Some((source, None))).unwrap();
        assert!(access(dir.path(), Some(("", None))).is_err());
        assert_eq!(
            access(dir.path(), None).unwrap().source.as_deref(),
            Some(source)
        );
        access(dir.path(), Some(("", Some(source)))).unwrap();
        std::fs::remove_file(dir.path().join(".zellij.kdl")).unwrap();
        std::os::unix::fs::symlink("missing", dir.path().join(".zellij.kdl")).unwrap();
        assert!(access(dir.path(), None).is_err());
    }
}
