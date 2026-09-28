//! The host half of one agent poll.
//!
//! Thin wrappers over host calls. The parts where drift would be a real bug —
//! classification and the manifest-to-`AgentPane` join — live in `hocus-proto`
//! and are shared. These stay local because sharing them would mean pulling all
//! of hocus's tool dispatch into a daemon that never runs a tool.

use hocus_proto::projects::{Repository, WorktreeStatus};
use std::collections::BTreeMap;
use yinz_tile::prelude::SessionName;
use yinz_tile::prelude::*;

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
pub fn pane_running_command(pane_id: u32) -> Option<String> {
    get_pane_running_command(PaneId::Terminal(yinz_tile::prelude::TerminalId::new(
        pane_id,
    )))
    .ok()
    .filter(|args| !args.is_empty())
    .map(|args| args.join(" "))
}

pub fn pane_cwd(pane_id: u32) -> Option<String> {
    get_pane_cwd(PaneId::Terminal(yinz_tile::prelude::TerminalId::new(
        pane_id,
    )))
    .ok()
    .map(|path| path.to_string_lossy().into_owned())
}

/// What Git said about a remote pane's folder, from its own machine.
///
/// `Ok(None)` means the directory is pending; an error means unavailable.
pub fn pane_git_probe(
    pane_id: u32,
    cwd: &str,
    refresh_token: Option<&str>,
) -> Result<Option<PaneGitProbe>, String> {
    get_pane_git_probe_with_refresh(
        PaneId::Terminal(yinz_tile::prelude::TerminalId::new(pane_id)),
        cwd,
        refresh_token.map(str::to_owned),
    )
}

/// One poll's reading of a pane: what it shows, and what that reading is worth.
///
/// The text alone cannot say whether an agent is working. A pane repaints when the user
/// scrolls it, when it is resized, and when a client attaches — so the host flags
/// travel with the text they qualify, sampled in the same call rather than looked up
/// afterwards against a manifest that may already have moved on.
pub struct PaneSample {
    pub text: String,
    /// The host answered with a screen. `false` means it did not — the pane has closed, or
    /// the call failed — and then `text` is an error message rather than anything the agent
    /// wrote, so nothing may be concluded from it and nothing may be summarised out of it.
    pub readable: bool,
    /// The user has scrolled this pane back off its live tail, so what it shows is old
    /// output rather than anything the process just did.
    pub is_scrolled: bool,
    /// Milliseconds since the pane's process last wrote to its pty, if it ever has.
    /// `None` means it never wrote anything this session.
    pub last_output_ms_ago: Option<yinz_tile::prelude::ElapsedMillis>,
}

impl PaneSample {
    /// A reading we could not take: the host refused — most often because the pane has
    /// closed — so this text is an error message rather than anything the agent wrote, and
    /// it must neither look like work nor be summarised. `readable` is what says so, and
    /// the daemon reads it as "this says nothing" — see `State::qualify`.
    fn unreadable(reason: String) -> Self {
        PaneSample {
            text: reason,
            readable: false,
            is_scrolled: false,
            last_output_ms_ago: None,
        }
    }
}

/// What a pane is showing. Only hashed for activity; trailing empty rows are not
/// content and must not reset the idle counter when the pane is resized.
pub fn read_pane(pane_id: u32) -> PaneSample {
    match get_pane_scrollback(
        PaneId::Terminal(yinz_tile::prelude::TerminalId::new(pane_id)),
        false,
    ) {
        Ok(contents) => {
            let is_scrolled = contents.is_scrolled;
            let last_output_ms_ago = contents.last_output_ms_ago;
            let mut lines = contents.lines_above_viewport;
            lines.extend(contents.viewport);
            lines.extend(contents.lines_below_viewport);
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            let text = if lines.is_empty() {
                "(pane is empty)".to_owned()
            } else {
                lines.join("\n")
            };
            PaneSample {
                text,
                readable: true,
                is_scrolled,
                last_output_ms_ago,
            }
        },
        Err(error) => PaneSample::unreadable(format!("could not read pane: {error}")),
    }
}

/// One startup snapshot for panes that existed before this plugin loaded.
///
/// Later changes come from `PaneUpdate`, `CommandChanged`, and `CwdChanged`; this
/// is deliberately not a repeating poll. The manifest includes hidden panes and tab
/// membership, so a quiet session is complete even before its first update.
pub struct StartupSnapshot {
    pub session_name: SessionName,
    pub manifest: PaneManifest,
    pub tabs: Vec<TabInfo>,
}

