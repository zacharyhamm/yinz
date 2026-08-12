//! The host half of one agent poll.
//!
//! Thin wrappers over host calls. The parts where drift would be a real bug —
//! classification and the manifest-to-`AgentPane` join — live in `hocus-proto`
//! and are shared. These stay local because sharing them would mean pulling all
//! of hocus's tool dispatch into a daemon that never runs a tool.

use hocus_proto::agents;
use hocus_proto::panes::terminal_panes;
use hocus_proto::projects::{Repository, WorktreeStatus};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

fn current_session() -> Option<SessionInfo> {
    get_session_list()
        .ok()?
        .live_sessions
        .into_iter()
        .find(|session| session.is_current_session)
}

/// What a pane is running right now, as one command line.
///
/// `PaneInfo.terminal_command` is only what a pane was *launched* with, so it is
/// `None` for every pane the user opened as a shell. The host resolves this one from
/// the pane's foreground process instead, which is what detection actually wants.
fn pane_running_command(pane_id: u32) -> Option<String> {
    get_pane_running_command(PaneId::Terminal(pane_id))
        .ok()
        .filter(|args| !args.is_empty())
        .map(|args| args.join(" "))
}

fn pane_cwd(pane_id: u32) -> Option<String> {
    get_pane_cwd(PaneId::Terminal(pane_id))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

/// What a pane is showing. Only hashed for activity; trailing empty rows are not
/// content and must not reset the idle counter when the pane is resized.
pub fn read_pane_text(pane_id: u32) -> String {
    match get_pane_scrollback(PaneId::Terminal(pane_id), false) {
        Ok(contents) => {
            let mut lines = contents.lines_above_viewport;
            lines.extend(contents.viewport);
            lines.extend(contents.lines_below_viewport);
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            if lines.is_empty() {
                "(pane is empty)".to_owned()
            } else {
                lines.join("\n")
            }
        },
        Err(error) => format!("could not read pane: {error}"),
    }
}

/// One startup snapshot for panes that existed before this plugin loaded.
///
/// Later changes come from `PaneUpdate`, `CommandChanged`, and `CwdChanged`; this
/// is deliberately not a repeating poll.
pub fn gather() -> Option<Vec<agents::AgentPane>> {
    let session = current_session()?;
    let mut panes = terminal_panes(&session.panes, &session.tabs);
    // ponytail: one blocking host round-trip per terminal pane at plugin startup.
    // Events keep this fresh afterwards; batch only if startup ever feels slow.
    for pane in &mut panes {
        if let Some(command) = pane_running_command(pane.pane_id) {
            pane.command = Some(command);
        }
        pane.cwd = pane_cwd(pane.pane_id);
    }
    Some(panes)
}

/// Turn the two Git command results into the checkout identity discovery needs.
pub fn repository(cwd: &str, metadata: &[u8], config: &[u8]) -> Repository {
    let lines: Vec<&str> = std::str::from_utf8(metadata)
        .unwrap_or_default()
        .lines()
        .collect();
    let [root, git_dir, common_dir, branch] = lines.as_slice() else {
        return Repository::folder(cwd);
    };
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

    Repository {
        root: (*root).to_owned(),
        remote: preferred.filter(|url| !url.is_empty()).map(str::to_owned),
        worktree_status: Some(if git_dir == common_dir {
            WorktreeStatus::Primary
        } else {
            WorktreeStatus::Linked
        }),
        common_dir: Some((*common_dir).to_owned()),
        // `--abbrev-ref HEAD` answers the literal word `HEAD` on a detached
        // checkout, which names no branch and would read as one called HEAD.
        branch: (*branch != "HEAD" && !branch.is_empty()).then(|| (*branch).to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_prefers_push_default_and_detects_a_linked_worktree() {
        let repository = repository(
            "/worktrees/topic/src",
            b"/worktrees/topic\n/repos/hocus/.git/worktrees/topic\n/repos/hocus/.git\ntopic\n",
            b"remote.origin.url\nhttps://example/origin\0remote.fork.url\nhttps://example/fork\0remote.pushdefault\nfork\0branch.topic.remote\norigin\0",
        );

        assert_eq!(repository.root, "/worktrees/topic");
        assert_eq!(repository.remote.as_deref(), Some("https://example/fork"));
        assert_eq!(repository.worktree_status, Some(WorktreeStatus::Linked));
        assert_eq!(repository.branch.as_deref(), Some("topic"));
    }

    /// A detached HEAD is on no branch, and the word Git answers with is not the
    /// name of one.
    #[test]
    fn repository_reports_a_detached_head_as_no_branch() {
        let repository = repository(
            "/repos/hocus",
            b"/repos/hocus\n/repos/hocus/.git\n/repos/hocus/.git\nHEAD\n",
            b"remote.origin.url\nhttps://example/origin\0",
        );

        assert_eq!(repository.worktree_status, Some(WorktreeStatus::Primary));
        assert_eq!(repository.branch, None);
    }
}
