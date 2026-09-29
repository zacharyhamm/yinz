//! The host half of one agent poll.
//!
//! Thin wrappers over host calls. The parts where drift would be a real bug —
//! classification and the manifest-to-`AgentPane` join — live in `hocus-proto`
//! and are shared. These stay local because sharing them would mean pulling all
//! of hocus's tool dispatch into a daemon that never runs a tool.

pub use hocus_proto::git_probe::{repository, ProbeOutcome};
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