pub fn gather() -> Option<StartupSnapshot> {
    let mut session = current_session()?;
    // ponytail: one blocking host round-trip per terminal pane at plugin startup.
    // Events keep this fresh afterwards; batch only if startup ever feels slow.
    for pane in session.panes.panes.values_mut().flatten() {
        let PaneId::Terminal(id) = pane.id else {
            continue;
        };
        if let Some(command) = pane_running_command(id.get()) {
            pane.terminal_command = Some(command);
        }
        if let Some(cwd) = pane_cwd(id.get()) {
            pane.cwd = Some(cwd);
        }
    }
    Some(StartupSnapshot {
        session_name: session.name,
        manifest: session.panes,
        tabs: session.tabs,
    })
}

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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checkout out of an outcome, or a panic naming what came back instead.
    fn checkout(outcome: ProbeOutcome) -> Repository {
        match outcome {
            ProbeOutcome::Checkout(repository) => repository,
            other => panic!("expected a checkout, got {other:?}"),
        }
    }

    #[test]
    fn repository_prefers_push_default_and_detects_a_linked_worktree() {
        let repository = checkout(repository(
            Some(0),
            b"/worktrees/topic\n/repos/hocus/.git/worktrees/topic\n/repos/hocus/.git\ntopic\n",
            b"remote.origin.url\nhttps://example/origin\0remote.fork.url\nhttps://example/fork\0remote.pushdefault\nfork\0branch.topic.remote\norigin\0",
        ));

        assert_eq!(repository.root, "/worktrees/topic");
        assert_eq!(repository.remote.as_deref(), Some("https://example/fork"));
        assert_eq!(repository.worktree_status, Some(WorktreeStatus::Linked));
        assert_eq!(repository.branch.as_deref(), Some("topic"));
    }

    /// A detached HEAD is on no branch, and the word Git answers with is not the
    /// name of one.
    #[test]
    fn repository_reports_a_detached_head_as_no_branch() {
        let repository = checkout(repository(
            Some(0),
            b"/repos/hocus\n/repos/hocus/.git\n/repos/hocus/.git\nHEAD\n",
            b"remote.origin.url\nhttps://example/origin\0",
        ));

        assert_eq!(repository.worktree_status, Some(WorktreeStatus::Primary));
        assert_eq!(repository.branch, None);
    }

    /// The case that silently denied a whole repository its project: `git
    /// rev-parse` writes the identity and *then* fails on the unborn ref, so the
    /// exit code alone says "not a repository" about a repository.
    #[test]
    fn a_checkout_with_no_commits_keeps_its_identity() {
        let repository = checkout(repository(
            Some(128),
            b"/work/fresh\n/work/fresh/.git\n/work/fresh/.git\nHEAD\n",
            b"",
        ));

        assert_eq!(repository.root, "/work/fresh");
        assert_eq!(repository.worktree_status, Some(WorktreeStatus::Primary));
        assert_eq!(repository.common_dir.as_deref(), Some("/work/fresh/.git"));
        assert_eq!(repository.branch, None);
        assert!(repository.is_checkout(), "a project can be made from it");
    }

    /// Git ran and answered. The folder is not a repository, and saying so is
    /// what stops the daemon probing it again on every event.
    #[test]
    fn a_folder_outside_a_repository_is_conclusively_not_one() {
        assert_eq!(
            repository(Some(128), b"", b""),
            ProbeOutcome::NotARepository
        );
    }

    /// Nothing was learned. Recording this as a folder would be a guess that
    /// outlives the reason for it.
    #[test]
    fn a_probe_that_could_not_run_is_not_recorded() {
        assert_eq!(repository(None, b"", b""), ProbeOutcome::Failed);
        assert_eq!(repository(Some(2), b"", b""), ProbeOutcome::Failed);
    }

    /// Output that is not what we asked for is not something to guess at: a
    /// relative root would name a project after whatever its last part happened
    /// to be.
    #[test]
    fn output_that_is_not_the_shape_we_asked_for_is_a_failure() {
        assert_eq!(
            repository(Some(0), b"not/absolute\n.git\n.git\nmain\n", b""),
            ProbeOutcome::Failed
        );
    }
}
