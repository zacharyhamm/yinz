use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use yinz_bus::screen::ScreenInstruction;
use yinz_runtime::channels::SenderWithContext;
use yinz_runtime::errors::prelude::*;

#[cfg(test)]
#[path = "../../unit/agent_worktrees_tests.rs"]
mod tests;

#[derive(Default)]
pub(in crate::screen) struct Worktrees {
    entries: HashMap<PathBuf, Lookup>,
    generation: u64,
}

struct Lookup {
    generation: u64,
    root: Option<PathBuf>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Lookup {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl Worktrees {
    fn refresh(
        &mut self,
        directories: HashSet<PathBuf>,
        to_screen: Option<SenderWithContext<ScreenInstruction>>,
    ) {
        self.entries.retain(|cwd, _| directories.contains(cwd));
        let Some(to_screen) = to_screen else { return };
        for cwd in directories {
            if self.entries.contains_key(&cwd) {
                continue;
            }
            self.generation += 1;
            let generation = self.generation;
            let lookup_cwd = cwd.clone();
            let to_screen = to_screen.clone();
            let task = crate::global_async_runtime::get_tokio_runtime().spawn(async move {
                let root = match tokio::time::timeout(
                    Duration::from_secs(5),
                    resolve_worktree(&lookup_cwd),
                )
                .await
                {
                    Ok(Ok(root)) => Some(root),
                    _ => None,
                };
                to_screen
                    .send(ScreenInstruction::AgentWorktreeResolved {
                        cwd: lookup_cwd,
                        generation,
                        root,
                    })
                    .context("reporting an agent worktree to the screen")
                    .non_fatal();
            });
            self.entries.insert(
                cwd,
                Lookup {
                    generation,
                    root: None,
                    task: Some(task),
                },
            );
        }
    }

    pub(in crate::screen) fn complete(
        &mut self,
        cwd: &Path,
        generation: u64,
        root: Option<PathBuf>,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(cwd) else {
            return false;
        };
        if entry.generation != generation {
            return false;
        }
        entry.root = root;
        entry.task = None;
        true
    }
}

async fn resolve_worktree(cwd: &Path) -> Result<PathBuf> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .kill_on_drop(true)
        .output()
        .await
        .context("resolving an agent's Git worktree")?;
    if !output.status.success() {
        return Err(anyhow!("agent directory has no Git worktree"));
    }
    let mut root = output.stdout;
    if root.last() == Some(&b'\n') {
        root.pop();
    }
    tokio::fs::canonicalize(PathBuf::from(OsString::from_vec(root)))
        .await
        .context("canonicalizing an agent's Git worktree")
}

impl super::AgentController {
    pub(in crate::screen) fn refresh_agent_worktrees(
        &mut self,
        host: &mut impl super::host::AgentHost,
    ) {
        let directories = host.local_agent_directories();
        self.agent_worktrees.refresh(
            directories.values().cloned().collect(),
            host.worktree_sender(),
        );
        self.publish_roots(host, directories);
    }
    pub(in crate::screen) fn resolve_agent_worktree(
        &mut self,
        host: &mut impl super::host::AgentHost,
        cwd: PathBuf,
        generation: u64,
        root: Option<PathBuf>,
    ) -> bool {
        let directories = host.local_agent_directories();
        self.agent_worktrees.refresh(
            directories.values().cloned().collect(),
            host.worktree_sender(),
        );
        let accepted = self.agent_worktrees.complete(&cwd, generation, root);
        self.publish_roots(host, directories);
        accepted
    }
    fn publish_roots(
        &self,
        host: &mut impl super::host::AgentHost,
        directories: HashMap<yinz_types::pane_id::PaneId, PathBuf>,
    ) {
        let roots = directories
            .into_iter()
            .filter_map(|(id, cwd)| {
                self.agent_worktrees
                    .entries
                    .get(&cwd)?
                    .root
                    .clone()
                    .map(|root| (id, root))
            })
            .collect();
        host.set_worktree_roots(roots);
    }
}
