//! Decode cached Git metadata shared by project discovery and agent control.
use crate::projects::{Repository, WorktreeStatus};
use std::collections::BTreeMap;

/// What one folder's Git probe concluded.
///
/// Three outcomes rather than two, because "Git could not tell us" and "Git told
/// us this is not a repository" must not be cached the same way. Recording a
/// failure as a plain folder denies the pane a project for as long as the entry
/// lives, and nothing would ever ask again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Git claimed the folder.
    Checkout(Repository),
    /// Git ran and said this is not a repository.
    NotARepository,
    /// The probe told us nothing — git is missing, or could not be spawned.
    Failed,
}

/// Git's exit code for "not a git repository", which is also what it answers for
/// a ref that does not resolve.
const NOT_A_REPOSITORY: i32 = 128;

/// Turn the two Git command results into the checkout identity discovery needs.
///
/// The exit code does not gate the parse. A checkout with no commits writes its
/// full identity to stdout and *then* exits 128, because `--abbrev-ref HEAD`
/// cannot resolve an unborn branch — so refusing to read stdout on failure
/// records a real repository as an ordinary folder. The identity is the first
/// three lines; the branch is a fourth line that a repository need not have.
pub fn repository(exit_code: Option<i32>, metadata: &[u8], config: &[u8]) -> ProbeOutcome {
    let lines: Vec<&str> = std::str::from_utf8(metadata)
        .unwrap_or_default()
        .lines()
        .collect();
    let [root, git_dir, common_dir] = lines.as_slice()[..lines.len().min(3)] else {
        return if exit_code == Some(NOT_A_REPOSITORY) {
            ProbeOutcome::NotARepository
        } else {
            ProbeOutcome::Failed
        };
    };
    // Git answers absolute paths here because of `--path-format=absolute`.
    // Anything else is not the output we asked for, and guessing at it would put
    // a nonsense root in the registry under a name taken from its last component.
    if !std::path::Path::new(root).is_absolute() {
        return ProbeOutcome::Failed;
    }
    // `--abbrev-ref HEAD` answers the literal word `HEAD` on a detached
    // checkout, which names no branch and would read as one called HEAD. A
    // checkout with no commits prints it too, and has no branch either.
    let branch = lines
        .get(3)
        .filter(|branch| **branch != "HEAD" && !branch.is_empty())
        .map(|branch| (*branch).to_owned());
    let entries: BTreeMap<&str, &str> = config
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok()?.split_once('\n'))
        .collect();
    let urls: BTreeMap<&str, &str> = entries
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("remote.")
                .and_then(|key| key.strip_suffix(".url"))
                .map(|name| (name, value.trim()))
        })
        .collect();
    let url = |name: &&str| (*name != ".").then(|| urls.get(*name).copied()).flatten();
    let preferred = entries
        .get("remote.pushdefault")
        .and_then(url)
        .or_else(|| {
            let branch = branch.as_deref()?;
            entries
                .get(format!("branch.{branch}.remote").as_str())
                .and_then(url)
        })
        .or_else(|| urls.get("origin").copied())
        .or_else(|| {
            (urls.len() == 1)
                .then(|| urls.values().next().copied())
                .flatten()
        });

    ProbeOutcome::Checkout(Repository {
        root: root.to_owned(),
        remote: preferred.filter(|url| !url.is_empty()).map(str::to_owned),
        worktree_status: Some(if git_dir == common_dir {
            WorktreeStatus::Primary
        } else {
            WorktreeStatus::Linked
        }),
        common_dir: Some(common_dir.to_owned()),
        branch,
        // The parse says nothing about which machine Git ran on — it is handed
        // bytes. Whoever asked stamps that on, in `record_probe_outcome`.
        host: None,
        ..Repository::default()
    })
}

#[cfg(test)]
#[path = "unit/git_probe.rs"]
mod tests;
