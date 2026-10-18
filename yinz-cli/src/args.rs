use crate::options::CliOptions;
use crate::setup::Setup;
use clap::builder::styling::{AnsiColor, Color, Style, Styles};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;
use yinz_types::geometry::{Direction, Resize};
use yinz_types::input::options::Options;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use {
    yinz_runtime::consts::YINZ_CONFIG_DIR_ENV, yinz_runtime::consts::YINZ_CONFIG_FILE_ENV,
    yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::options::PaneFrameStyle,
};
use {yinz_types::data::InputMode, yinz_types::data::UnblockCondition};

const fn ansi(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(color)))
}

const CLI_STYLES: Styles = Styles::styled()
    .header(ansi(AnsiColor::Yellow))
    .usage(ansi(AnsiColor::Yellow))
    .literal(ansi(AnsiColor::Green))
    .placeholder(Style::new())
    .error(ansi(AnsiColor::Red))
    .valid(ansi(AnsiColor::Green))
    .invalid(ansi(AnsiColor::Yellow));

fn validate_session(name: &str) -> Result<String, String> {
    #[cfg(unix)]
    {
        use yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH;

        let mut socket_path = yinz_runtime::consts::YINZ_SOCK_DIR.clone();
        socket_path.push(name);

        if socket_path.as_os_str().len() >= YINZ_SOCK_MAX_LENGTH {
            // socket path must be less than 108 bytes
            let available_length = YINZ_SOCK_MAX_LENGTH
                .saturating_sub(socket_path.as_os_str().len())
                .saturating_sub(1);

            return Err(format!(
                "session name must be less than {} characters",
                available_length
            ));
        };
    };

    Ok(name.to_owned())
}

#[derive(Parser, Default, Debug, Clone, Serialize, Deserialize)]
#[clap(
    version,
    name = "yinz",
    about = "A terminal workspace with batteries included",
    styles = CLI_STYLES,
    args_override_self = true
)]
pub struct CliArgs {
    /// Maximum panes on screen, caution: opening more panes will close old ones
    #[clap(long, value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    pub max_panes: Option<usize>,

    /// Change where yinz looks for plugins
    #[clap(long, value_parser, overrides_with = "data_dir")]
    pub data_dir: Option<PathBuf>,

    /// Run server listening at the specified socket path
    #[clap(long, value_parser, hide = true, overrides_with = "server")]
    pub server: Option<PathBuf>,

    /// Internal: original client terminal settings, before raw mode (JSON or null).
    #[clap(long, hide = true, requires = "server")]
    pub server_termios: Option<String>,

    /// Internal: run as a pane's pty supervisor, reading its control channel from
    /// fd 3. Spawned by the server, never by a user. The value is a
    /// `<session>/<pane>` label that only exists to make `ps` readable.
    #[clap(long, value_parser, hide = true, overrides_with = "pty_host")]
    pub pty_host: Option<String>,

    /// Specify name of a new session
    #[clap(long, short, overrides_with = "session", value_parser = validate_session)]
    pub session: Option<String>,

    /// Name of a predefined layout inside the layout directory or the path to a layout file
    /// if inside a session (or using the --session flag) will be added to the session as a new tab
    /// or tabs, otherwise will start a new session
    #[clap(short, long, value_parser, overrides_with = "layout")]
    pub layout: Option<PathBuf>,

    /// Raw KDL layout string to use directly (instead of a file path)
    /// if inside a session (or using the --session flag) will be added to the session as a new tab
    /// or tabs, otherwise will start a new session
    #[clap(long, value_parser, conflicts_with_all = &["layout", "new_session_with_layout"])]
    pub layout_string: Option<String>,

    /// Name of a predefined layout inside the layout directory or the path to a layout file
    /// Will always start a new session, even if inside an existing session
    #[clap(short, long, value_parser, overrides_with = "new_session_with_layout")]
    pub new_session_with_layout: Option<PathBuf>,

    /// Change where yinz looks for the configuration file
    #[clap(short, long, overrides_with = "config", env = YINZ_CONFIG_FILE_ENV, value_parser)]
    pub config: Option<PathBuf>,

    /// Change where yinz looks for the configuration directory
    #[clap(long, overrides_with = "config_dir", env = YINZ_CONFIG_DIR_ENV, value_parser)]
    pub config_dir: Option<PathBuf>,

    #[clap(subcommand)]
    pub command: Option<Command>,

    /// Specify emitting additional debug information
    #[clap(short, long, value_parser)]
    pub debug: bool,
}

impl CliArgs {
    pub fn is_setup_clean(&self) -> bool {
        if let Some(Command::Setup(ref setup)) = &self.command {
            if setup.clean {
                return true;
            }
        }
        false
    }
    pub fn options(&self) -> Option<Options> {
        if let Some(Command::Options(options)) = &self.command {
            return Some((**options).clone().into());
        }
        None
    }
}

/// Authoring Steel Scheme plugins in the user's plugin folder from a shell.
/// Extensionless names use .scm; explicit .scm, .ss and .sld names retain their extension.
#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Plugins {
    /// Install configured repository plugins; omit aliases to select all.
    Install { aliases: Vec<String> },
    /// Update installed branches; tags and commits stay pinned. Reload explicitly to use updates.
    Update { aliases: Vec<String> },
    /// Show repository sources and installed revisions without fetching.
    Status { aliases: Vec<String> },
    /// Read bundled plugin documentation without a source checkout.
    Docs {
        #[clap(default_value = "index")]
        section: String,
    },
    /// List source files, or session plugin lifecycle records with --running.
    List {
        #[clap(long)]
        running: bool,
        #[clap(long, requires = "running")]
        json: bool,
    },
    /// Start a plugin in the background, reusing an existing identity without changing focus.
    Start {
        name: String,
        /// Select the exact configuration as a JSON object of string values.
        #[clap(long)]
        configuration_json: Option<String>,
        #[clap(long)]
        json: bool,
    },
    /// Stop all client copies of a plugin identity and remove its registrations.
    Stop {
        name: String,
        /// Select the exact configuration as a JSON object of string values.
        #[clap(long)]
        configuration_json: Option<String>,
        #[clap(long)]
        json: bool,
    },
    /// Apply an exact-text edit JSON array, then format and check before writing.
    Edit {
        name: String,
        #[clap(long)]
        file: PathBuf,
    },
    /// Run adjacent plugin unit and integration tests using the installed runtime.
    Test {
        #[clap(required_unless_present = "file")]
        name: Option<String>,
        #[clap(long, conflicts_with = "name")]
        file: Option<PathBuf>,
        #[clap(long, default_value = "all", value_parser = ["unit", "integration", "all"])]
        kind: String,
        #[clap(long)]
        filter: Option<String>,
        #[clap(long, default_value_t = 30_000)]
        timeout_ms: u64,
        #[clap(long)]
        json: bool,
    },
    /// Write a Scheme plugin from a file or stdin, after formatting and source checks. Refuses to replace one that
    /// exists unless --overwrite is given.
    Write {
        name: String,
        /// The source; stdin when left out.
        #[clap(short, long, value_parser)]
        file: Option<PathBuf>,
        #[clap(long)]
        overwrite: bool,
        /// Skip authoring formatting and checks.
        #[clap(long)]
        no_check: bool,
    },
    /// Check a Scheme plugin by managed name, alias, absolute path, ./path, ../path,
    /// file: URL, or --file.
    Check {
        name: Option<String>,
        #[clap(short, long, value_parser, conflicts_with = "name")]
        file: Option<PathBuf>,
        #[clap(long)]
        json: bool,
    },
    /// Format Scheme source; --check only verifies formatting.
    Format {
        name: Option<String>,
        #[clap(short, long, value_parser, conflicts_with = "name")]
        file: Option<PathBuf>,
        #[clap(long)]
        check: bool,
    },
    /// Reload an existing identity in place. Missing plugins require `plugins start`.
    /// Accepts a managed name, alias, path, URL or plugin_N.
    Reload {
        name: String,
        /// Compatibility spelling. All reloads preserve existing placement.
        #[clap(long)]
        headless: bool,
        /// Select the exact configuration as a JSON object of string values.
        #[clap(long)]
        configuration_json: Option<String>,
        #[clap(long)]
        json: bool,
    },
    /// Report runtime status and historical logs by name, alias, path, URL or plugin_N.
    Log {
        name: String,
        #[clap(short, long, default_value = "50")]
        lines: usize,
        /// Select the exact configuration as a JSON object of string values.
        #[clap(long)]
        configuration_json: Option<String>,
        #[clap(long)]
        json: bool,
    },
    /// Where plugins are written and read from.
    Dir,
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Data {
    /// Discover shared and private entities and their schema owners.
    Catalog {
        #[clap(long)]
        namespace: Option<String>,
    },
    /// Fetch a page using existing FetchRequest JSON.
    Fetch {
        #[clap(long)]
        namespace: String,
        #[clap(long)]
        file: PathBuf,
    },
    /// Count matching rows using existing FetchRequest JSON.
    Count {
        #[clap(long)]
        namespace: String,
        #[clap(long)]
        file: PathBuf,
    },
    /// Apply a Mutation JSON array atomically. Replacements require revision guards.
    Apply {
        #[clap(long)]
        namespace: String,
        #[clap(long)]
        file: PathBuf,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Skills {
    List,
    Show {
        name: String,
    },
    Install {
        names: Vec<String>,
        /// Install for one agent (default: all agents). --dest overrides its skills directory.
        #[clap(long, value_enum)]
        agent: Option<HookAgent>,
        /// Install directly into this skills directory instead of agent defaults.
        #[clap(long)]
        dest: Option<PathBuf>,
        #[clap(long)]
        dry_run: bool,
        #[clap(long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum HookAgent {
    Claude,
    Codex,
    Opencode,
    Muse,
    Agy,
    Pi,
    All,
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Hooks {
    /// Write hook scripts and add them to the agents' settings.
    ///
    /// Idempotent: running it again rewrites the script and replaces the entries it
    /// put there before, leaving everything else in the file alone.
    Install {
        /// Install for one agent. User-level installs default to every agent; a project
        /// install defaults to Claude because only Claude Code has a machine-local
        /// project file.
        #[clap(long, value_name = "AGENT_OR_SCHEME_PLUGIN")]
        agent: Option<String>,
        /// Install into this project's `.claude/settings.local.json` (not committed)
        /// instead of `~/.claude/settings.json`.
        ///
        /// Claude Code merges hooks across both rather than one overriding the
        /// other, so an install in each fires every hook twice. An older install in
        /// the project's committed `settings.json` is moved out.
        #[clap(long)]
        project: bool,
        /// The project folder to install for, instead of the current one. Implies
        /// `--project`.
        #[clap(long, value_name = "DIR")]
        project_dir: Option<PathBuf>,
        /// Print what would be written, and write nothing.
        #[clap(long)]
        dry_run: bool,
    },
    /// Remove the hook entries and the script.
    Uninstall {
        /// Uninstall for one agent. User-level uninstalls default to every agent; a project
        /// uninstall defaults to Claude.
        #[clap(long, value_name = "AGENT_OR_SCHEME_PLUGIN")]
        agent: Option<String>,
        /// Uninstall from this project's `.claude/settings.local.json` (and its
        /// committed `settings.json`, if an older install is still in it).
        #[clap(long)]
        project: bool,
        /// The project folder to uninstall from, instead of the current one. Implies
        /// `--project`.
        #[clap(long, value_name = "DIR")]
        project_dir: Option<PathBuf>,
    },
    /// Say what is wired up, and what would stop it working.
    Status {
        /// Show one agent instead of every one.
        #[clap(long, value_name = "AGENT_OR_SCHEME_PLUGIN")]
        agent: Option<String>,
    },
}

/// Unix-only, because `yinz_server::pty_host` is: an ungated variant would
/// still parse elsewhere, find no handler in `main`, fall past the whole
/// dispatch chain and quietly start a normal session instead.
#[cfg(unix)]
#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum PtyHosts {
    /// List the pane supervisors that are currently running.
    List {
        /// Print a JSON array instead of a table.
        #[clap(long)]
        json: bool,
        /// List panes held by the yinz broker on another machine instead of
        /// supervisors on this one.
        ///
        /// A remote pane's supervisor is on that machine, so it leaves nothing
        /// here to enumerate — the broker holding it is the only thing that can
        /// say what survived.
        #[clap(long)]
        host: Option<String>,
    },
    /// Terminate one supervisor and the process it is running.
    Kill {
        /// Supervisor ID from `yinz pty-host list`. A unique prefix is enough.
        host_id: String,
    },
    /// Terminate every supervisor, and every process they are running.
    KillAll,
    /// Adopt a surviving pane into a session, putting it back on screen.
    ///
    /// The counterpart to `list`: what to do about a supervisor that outlived the
    /// server that started it.
    Adopt {
        /// Supervisor ID from `yinz pty-host list`. A unique prefix is enough.
        ///
        /// With `--host`, this is a pane token from
        /// `yinz pty-host list --host <machine>` instead, and must be given in
        /// full: a token means something only to the broker that issued it, and
        /// this side has nothing to match a prefix against.
        host_id: String,
        /// Adopt a pane held by the yinz broker on this machine, rather than a
        /// supervisor on the local one.
        #[clap(long)]
        host: Option<String>,
        /// Session to adopt the pane into. Defaults to the current session.
        #[clap(long, short)]
        session: Option<String>,
        /// Land the pane in the session's focused tab instead of a new tab.
        #[clap(long, conflicts_with = "floating")]
        focused_tab: bool,
        /// Land the pane floating in the session's focused tab.
        #[clap(long, conflicts_with = "focused_tab")]
        floating: bool,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Command {
    /// Evaluate Steel Scheme in a running session, interactively, from stdin, or with --eval.
    ///
    /// Use --session NAME to select a session. Otherwise, use the current session,
    /// or the sole running session when outside Yinz. Outside Yinz, no or multiple
    /// running sessions is an error.
    Repl {
        /// Evaluate one expression or program and exit.
        #[clap(short = 'e', long)]
        eval: Option<String>,
    },
    /// Prepare a persistent project Bubblewrap environment (Linux).
    #[clap(subcommand)]
    Bubblewrap(Bubblewrap),
    /// Find, launch and control agents in explicit terminal panes.
    // Running servers from before the rename still launch `yinz hocus codex`.
    #[clap(subcommand, alias = "hocus")]
    Agent(Agent),

    /// Inspect and install portable coding-agent skills.
    #[clap(subcommand)]
    Skills(Skills),
    /// Query and atomically update the selected session's plugin data.
    #[clap(subcommand)]
    Data(Data),

    /// Change the behaviour of yinz
    #[clap(name = "options", value_parser)]
    Options(Box<CliOptions>),

    /// Setup yinz and check its configuration
    #[clap(name = "setup", value_parser)]
    Setup(Setup),

    /// Send actions to a specific session
    #[clap(visible_alias = "ac")]
    #[clap(subcommand)]
    Action(Box<CliAction>),

    /// Explore existing yinz sessions
    #[clap(flatten)]
    Sessions(Sessions),

    /// Subscribe to pane render updates (viewport and scrollback)
    #[clap(override_usage(
        "yinz [--session <OTHER SESSION NAME>] subscribe [OPTIONS] --pane-id..."
    ))]
    Subscribe(SubscribeCli),

    /// Stream the session's state events as JSON lines: what plugins see as PaneUpdate,
    /// TabUpdate, CommandPaneExited, CwdChanged and the rest. One object per line,
    /// `{"event": <type>, "payload": <the event>}`, until the session ends.
    #[clap(override_usage("yinz [--session <OTHER SESSION NAME>] events [--type <EVENT>]..."))]
    Events(EventsCli),

    /// Write, check, reload and read logs of Scheme plugins.
    #[clap(subcommand)]
    Plugins(Plugins),

    /// Install, inspect or remove coding-agent hooks.
    #[clap(subcommand)]
    Hooks(Hooks),

    /// Inspect and clean up detached pane supervisors. Panes outlive a server
    /// that was killed rather than shut down, and this is how you find them.
    #[cfg(unix)]
    #[clap(subcommand, name = "pty-host")]
    PtyHosts(PtyHosts),
}

/// A terminal pane named the way `action` documents it: `terminal_7`, or `7` for short.
/// Plugin panes are refused, because no agent runs in one.
pub(crate) fn parse_terminal_pane_number(value: &str) -> Result<u32, String> {
    match value.parse() {
        Ok(yinz_types::pane_id::PaneId::Terminal(id)) => Ok(id.get()),
        Ok(yinz_types::pane_id::PaneId::Plugin(_)) => Err(
            "expected a terminal pane ID, such as terminal_7 or 7; plugin panes do not run agents"
                .to_owned(),
        ),
        Err(error) => Err(format!(
            "invalid terminal pane ID {value:?} (expected terminal_7 or 7): {error}"
        )),
    }
}

fn parse_agent_target(value: &str) -> Result<yinz_types::pane_id::PaneId, String> {
    parse_terminal_pane_number(value).map(|id| {
        yinz_types::pane_id::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(id))
    })
}

impl Agent {
    /// Whether the caller asked for machine-readable output, including for errors. Archive
    /// replies are JSON either way.
    pub fn json(&self) -> bool {
        match self {
            Agent::Claude { .. }
            | Agent::CodexUntracked { .. }
            | Agent::Codex { .. }
            | Agent::CodexReport
            | Agent::Muse { .. }
            | Agent::MuseReport => false,
            Agent::Archive(archive) => archive.json(),
            Agent::Watch { json, .. }
            | Agent::Register { json, .. }
            | Agent::List { json, .. }
            | Agent::Status { json, .. }
            | Agent::Spawn { json, .. }
            | Agent::Prompt { json, .. }
            | Agent::Wait { json, .. }
            | Agent::Read { json, .. }
            | Agent::Recover { json, .. }
            | Agent::ClearDraft { json, .. }
            | Agent::SendKeys { json, .. } => *json,
        }
    }
}

fn parse_agent_timeout(value: &str) -> Result<std::time::Duration, String> {
    let timeout = humantime::parse_duration(value).map_err(|error| error.to_string())?;
    if timeout < std::time::Duration::from_millis(1)
        || timeout > std::time::Duration::from_millis(yinz_types::agent::control::MAX_TIMEOUT_MS)
    {
        return Err("timeout must be between 1ms and 24h".to_owned());
    }
    Ok(timeout)
}

fn parse_agent_state(value: &str) -> Result<yinz_types::agent::AgentState, String> {
    yinz_types::agent::AgentState::parse_lenient(value)
        .ok_or_else(|| "expected busy, waiting, idle or exited".to_owned())
}

fn parse_submission_id(value: &str) -> Result<String, String> {
    yinz_types::agent::control::validate_submission_id(value).map_err(str::to_owned)?;
    Ok(value.to_owned())
}

#[derive(Debug, Default, Parser, Clone, Serialize, Deserialize)]
pub struct AgentWatchFilters {
    #[clap(long, value_parser = parse_agent_target)]
    pub pane: Vec<yinz_types::pane_id::PaneId>,
    #[clap(long)]
    pub project: Vec<String>,
    #[clap(long)]
    pub kind: Vec<String>,
    #[clap(long)]
    pub lifecycle_type: Vec<String>,
    #[clap(long)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Subcommand)]
pub enum Bubblewrap {
    /// Accept the current declaration and rerun its provisioning and checks.
    Prepare {
        #[clap(long)]
        project: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Agent {
    /// Run Claude with a launch-time source palette snapshot.
    Claude {
        #[clap(long, default_value = "claude")]
        command: PathBuf,
        #[clap(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[clap(hide = true)]
    CodexUntracked {
        #[clap(long, default_value = "codex")]
        command: PathBuf,
        #[clap(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run Codex with pane tracking while retaining its shared background daemon.
    Codex {
        #[clap(long, default_value = "codex")]
        command: PathBuf,
        #[clap(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Receive a Codex hook and route it by its conversation id.
    #[clap(hide = true)]
    CodexReport,
    /// Run Muse with pane tracking, binding its session id back to this pane.
    Muse {
        #[clap(long, default_value = "muse")]
        command: PathBuf,
        #[clap(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Receive a Muse hook and route it by its session id.
    #[clap(hide = true)]
    MuseReport,

    /// Follow normalized agent lifecycle events in this session.
    Watch {
        #[clap(flatten)]
        filters: AgentWatchFilters,
        #[clap(long)]
        json: bool,
    },
    #[clap(flatten)]
    Archive(crate::agent_archive::AgentArchive),
    /// Register a terminal pane through the persistent agent service.
    Register {
        /// The terminal pane, such as terminal_7 or 7.
        #[clap(value_parser = parse_terminal_pane_number)]
        pane_id: u32,
        kind: String,
        /// Report errors as JSON too; successful registration always returns JSON.
        #[clap(long)]
        #[serde(default)]
        json: bool,
    },
    /// List agents and their reported lifecycle state.
    List {
        #[clap(long)]
        project: Option<String>,
        #[clap(long)]
        json: bool,
    },
    /// Inspect an agent's identity, readiness and reporting source.
    Status {
        /// Terminal pane ID, such as terminal_7 or 7.
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        /// Require this agent conversation to still occupy the pane.
        #[clap(long)]
        conversation: Option<String>,
        #[clap(long)]
        json: bool,
    },
    /// Launch an agent with the shared launch planner and return its actual pane and checkout.
    /// Preserve keyboard focus, including when the worker needs another tab.
    /// Refresh existing Claude hooks with `yinz hooks install --agent claude` to prevent duplicate reports.
    Spawn {
        /// Initial destination: stable numeric tab ID, including zero, not tab-bar position.
        /// Does not assign a project or change focus. Requires a matching running server.
        #[clap(long, conflicts_with = "new_tab", value_name = "ID")]
        tab: Option<usize>,
        /// Create an unfocused tab, optionally named. Later moves or arrangement may relocate the pane.
        /// --project selects the checkout; tab assignments still take precedence in project listings.
        #[clap(long, num_args = 0..=1, value_name = "NAME")]
        new_tab: Option<Option<String>>,
        #[clap(long, help = format!("Agent kind (native: {}; loaded Scheme plugins add more). Overrides the JSON kind", yinz_types::agent::KINDS.join(", ")))]
        kind: Option<String>,
        /// Read a JSON launch object from a file, or '-' for stdin. Flags override JSON fields.
        /// JSON also accepts command, args, env and hooks; opening prompts are not allowed.
        #[clap(long)]
        file: Option<PathBuf>,
        /// Select a registered project's checkout; --cwd overrides its directory.
        #[clap(long)]
        project: Option<String>,
        /// Create and remember an empty Git repository on main; requires --destination.
        /// Rejects project, cwd, SSH, branch, base, worktree, reuse and resume options.
        #[clap(long, value_name = "NAME", conflicts_with_all = ["project", "cwd", "remote_host", "branch", "base", "worktree", "worktree_dir", "reuse", "resume"])]
        new_project: Option<String>,
        /// Exact new repository root; its parent must exist and the destination must be absent.
        /// Requires --new-project. Failed launches retain the repository and registration.
        #[clap(long, value_name = "PATH")]
        destination: Option<PathBuf>,
        /// Run over SSH in an existing absolute --cwd; requires --allow-outside-worktree.
        #[clap(long)]
        remote_host: Option<String>,
        /// Put --branch in a separate worktree (default: true); false switches the checkout.
        #[clap(long)]
        worktree: Option<bool>,
        /// Working directory; defaults to the project checkout or current local directory.
        #[clap(long)]
        cwd: Option<PathBuf>,
        /// Create this branch, in a worktree by default; collisions get a fresh branch name.
        #[clap(long)]
        branch: Option<String>,
        #[clap(long, help = format!("Cut --branch from this ref (default: {})", yinz_runtime::agent::launch::DEFAULT_BASE))]
        base: Option<String>,
        #[clap(long, help = format!("Parent directory for worktrees, relative to the primary checkout (default: {})", yinz_runtime::agent::launch::WORKTREE_PREFIX))]
        worktree_dir: Option<String>,
        /// Provider model ID or an alias supported by the selected agent.
        #[clap(long)]
        model: Option<String>,
        /// Reasoning effort supported by the selected agent and model.
        #[clap(long)]
        effort: Option<String>,
        /// Agent-specific permission mode; 'skip' requests its supported bypass mode.
        /// Managed Codex resumes cannot use this option. Choose a mode for a new conversation.
        #[clap(long)]
        permission_mode: Option<String>,
        /// Resume a conversation by ID, or 'last' for the most recent.
        /// For managed Codex launches, omit --permission-mode when resuming.
        #[clap(long)]
        resume: Option<String>,
        /// Reuse an existing branch or worktree when its name is taken.
        #[clap(long)]
        reuse: bool,
        /// Permit launching outside a linked Git worktree.
        #[clap(long)]
        allow_outside_worktree: bool,
        /// Use the project environment accepted by `yinz bubblewrap prepare`.
        #[clap(long)]
        bubblewrap: bool,
        /// Let a sandboxed Codex run `yinz` commands. Codex's sandbox blocks Unix socket
        /// connections unless it has network access, so this also allows the internet.
        #[clap(long)]
        allow_yinz: bool,
        /// Print the launch outcome, actual pane, directory and branch as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Submit to an idle agent, replacing its draft, and wait for acknowledgement or the resulting turn.
    /// Clears the composer before every prompt, including unrecognized layouts. Does not interrupt work or answer dialogs.
    /// Claude delivery removes U+200B and U+202E; other characters and Codex text are preserved.
    Prompt {
        /// Terminal pane ID, such as terminal_7 or 7.
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        /// The prompt. Pass either TEXT or --file, not both.
        text: Option<String>,
        /// Read the prompt from a UTF-8 file, or from standard input with '-'.
        #[clap(long)]
        file: Option<PathBuf>,
        /// Require this provider conversation to still occupy the pane.
        #[clap(long)]
        conversation: Option<String>,
        /// Identify this submission so its exact result can be retrieved with `wait --submission`.
        /// Defaults to a generated UUID. Save an explicit ID before sending if the caller may
        /// lose the response. After uncertainty, wait for this ID instead of resubmitting.
        #[clap(long, value_parser = parse_submission_id)]
        submission: Option<String>,
        /// Ignored compatibility option: explicit prompts always replace drafts.
        #[clap(long, hide = true)]
        preserve_draft: bool,
        /// Wait for this turn's outcome after the agent acknowledges the prompt. If Claude left
        /// background work running, the reply lists `background_tasks`; `agent wait --until idle`
        /// returns its final answer.
        #[clap(long)]
        wait: bool,
        /// Total deadline, including acknowledgement (default: 30s, or 10m with --wait).
        #[clap(long, value_parser = parse_agent_timeout)]
        timeout: Option<std::time::Duration>,
        #[clap(long)]
        json: bool,
    },
    /// Wait for a submission's exact outcome, or observe a reported state without sending input.
    /// A successful idle state wait returns the final
    /// text of the last completed turn. For Claude, it also waits for background work (listed
    /// as `background_tasks`) to finish its notification turn.
    Wait {
        /// Terminal pane ID, such as terminal_7 or 7.
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        /// Require this provider conversation to still occupy the pane.
        #[clap(long)]
        conversation: Option<String>,
        /// Retrieve the original submission's outcome, including failure, after a timeout or
        /// needs_input. Uses the ID returned by prompt; results are retained in this live session.
        #[clap(long, conflicts_with = "until", value_parser = parse_submission_id)]
        submission: Option<String>,
        /// Reported state to wait for: busy, waiting, idle, or exited.
        #[clap(long, default_value = "idle", value_parser = parse_agent_state)]
        until: yinz_types::agent::AgentState,
        /// Observation deadline; a timeout does not cancel the agent's work.
        #[clap(long, default_value = "30s", value_parser = parse_agent_timeout)]
        timeout: std::time::Duration,
        #[clap(long)]
        json: bool,
    },
    /// Read the agent's terminal output. Plain text includes ghost suggestions and cannot
    /// establish whether the composer contains a draft. Do not infer a draft from suggestion text.
    Read {
        /// Terminal pane ID, such as terminal_7 or 7.
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        /// Require this provider conversation to still occupy the pane.
        #[clap(long)]
        conversation: Option<String>,
        /// Maximum terminal lines (1–100000), including internal blank rows. Remove blank
        /// padding at both ends. Trailing blank rows do not count toward the limit.
        #[clap(long, default_value_t = 80, value_parser = clap::value_parser!(u32).range(1..=100_000))]
        lines: u32,
        #[clap(long)]
        json: bool,
    },
    /// Discard the inspected composer draft and restore readiness only after verification.
    ClearDraft {
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        #[clap(long)]
        conversation: String,
        /// Exact input revision from the inspected status. Never refresh and retry automatically.
        #[clap(long)]
        input_revision: u64,
        /// Explicitly authorize discarding the inspected draft.
        #[clap(long, required = true)]
        confirm_discard: bool,
        /// Confirm work stopped despite a stale busy or waiting report.
        #[clap(long)]
        interrupted: bool,
        #[clap(long, default_value = "30s", value_parser = parse_agent_timeout)]
        timeout: std::time::Duration,
        #[clap(long)]
        json: bool,
    },
    /// Restore readiness after inspecting the pane and clearing any draft. Plain terminal text
    /// includes ghost suggestions and cannot establish whether the composer contains a draft.
    Recover {
        /// Terminal pane ID, such as terminal_7 or 7.
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        /// Pin the expected conversation; pass an empty value when the pane reports
        /// no session, as a fresh Muse pane does before its first turn binds one.
        #[clap(long)]
        conversation: String,
        /// Confirm deliberately that the composer is empty. Do not infer this from `agent read`
        /// or treat suggestion text as a draft. Clear actual input through the terminal UI first.
        #[clap(long, required = true)]
        confirm_empty: bool,
        /// Also reset a busy or waiting agent to idle. Verify in the terminal UI that the agent
        /// stopped its turn without reporting it, such as after an Esc interrupt.
        #[clap(long)]
        interrupted: bool,
        #[clap(long)]
        json: bool,
    },
    /// Deliberately send keys to an agent's terminal UI.
    SendKeys {
        /// Terminal pane ID, such as terminal_7 or 7.
        #[clap(value_parser = parse_agent_target)]
        target: yinz_types::pane_id::PaneId,
        /// One key per argument. Quote modifiers, such as "Ctrl u". Named keys include Esc,
        /// Enter, and Down (example: Down Enter). Unsupported aliases: Ctrl+u, ctrl-u, C-u,
        /// Escape, Return.
        #[clap(required = true, num_args = 1..=1024)]
        keys: Vec<String>,
        /// Require this provider conversation to still occupy the pane.
        #[clap(long)]
        conversation: Option<String>,
        #[clap(long)]
        json: bool,
    },
}

fn parse_configuration_json(value: &str) -> Result<PluginUserConfiguration, serde_json::Error> {
    serde_json::from_str(value)
}

#[cfg(test)]
#[path = "unit/agent_cli_tests.rs"]
mod agent_cli_tests;

#[derive(Debug, Parser, Clone, Serialize, Deserialize)]
pub struct SubscribeCli {
    /// Pane ID(s) to subscribe to (e.g. terminal_1, plugin_2, or bare number like 1)
    #[clap(
        short,
        long,
        required = true,
        num_args(1..)
    )]
    pub pane_id: Vec<String>,

    /// Include scrollback lines in initial delivery.
    /// Bare --scrollback = all scrollback, --scrollback N = last N lines.
    #[clap(
        short,
        long,
        default_missing_value = "0",
        num_args(0..=1)
    )]
    pub scrollback: Option<usize>,

    /// Output format
    #[clap(short, long, default_value = "raw", value_enum)]
    pub format: SubscribeFormat,

    /// Preserve ANSI styling in the output
    #[clap(long)]
    pub ansi: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum)]
pub enum SubscribeFormat {
    Raw,
    Json,
}

#[derive(Debug, Parser, Clone, Serialize, Deserialize)]
pub struct EventsCli {
    #[clap(flatten)]
    #[serde(default)]
    pub filters: AgentWatchFilters,
    #[clap(skip)]
    #[serde(default)]
    pub text: bool,
    /// Event type(s) to stream, as PaneUpdate or pane_update. Every streamable event when
    /// none is given; an unknown or unstreamable name lists what is.
    #[clap(short = 't', long = "type", visible_alias = "event-type", num_args(1..))]
    pub event_type: Vec<String>,
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum SessionCommand {
    /// Change the behaviour of yinz
    #[clap(name = "options")]
    Options(CliOptions),
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum Sessions {
    /// List active sessions
    #[clap(visible_alias = "ls")]
    ListSessions {
        /// Do not add colors and formatting to the list (useful for parsing)
        #[clap(short, long)]
        no_formatting: bool,

        /// Print just the session name
        #[clap(short, long)]
        short: bool,

        /// Include exited sessions that can be resurrected
        #[clap(long)]
        exited: bool,

        /// List the sessions in reverse order (default is ascending order)
        #[clap(short, long)]
        reverse: bool,
    },
    /// List existing plugin aliases
    #[clap(visible_alias = "la")]
    ListAliases,
    /// Attach to a session
    #[clap(visible_alias = "a")]
    Attach {
        /// Name of the session to attach to. Without one, attaches to the only running session,
        /// or on a terminal opens a picker when there are several (switching, from inside a
        /// session).
        #[clap(value_parser)]
        session_name: Option<String>,

        /// Create a session if one does not exist.
        #[clap(short, long, value_parser)]
        create: bool,

        /// Create a detached session in the background if one does not exist
        #[clap(short('b'), long, value_parser)]
        create_background: bool,

        /// Number of the session index in the active sessions ordered creation date.
        #[clap(long, value_parser)]
        index: Option<usize>,

        /// Change the behaviour of yinz
        #[clap(subcommand, name = "options")]
        options: Option<Box<SessionCommand>>,

        /// If resurrecting a dead session, immediately run all its commands on startup
        #[clap(short, long)]
        force_run_commands: bool,
    },

    /// Watch a session (read-only)
    #[clap(visible_alias = "w")]
    Watch {
        /// Name of the session to watch
        #[clap(value_parser)]
        session_name: Option<String>,
    },

    /// Kill a specific session
    ///
    /// Also terminates any detached pane supervisors an earlier crash of that session
    /// left behind (see `yinz pty-host list`).
    #[clap(visible_alias = "k")]
    KillSession {
        /// Name of target session
        #[clap(value_parser)]
        target_session: Option<String>,
    },

    /// Delete a specific session
    #[clap(visible_alias = "d")]
    DeleteSession {
        /// Name of target session
        #[clap(value_parser)]
        target_session: Option<String>,
        /// Kill the session if it's running before deleting it
        #[clap(short, long)]
        force: bool,
    },

    /// Kill all sessions
    #[clap(visible_alias = "ka")]
    KillAllSessions {
        /// Automatic yes to prompts
        #[clap(short, long, value_parser)]
        yes: bool,
    },

    /// Delete all sessions
    #[clap(visible_alias = "da")]
    DeleteAllSessions {
        /// Automatic yes to prompts
        #[clap(short, long, value_parser)]
        yes: bool,
        /// Kill the sessions if they're running before deleting them
        #[clap(short, long)]
        force: bool,
    },

    /// Run a command in a new pane
    /// Returns: Created pane ID (format: terminal_<id>)
    #[clap(visible_alias = "r")]
    Run {
        /// Command to run
        #[clap(last(true), required(true))]
        command: Vec<String>,

        /// Direction to open the new pane in
        #[clap(short, long, value_parser, conflicts_with("floating"))]
        direction: Option<Direction>,

        /// Change the working directory of the new pane
        #[clap(long, value_parser)]
        cwd: Option<PathBuf>,

        /// Open the new pane in floating mode
        #[clap(short, long)]
        floating: bool,

        /// Open the new pane in place of the current pane, temporarily suspending it
        #[clap(short, long, conflicts_with("floating"), conflicts_with("direction"))]
        in_place: bool,

        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,

        /// Name of the new pane
        #[clap(short, long, value_parser)]
        name: Option<String>,

        /// Close the pane immediately when its command exits
        #[clap(short, long)]
        close_on_exit: bool,

        /// Start the command suspended, only running after you first presses ENTER
        #[clap(short, long)]
        start_suspended: bool,

        /// The x coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        x: Option<String>,
        /// The y coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        y: Option<String>,
        /// The width if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        width: Option<String>,
        /// The height if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        height: Option<String>,
        /// Whether to pin a floating pane so that it is always on top
        #[clap(long, requires("floating"))]
        pinned: Option<bool>,
        /// Position this pane against the whole session surface and draw it above the tab's panes
        #[clap(long, requires("floating"))]
        overlay: Option<bool>,
        /// As --overlay, but the pane keeps focus until it closes itself. For a pane that has no
        /// way to learn it was defocused, and would be stranded on screen if focus moved.
        #[clap(long, requires("floating"))]
        modal: Option<bool>,
        #[clap(long, conflicts_with("floating"), conflicts_with("direction"))]
        stacked: bool,
        /// Block until the command has finished and its pane has been closed
        #[clap(long)]
        blocking: bool,

        /// Block until the command exits successfully (exit status 0) OR its pane has been closed
        #[clap(
            long,
            conflicts_with("blocking"),
            conflicts_with("block_until_exit_failure"),
            conflicts_with("block_until_exit")
        )]
        block_until_exit_success: bool,

        /// Block until the command exits with failure (non-zero exit status) OR its pane has been
        /// closed
        #[clap(
            long,
            conflicts_with("blocking"),
            conflicts_with("block_until_exit_success"),
            conflicts_with("block_until_exit")
        )]
        block_until_exit_failure: bool,

        /// Block until the command exits (regardless of exit status) OR its pane has been closed
        #[clap(
            long,
            conflicts_with("blocking"),
            conflicts_with("block_until_exit_success"),
            conflicts_with("block_until_exit_failure")
        )]
        block_until_exit: bool,
        /// if set, will open the pane near the current one rather than following the user's focus
        #[clap(long)]
        near_current_pane: bool,
        #[clap(
            long,
            help = "if set, will open the pane without changing the focus of any client, placing it relative to the pane the command was issued from"
        )]
        no_focus: bool,
        /// start this pane without a border (warning: will make it impossible to move with the
        /// mouse)
        #[clap(short, long, value_parser)]
        borderless: Option<bool>,
        /// Target a specific tab by ID
        #[clap(
            long,
            value_parser,
            conflicts_with("near_current_pane"),
            conflicts_with("in_place")
        )]
        tab_id: Option<TabId>,
    },
    /// Load a plugin
    /// Returns: Created pane ID (format: plugin_<id>)
    #[clap(visible_alias = "p")]
    Plugin {
        /// Plugin URL, can either start with file: or yinz:
        #[clap(last(true), required(true))]
        url: String,

        /// Plugin configuration
        #[clap(short, long, value_parser)]
        configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,

        /// Open the new pane in floating mode
        #[clap(short, long)]
        floating: bool,

        /// Open the new pane in place of the current pane, temporarily suspending it
        #[clap(short, long, conflicts_with("floating"))]
        in_place: bool,

        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,

        /// Skip the memory and HD cache and force recompile of the plugin (good for development)
        #[clap(short, long)]
        skip_plugin_cache: bool,
        /// The x coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        x: Option<String>,
        /// The y coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        y: Option<String>,
        /// The width if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        width: Option<String>,
        /// The height if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        height: Option<String>,
        /// Whether to pin a floating pane so that it is always on top
        #[clap(long, requires("floating"))]
        pinned: Option<bool>,
        /// Position this pane against the whole session surface and draw it above the tab's panes
        #[clap(long, requires("floating"))]
        overlay: Option<bool>,
        /// As --overlay, but the pane keeps focus until it closes itself. For a pane that has no
        /// way to learn it was defocused, and would be stranded on screen if focus moved.
        #[clap(long, requires("floating"))]
        modal: Option<bool>,
        #[clap(
            long,
            help = "if set, will open the plugin pane without changing the focus of any client, placing it relative to the pane the command was issued from"
        )]
        no_focus: bool,
        /// start this pane without a border (warning: will make it impossible to move with the
        /// mouse)
        #[clap(short, long, value_parser)]
        borderless: Option<bool>,
        /// Target a specific tab by ID
        #[clap(long, value_parser, conflicts_with("in_place"))]
        tab_id: Option<TabId>,
    },
    /// Edit file with default $EDITOR / $VISUAL
    /// Returns: Created pane ID (format: terminal_<id>)
    #[clap(visible_alias = "e")]
    Edit {
        file: PathBuf,

        /// Open the file in the specified line number
        #[clap(short, long, value_parser)]
        line_number: Option<usize>,

        /// Direction to open the new pane in
        #[clap(short, long, value_parser, conflicts_with("floating"))]
        direction: Option<Direction>,

        /// Open the new pane in place of the current pane, temporarily suspending it
        #[clap(short, long, conflicts_with("floating"), conflicts_with("direction"))]
        in_place: bool,

        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,

        /// Open the new pane in floating mode
        #[clap(short, long)]
        floating: bool,

        /// Change the working directory of the editor
        #[clap(long, value_parser)]
        cwd: Option<PathBuf>,
        /// The x coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        x: Option<String>,
        /// The y coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        y: Option<String>,
        /// The width if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        width: Option<String>,
        /// The height if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        height: Option<String>,
        /// Whether to pin a floating pane so that it is always on top
        #[clap(long, requires("floating"))]
        pinned: Option<bool>,
        /// Position this pane against the whole session surface and draw it above the tab's panes
        #[clap(long, requires("floating"))]
        overlay: Option<bool>,
        /// As --overlay, but the pane keeps focus until it closes itself. For a pane that has no
        /// way to learn it was defocused, and would be stranded on screen if focus moved.
        #[clap(long, requires("floating"))]
        modal: Option<bool>,
        /// if set, will open the pane near the current one rather than following the user's focus
        #[clap(long)]
        near_current_pane: bool,
        #[clap(
            long,
            help = "if set, will open the pane without changing the focus of any client, placing it relative to the pane the command was issued from"
        )]
        no_focus: bool,
        /// start this pane without a border (warning: will make it impossible to move with the
        /// mouse)
        #[clap(short, long, value_parser)]
        borderless: Option<bool>,
        /// Target a specific tab by ID
        #[clap(
            long,
            value_parser,
            conflicts_with("near_current_pane"),
            conflicts_with("in_place")
        )]
        tab_id: Option<TabId>,
    },
    /// Send data to one or more plugins, launch them if they are not running.
    #[clap(override_usage(
        r#"
yinz pipe [OPTIONS] [--] <PAYLOAD>

* Send data to a specific plugin:

yinz pipe --plugin file:/path/to/my/plugin.scm --name my_pipe_name -- my_arbitrary_data

* To all running plugins (that are listening):

yinz pipe --name my_pipe_name -- my_arbitrary_data

* Pipe data into this command's STDIN and get output from the plugin on this command's STDOUT

tail -f /tmp/my-live-logfile | yinz pipe --name logs --plugin file:/path/to/my-plugin.scm | wc -l
"#
    ))]
    Pipe {
        /// Focus a newly auto-launched plugin pane, revealing floating panes when applicable.
        /// Does not focus an existing or already-loading destination.
        #[clap(long, requires("plugin"))]
        #[serde(default)]
        focus_plugin: bool,
        /// The name of the pipe
        #[clap(short, long, value_parser, display_order(1))]
        name: Option<String>,
        /// The data to send down this pipe (if blank, will listen to STDIN)
        payload: Option<String>,

        #[clap(short, long, value_parser, display_order(2))]
        /// Comma-separated key=value arguments for the pipe
        args: Option<PluginUserConfiguration>,
        /// The plugin url (eg. file:/tmp/my-plugin.scm) to direct this pipe to, if not specified,
        /// will be sent to running plugins. If specified and not running, the plugin will be
        /// launched floating by default, without changing focus or revealing a hidden floating layer.
        #[clap(short, long, value_parser, display_order(3))]
        plugin: Option<String>,
        /// The plugin configuration (note: the same plugin with different configuration is
        /// considered a different plugin for the purposes of determining the pipe destination)
        #[clap(short('c'), long, value_parser, display_order(4))]
        plugin_configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "plugin_configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,
    },
}

fn parse_wait_pane(value: &str) -> Result<yinz_types::pane_id::PaneId, String> {
    value.parse().map_err(|error| format!("{error}"))
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum Wait {
    /// Wait for literal viewport content, process exit, or pane disappearance.
    PaneWait {
        #[clap(value_parser = parse_wait_pane)]
        pane_id: yinz_types::pane_id::PaneId,
        #[clap(long, conflicts_with_all = ["exited", "closed"], required_unless_present_any = ["exited", "closed"])]
        contains: Option<String>,
        #[clap(long, conflicts_with = "closed")]
        exited: bool,
        #[clap(long)]
        closed: bool,
        #[clap(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
}

#[derive(Debug, Subcommand, Clone, Serialize, Deserialize)]
pub enum CliAction {
    #[clap(flatten)]
    Wait(Wait),
    #[clap(flatten)]
    Operations(crate::operations::Operations),
    #[clap(flatten)]
    NotesTodos(crate::notes_todos::NotesTodos),
    /// Write bytes to the terminal.
    Write {
        bytes: Vec<u8>,
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Write characters to the terminal.
    WriteChars {
        chars: String,
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Paste text to the terminal (using bracketed paste mode).
    Paste {
        chars: String,
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Send one or more keys to the terminal (e.g., "Ctrl a", "F1", "Alt Shift b")
    SendKeys {
        /// Keys to send as space-separated strings
        #[clap(value_parser, required = true)]
        keys: Vec<String>,

        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// [increase|decrease] the focused panes area at the [left|down|up|right] border.
    Resize {
        resize: Resize,
        direction: Option<Direction>,
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Change focus to the next pane
    FocusNextPane,
    /// Change focus to the previous pane
    FocusPreviousPane,
    /// Focus a specific pane by its ID
    FocusPaneId {
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3
        pane_id: String,
    },
    /// Move a visible pane and its hidden contents to another session. Plugins restart.
    WarpPane {
        /// Name of the session to move the pane to
        #[clap(long, short)]
        to_session: String,
        /// Visible pane, eg. terminal_1, plugin_2 or 3. Moves its whole stack list if present.
        #[clap(long)]
        pane_id: Option<String>,
        /// Land the pane in the target session's focused tab instead of a new tab
        #[clap(long, conflicts_with = "floating")]
        focused_tab: bool,
        /// Land the pane floating in the target session's focused tab
        #[clap(long, conflicts_with = "focused_tab")]
        floating: bool,
    },
    /// Move a tab, with everything running in it, to another session
    WarpTab {
        /// Name of the session to move the tab to
        #[clap(long, short, conflicts_with = "from_session")]
        to_session: Option<String>,
        /// Name of the session to bring a tab from. Requires --tab-id.
        #[clap(long, short, conflicts_with = "to_session")]
        from_session: Option<String>,
        /// The stable ID of the tab to move. Defaults to the focused tab, and is required with
        /// --from-session.
        #[clap(long)]
        tab_id: Option<TabId>,
    },
    /// Change focus to the last focused frame
    FocusLastPane,
    /// Focus the native pane tab bar.
    FocusPaneTabs,
    /// Select the previous plugin in the current pane tabs.
    PreviousPaneTab,
    /// Select the next plugin in the current pane tabs.
    NextPaneTab,
    /// Move the focused pane in the specified direction. [right|left|up|down]
    MoveFocus {
        direction: Direction,
    },
    /// Move focus to the pane or tab (if on screen edge) in the specified direction
    /// [right|left|up|down]
    MoveFocusOrTab {
        direction: Direction,
    },
    /// Change the location of the focused pane in the specified direction or rotate forwrads
    /// [right|left|up|down]
    MovePane {
        direction: Option<Direction>,
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Rotate the location of the previous pane backwards
    MovePaneBackwards {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Clear all buffers for a focused pane
    Clear {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Dumps the viewport and optionally scrollback of a pane to a file or STDOUT
    DumpScreen {
        /// File path to dump the pane content to. If omitted, prints to STDOUT.
        #[clap(long, value_parser)]
        path: Option<PathBuf>,

        /// Dump the pane with full scrollback
        #[clap(short, long)]
        full: bool,

        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3). If not specified, dumps the focused pane.
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,

        /// Preserve ANSI styling in the dump output
        #[clap(short, long)]
        ansi: bool,
    },
    /// Dump current layout to stdout
    DumpLayout,
    /// Save the current session state to disk immediately
    SaveSession,
    /// Open the pane scrollback in your default editor
    EditScrollback {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,

        /// Preserve ANSI styling in the scrollback dump
        #[clap(short, long)]
        ansi: bool,
    },
    /// Scroll up in the focused pane
    ScrollUp {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll down in focus pane.
    ScrollDown {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll down to bottom in focus pane.
    ScrollToBottom {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll up to top in focus pane.
    ScrollToTop {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll up one page in focus pane.
    PageScrollUp {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll down one page in focus pane.
    PageScrollDown {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll up half page in focus pane.
    HalfPageScrollUp {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Scroll down half page in focus pane.
    HalfPageScrollDown {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Toggle between fullscreen focus pane and normal layout.
    ToggleFullscreen {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    #[clap(
        about = "Toggle between fullscreen over the entire display (including the UI bars) and normal layout"
    )]
    ToggleNoUiFullscreen {
        #[clap(
            short,
            long,
            value_parser,
            help = "Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)"
        )]
        pane_id: Option<String>,
    },
    /// Toggle frames around panes in the UI
    TogglePaneFrames,
    SetPaneFrameStyle {
        #[clap(value_parser = crate::options::pane_frame_style_parser())]
        style: PaneFrameStyle,
    },
    /// Toggle between sending text commands to all panes on the current tab and normal mode.
    ToggleActiveSyncTab {
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Open a new pane in the specified direction [right|down]
    /// If no direction is specified, will try to use the biggest available space.
    /// Returns: Created pane ID (format: terminal_<id> or plugin_<id>)
    NewPane {
        /// Direction to open the new pane in
        #[clap(short, long, value_parser, conflicts_with("floating"))]
        direction: Option<Direction>,

        #[clap(last(true))]
        command: Vec<String>,

        #[clap(short, long, conflicts_with("command"), conflicts_with("direction"))]
        plugin: Option<String>,

        /// Change the working directory of the new pane
        #[clap(long, value_parser)]
        cwd: Option<PathBuf>,

        /// Open the new pane in floating mode
        #[clap(short, long)]
        floating: bool,

        /// Open the new pane in place of the current pane, temporarily suspending it
        #[clap(short, long, conflicts_with("floating"), conflicts_with("direction"))]
        in_place: bool,

        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,

        /// The pane to replace when opening in place, eg. terminal_1, plugin_2 or 3 (only
        /// effective with --in-place; defaults to the focused pane)
        #[clap(
            long,
            value_parser,
            requires("in_place"),
            conflicts_with("near_current_pane")
        )]
        pane_id: Option<String>,

        /// Name of the new pane
        #[clap(short, long, value_parser)]
        name: Option<String>,

        /// Close the pane immediately when its command exits
        #[clap(short, long, requires("command"))]
        close_on_exit: bool,
        /// Start the command suspended, only running it after the you first press ENTER
        #[clap(short, long, requires("command"))]
        start_suspended: bool,
        /// Run the pane on another machine over ssh, given as an ssh destination
        /// (a host, an alias from your ssh config, or user@host). Needs a yinz
        /// of the same version on its PATH there. With no command, opens that
        /// machine's default login shell, like a plain ssh would.
        #[clap(long, value_parser, conflicts_with("plugin"))]
        remote_host: Option<String>,
        #[clap(long, value_parser)]
        configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,
        #[clap(long, value_parser)]
        skip_plugin_cache: bool,
        /// The x coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        x: Option<String>,
        /// The y coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        y: Option<String>,
        /// The width if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        width: Option<String>,
        /// The height if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        height: Option<String>,
        /// Whether to pin a floating pane so that it is always on top
        #[clap(long, requires("floating"))]
        pinned: Option<bool>,
        /// Position this pane against the whole session surface and draw it above the tab's panes
        #[clap(long, requires("floating"))]
        overlay: Option<bool>,
        /// As --overlay, but the pane keeps focus until it closes itself. For a pane that has no
        /// way to learn it was defocused, and would be stranded on screen if focus moved.
        #[clap(long, requires("floating"))]
        modal: Option<bool>,
        #[clap(long, conflicts_with("floating"), conflicts_with("direction"))]
        stacked: bool,
        /// Block until the command has finished and its pane has been closed
        #[clap(short, long)]
        blocking: bool,

        /// Block until the command exits successfully (exit status 0) OR its pane has been closed
        #[clap(
            long,
            conflicts_with("blocking"),
            conflicts_with("block_until_exit_failure"),
            conflicts_with("block_until_exit")
        )]
        block_until_exit_success: bool,

        /// Block until the command exits with failure (non-zero exit status) OR its pane has been
        /// closed
        #[clap(
            long,
            conflicts_with("blocking"),
            conflicts_with("block_until_exit_success"),
            conflicts_with("block_until_exit")
        )]
        block_until_exit_failure: bool,

        /// Block until the command exits (regardless of exit status) OR its pane has been closed
        #[clap(
            long,
            conflicts_with("blocking"),
            conflicts_with("block_until_exit_success"),
            conflicts_with("block_until_exit_failure")
        )]
        block_until_exit: bool,

        #[clap(skip)]
        unblock_condition: Option<UnblockCondition>,

        /// if set, will open the pane near the current one rather than following the user's focus
        #[clap(long)]
        near_current_pane: bool,
        #[clap(
            long,
            help = "if set, will open the pane without changing the focus of any client, placing it relative to the pane the command was issued from"
        )]
        no_focus: bool,
        /// start this pane without a border (warning: will make it impossible to move with the
        /// mouse)
        #[clap(long, value_parser)]
        borderless: Option<bool>,
        /// Target a specific tab by ID
        #[clap(
            long,
            value_parser,
            conflicts_with("near_current_pane"),
            conflicts_with("in_place")
        )]
        tab_id: Option<TabId>,
    },
    /// Open the specified file in a new yinz pane with your default EDITOR
    /// Returns: Created pane ID (format: terminal_<id>)
    Edit {
        file: PathBuf,

        /// Direction to open the new pane in
        #[clap(short, long, value_parser, conflicts_with("floating"))]
        direction: Option<Direction>,

        /// Open the file in the specified line number
        #[clap(short, long, value_parser)]
        line_number: Option<usize>,

        /// Open the new pane in floating mode
        #[clap(short, long)]
        floating: bool,

        /// Open the new pane in place of the current pane, temporarily suspending it
        #[clap(short, long, conflicts_with("floating"), conflicts_with("direction"))]
        in_place: bool,

        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,

        /// Change the working directory of the editor
        #[clap(long, value_parser)]
        cwd: Option<PathBuf>,
        /// The x coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        x: Option<String>,
        /// The y coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long, requires("floating"))]
        y: Option<String>,
        /// The width if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        width: Option<String>,
        /// The height if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long, requires("floating"))]
        height: Option<String>,
        /// Whether to pin a floating pane so that it is always on top
        #[clap(long, requires("floating"))]
        pinned: Option<bool>,
        /// Position this pane against the whole session surface and draw it above the tab's panes
        #[clap(long, requires("floating"))]
        overlay: Option<bool>,
        /// As --overlay, but the pane keeps focus until it closes itself. For a pane that has no
        /// way to learn it was defocused, and would be stranded on screen if focus moved.
        #[clap(long, requires("floating"))]
        modal: Option<bool>,
        /// if set, will open the pane near the current one rather than following the user's focus
        #[clap(long)]
        near_current_pane: bool,
        #[clap(
            long,
            help = "if set, will open the pane without changing the focus of any client, placing it relative to the pane the command was issued from"
        )]
        no_focus: bool,
        /// start this pane without a border (warning: will make it impossible to move with the
        /// mouse)
        #[clap(short, long, value_parser)]
        borderless: Option<bool>,
        /// Target a specific tab by ID
        #[clap(
            long,
            value_parser,
            conflicts_with("near_current_pane"),
            conflicts_with("in_place")
        )]
        tab_id: Option<TabId>,
    },
    /// Switch input mode of all connected clients [locked|pane|tab|resize|move|search|session]
    SwitchMode {
        input_mode: InputMode,
    },
    /// Embed focused pane if floating or float focused pane if embedded
    TogglePaneEmbedOrFloating {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Toggle the visibility of all floating panes in the current Tab, open one if none exist
    ToggleFloatingPanes {
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Show all floating panes in the specified tab (or active tab if tab_id is not provided).
    ///
    /// Returns exit code 0 if state was changed, 2 if already visible, 1 if tab not found.
    ShowFloatingPanes {
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Hide all floating panes in the specified tab (or active tab if tab_id is not provided).
    ///
    /// Returns exit code 0 if state was changed, 2 if already hidden, 1 if tab not found.
    HideFloatingPanes {
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Check if floating panes are visible in the specified tab (or active tab).
    ///
    /// Prints "true" to stdout and exits 0 if visible.
    /// Prints "false" to stdout and exits 1 if not visible.
    AreFloatingPanesVisible {
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Close the focused pane.
    ClosePane {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Renames the focused pane
    RenamePane {
        name: String,
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Remove a previously set pane name
    UndoRenamePane {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Create and focus a group with a fresh tab. Prints the new group ID.
    NewTabGroup {
        #[clap(short, long)]
        name: Option<String>,
    },
    /// List groups with their IDs, displayed names, active state, and tab IDs.
    ListTabGroups {
        #[clap(short, long)]
        json: bool,
    },
    /// Print the current group's name and ID, or its full record as JSON.
    CurrentTabGroupInfo {
        #[clap(short, long)]
        json: bool,
    },
    /// Focus the remembered tab in a group by stable ID.
    GoToTabGroupById {
        id: u32,
    },
    /// Focus a group by exact displayed name; duplicate names select the lowest ID.
    GoToTabGroupName {
        name: String,
        /// Create when missing and print the new group ID.
        #[clap(short, long)]
        create: bool,
    },
    /// Rename the current group or a group by ID. Project names can override this name.
    RenameTabGroup {
        name: String,
        #[clap(long)]
        group_id: Option<u32>,
    },
    /// Close every tab in the current group or a group by ID.
    CloseTabGroup {
        #[clap(long)]
        group_id: Option<u32>,
    },
    /// Rename a group by stable ID.
    RenameTabGroupById {
        id: u32,
        name: String,
    },
    /// Close every tab in a group by stable ID.
    CloseTabGroupById {
        id: u32,
    },
    /// Focus the next group, wrapping at the end.
    GoToNextTabGroup,
    /// Focus the previous group, wrapping at the start.
    GoToPreviousTabGroup,
    /// Focus a tab by its one-based index within the current group.
    GoToTabInGroup {
        index: TabNumber,
    },
    /// Move the focused pane to the next group and follow it.
    MovePaneToNextTabGroup,
    /// Move the focused pane to the previous group and follow it.
    MovePaneToPreviousTabGroup,
    /// Go to the next tab.
    GoToNextTab,
    /// Go to the previous tab.
    GoToPreviousTab,
    /// Close the current tab.
    CloseTab {
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Go to tab with index [index]
    GoToTab {
        index: TabNumber,
    },
    /// Go to tab with name [name]
    ///
    /// Returns: When --create is used and tab is created, outputs the tab ID as a single number
    GoToTabName {
        name: String,
        /// Create a tab if one does not exist.
        #[clap(short, long, value_parser)]
        create: bool,
    },
    /// Renames the focused pane
    RenameTab {
        name: String,
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Remove a previously set tab name
    UndoRenameTab {
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Go to tab with stable ID
    GoToTabById {
        id: u64,
    },
    /// Close tab with stable ID
    CloseTabById {
        id: u64,
    },
    /// Rename tab by stable ID
    RenameTabById {
        id: u64,
        name: String,
    },
    /// Create a new tab, optionally with a specified tab layout and name
    ///
    /// Returns: The created tab's ID as a single number on stdout
    NewTab {
        /// Layout to use for the new tab
        #[clap(short, long, value_parser, conflicts_with = "layout_string")]
        layout: Option<PathBuf>,

        /// Raw KDL layout string to use directly (instead of a layout file path)
        #[clap(long, value_parser, conflicts_with = "layout")]
        layout_string: Option<String>,

        /// Default folder to look for layouts
        #[clap(long, value_parser, requires("layout"))]
        layout_dir: Option<PathBuf>,

        /// Name of the new tab
        #[clap(short, long, value_parser)]
        name: Option<String>,

        /// Change the working directory of the new tab
        #[clap(short, long, value_parser)]
        cwd: Option<PathBuf>,

        /// Optional initial command to run in the new tab
        #[clap(value_parser, conflicts_with("initial_plugin"), last(true))]
        initial_command: Vec<String>,

        /// Initial plugin to load in the new tab
        #[clap(long, value_parser, conflicts_with("initial_command"))]
        initial_plugin: Option<String>,

        /// Close the pane immediately when its command exits
        #[clap(long, requires("initial_command"))]
        close_on_exit: bool,

        /// Start the command suspended, only running it after you first press ENTER
        #[clap(long, requires("initial_command"))]
        start_suspended: bool,

        /// Block until the command exits successfully (exit status 0) OR its pane has been closed
        #[clap(
            long,
            requires("initial_command"),
            conflicts_with("block_until_exit_failure"),
            conflicts_with("block_until_exit")
        )]
        block_until_exit_success: bool,

        /// Block until the command exits with failure (non-zero exit status) OR its pane has been closed
        #[clap(
            long,
            requires("initial_command"),
            conflicts_with("block_until_exit_success"),
            conflicts_with("block_until_exit")
        )]
        block_until_exit_failure: bool,

        /// Block until the command exits (regardless of exit status) OR its pane has been closed
        #[clap(
            long,
            requires("initial_command"),
            conflicts_with("block_until_exit_success"),
            conflicts_with("block_until_exit_failure")
        )]
        block_until_exit: bool,

        #[clap(
            long,
            help = "if set, will create the tab without changing the focus of any client"
        )]
        no_focus: bool,
    },
    /// Move the focused tab in the specified direction. [right|left]
    MoveTab {
        direction: Direction,
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    PreviousSwapLayout {
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    NextSwapLayout {
        /// Target a specific tab by ID
        #[clap(short, long, value_parser)]
        tab_id: Option<TabId>,
    },
    /// Override the layout of the active tab
    OverrideLayout {
        /// Path to the layout file
        #[clap(
            value_parser,
            required_unless_present = "layout_string",
            conflicts_with = "layout_string"
        )]
        layout: Option<PathBuf>,

        /// Raw KDL layout string to use directly (instead of a layout file path)
        #[clap(long, value_parser, conflicts_with = "layout")]
        layout_string: Option<String>,

        /// Default folder to look for layouts
        #[clap(long, value_parser)]
        layout_dir: Option<PathBuf>,

        /// Retain existing terminal panes that do not fit in the layout (default: false)
        #[clap(long)]
        retain_existing_terminal_panes: bool,

        /// Retain existing plugin panes that do not fit with the layout default: false)
        #[clap(long)]
        retain_existing_plugin_panes: bool,

        /// Only apply the layout to the active tab (uses just the first layout tab if it has
        /// multiple)
        #[clap(long)]
        apply_only_to_active_tab: bool,
    },
    /// Query all tab names
    QueryTabNames,
    StartOrReloadPlugin {
        url: String,
        #[clap(short, long, value_parser)]
        configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,
    },
    /// Returns: Plugin pane ID (format: plugin_<id>) when creating or focusing plugin
    LaunchOrFocusPlugin {
        #[clap(short, long, value_parser)]
        floating: bool,
        #[clap(short, long, value_parser)]
        in_place: bool,
        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,
        #[clap(short, long, value_parser)]
        move_to_focused_tab: bool,
        url: String,
        #[clap(short, long, value_parser)]
        configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,
        #[clap(short, long, value_parser)]
        skip_plugin_cache: bool,
        /// Target a specific tab by ID
        #[clap(long, value_parser, conflicts_with("in_place"))]
        tab_id: Option<TabId>,
    },
    /// Returns: Plugin pane ID (format: plugin_<id>)
    LaunchPlugin {
        #[clap(short, long, value_parser)]
        floating: bool,
        #[clap(short, long, value_parser)]
        in_place: bool,
        /// Close the replaced pane instead of suspending it (only effective with --in-place)
        #[clap(long, requires("in_place"))]
        close_replaced_pane: bool,
        url: Url,
        #[clap(short, long, value_parser)]
        configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,
        #[clap(short, long, value_parser)]
        skip_plugin_cache: bool,
        #[clap(
            long,
            help = "if set, will open the plugin pane without changing the focus of any client"
        )]
        no_focus: bool,
        /// Target a specific tab by ID
        #[clap(long, value_parser, conflicts_with("in_place"))]
        tab_id: Option<TabId>,
    },
    RenameSession {
        name: String,
    },
    /// Apply a KDL config fragment to the running session: options, keybinds, hooks, plugins,
    /// anything config.kdl takes. Give it as --kdl, --file, or on stdin. A fragment that does not
    /// parse is refused here. With --write it is also persisted.
    Reconfigure {
        /// The fragment itself, eg. 'keybinds { normal { bind "Ctrl y" { NewPane; } } }'
        #[clap(long, value_parser, conflicts_with = "file")]
        kdl: Option<String>,
        /// A file holding the fragment
        #[clap(long, value_parser)]
        file: Option<PathBuf>,
        /// Persist the change as well as applying it
        #[clap(long)]
        write: bool,
    },
    /// Bind a hook point (agent_exit, agent_start, agent_waiting, pane_exit, session_start,
    /// context_menu) to a plugin, or unbind it with an empty url.
    /// The plugin is a url as the config spells it: file:my-hook.scm, yinz:agent-exit.
    SetHook {
        /// The hook point, eg. agent_exit
        #[clap(value_parser)]
        point: String,
        /// The plugin url; empty, or left out, unbinds the point
        #[clap(value_parser, default_value = "")]
        url: String,
        /// Persist the binding as well as applying it
        #[clap(long)]
        write: bool,
    },
    /// Show a notification that dismisses itself after a timeout. Its corner and timeout come from
    /// the `notification_position` and `notification_timeout` config options.
    Notify {
        /// The message to show
        text: String,
    },
    /// Send data to one or more plugins, launch them if they are not running.
    #[clap(override_usage(
r#"
yinz action pipe [OPTIONS] [--] <PAYLOAD>

* Send data to a specific plugin:

yinz action pipe --plugin file:/path/to/my/plugin.scm --name my_pipe_name -- my_arbitrary_data

* To all running plugins (that are listening):

yinz action pipe --name my_pipe_name -- my_arbitrary_data

* Pipe data into this command's STDIN and get output from the plugin on this command's STDOUT

tail -f /tmp/my-live-logfile | yinz action pipe --name logs --plugin file:/path/to/my-plugin.scm | wc -l
"#))]
    Pipe {
        /// Focus a newly auto-launched plugin pane, revealing floating panes when applicable.
        /// Does not focus an existing or already-loading destination.
        #[clap(long, requires("plugin"))]
        #[serde(default)]
        focus_plugin: bool,
        /// The name of the pipe
        #[clap(short, long, value_parser, display_order(1))]
        name: Option<String>,
        /// The data to send down this pipe (if blank, will listen to STDIN)
        payload: Option<String>,

        #[clap(short, long, value_parser, display_order(2))]
        /// Comma-separated key=value arguments for the pipe
        args: Option<PluginUserConfiguration>,
        /// The plugin url (eg. file:/tmp/my-plugin.scm) to direct this pipe to, if not specified,
        /// will be sent to running plugins. If specified and not running, the plugin will be
        /// launched floating by default, without changing focus or revealing a hidden floating layer.
        #[clap(short, long, value_parser, display_order(3))]
        plugin: Option<String>,
        /// The plugin configuration (note: the same plugin with different configuration is
        /// considered a different plugin for the purposes of determining the pipe destination)
        #[clap(short('c'), long, value_parser, display_order(4))]
        plugin_configuration: Option<PluginUserConfiguration>,
        /// Plugin configuration as a JSON object of strings. Mutually exclusive with
        /// legacy configuration. Percent sequences are literal; only JSON escapes are decoded.
        /// If repeated, the last JSON object wins.
        #[clap(long, value_name = "JSON", value_parser = parse_configuration_json, conflicts_with = "plugin_configuration")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_json: Option<PluginUserConfiguration>,
        /// Launch a new plugin even if one is already running
        #[clap(short('l'), long, display_order(5))]
        force_launch_plugin: bool,
        /// If launching a new plugin, skip cache and force-compile the plugin
        #[clap(short('s'), long, display_order(6))]
        skip_plugin_cache: bool,
        /// If launching a plugin, should it be floating or not, defaults to floating
        #[clap(short('f'), long, value_parser, display_order(7))]
        floating_plugin: Option<bool>,
        /// If launching a plugin, launch it in-place (on top of the current pane)
        #[clap(
            short('i'),
            long,
            value_parser,
            conflicts_with("floating_plugin"),
            display_order(8)
        )]
        in_place_plugin: Option<bool>,
        /// If launching a plugin, specify its working directory
        #[clap(short('w'), long, value_parser, display_order(9))]
        plugin_cwd: Option<PathBuf>,
        /// If launching a plugin, specify its pane title
        #[clap(short('t'), long, value_parser, display_order(10))]
        plugin_title: Option<String>,
    },
    ListClients,
    /// List all panes in the current session
    ///
    /// Returns: Formatted list of panes (table or JSON) to stdout
    ListPanes {
        /// Include tab information (name, position, ID)
        #[clap(short, long, value_parser)]
        tab: bool,

        /// Include running command information
        #[clap(short, long, value_parser)]
        command: bool,

        /// Include pane state (focused, floating, exited, etc.)
        #[clap(short, long, value_parser)]
        state: bool,

        /// Include geometry (position, size)
        #[clap(short, long, value_parser)]
        geometry: bool,

        /// Include all available fields
        #[clap(short, long, value_parser)]
        all: bool,

        /// Output as JSON
        #[clap(short, long, value_parser)]
        json: bool,
    },
    /// List all tabs with their information
    ///
    /// Returns: Tab information in table or JSON format
    ListTabs {
        /// Include state information (active, fullscreen, sync, floating visibility)
        #[clap(short, long, value_parser)]
        state: bool,

        /// Include dimension information (viewport, display area)
        #[clap(short, long, value_parser)]
        dimensions: bool,

        /// Include pane counts
        #[clap(short, long, value_parser)]
        panes: bool,

        /// Include layout information (swap layout name and dirty state)
        #[clap(short, long, value_parser)]
        layout: bool,

        /// Include all available fields
        #[clap(short, long, value_parser)]
        all: bool,

        /// Output as JSON
        #[clap(short, long, value_parser)]
        json: bool,
    },
    /// Get information about the currently active tab
    ///
    /// Returns: Tab name and ID by default, or full info in JSON
    CurrentTabInfo {
        /// Output as JSON with full TabInfo
        #[clap(short, long, value_parser)]
        json: bool,
    },
    /// Group terminal panes into project tabs, with branches and worktrees together.
    ArrangePanesByProject,
    TogglePanePinned {
        /// Target a specific pane by ID (eg. terminal_1, plugin_2, or 3)
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
    },
    /// Stack pane ids
    /// Ids are a space separated list of pane ids.
    /// They should either be in the form of `terminal_<int>` (eg. terminal_1), `plugin_<int>` (eg.
    /// plugin_1) or bare integers in which case they'll be considered terminals (eg. 1 is
    /// the equivalent of terminal_1)
    ///
    /// Example: yinz action stack-panes -- terminal_1 plugin_2 3
    StackPanes {
        #[clap(last(true), required(true))]
        pane_ids: Vec<String>,
    },
    ChangeFloatingPaneCoordinates {
        /// The pane_id of the floating pane, eg.  terminal_1, plugin_2 or 3 (equivalent to
        /// terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: String,
        /// The x coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long)]
        x: Option<String>,
        /// The y coordinates if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(short, long)]
        y: Option<String>,
        /// The width if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long)]
        width: Option<String>,
        /// The height if the pane is floating as a bare integer (eg. 1) or percent (eg. 10%)
        #[clap(long)]
        height: Option<String>,
        /// Whether to pin a floating pane so that it is always on top
        #[clap(long)]
        pinned: Option<bool>,
        /// change this pane to be with/without a border (warning: will make it impossible to move with the
        /// mouse if without a border)
        #[clap(short, long, value_parser)]
        borderless: Option<bool>,
    },
    TogglePaneBorderless {
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: String,
    },
    SetPaneBorderless {
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3)
        #[clap(short, long, value_parser)]
        pane_id: String,
        /// Whether the pane should be borderless (flag present) or bordered (flag absent)
        #[clap(short, long, value_parser)]
        borderless: bool,
    },
    /// Detach from the current session
    Detach,
    /// Switch the theme to dark (uses configured `theme_dark`).
    SetDarkTheme,
    /// Switch the theme to light (uses configured `theme_light`).
    SetLightTheme,
    /// Toggle between dark and light themes (used configured `theme_dark` and `theme_light`)
    ToggleTheme,
    /// Switch to a different session
    SwitchSession {
        /// Name of the session to switch to
        name: String,
        /// Optional tab position to focus
        #[clap(long)]
        tab_position: Option<TabNumber>,
        /// Optional pane ID to focus (eg. "terminal_1" for terminal pane with id 1, or "plugin_2" for plugin pane with id 2)
        #[clap(long)]
        pane_id: Option<String>,
        /// Layout to apply when switching to the session (relative paths start at layout-dir)
        #[clap(short, long, value_parser, conflicts_with = "layout_string")]
        layout: Option<PathBuf>,
        /// Raw KDL layout string to use directly
        #[clap(long, value_parser, conflicts_with = "layout")]
        layout_string: Option<String>,
        /// Default folder to look for layouts
        #[clap(long, value_parser, requires("layout"))]
        layout_dir: Option<PathBuf>,
        /// Change the working directory when switching
        #[clap(short, long, value_parser)]
        cwd: Option<PathBuf>,
    },
    /// Report what the agent running in a pane is doing. The session carries it on the pane
    /// (PaneInfo.agent), so `yinz events`, plugins and the agents list all see it. Coding-agent
    /// hooks call this with --hook-json and the hook's JSON on stdin.
    AgentReport {
        /// The pane the agent runs in, eg. terminal_1 or 3. Defaults to the current pane resolved through $YINZ_PANE_REF.
        #[clap(long, value_parser)]
        pane_id: Option<String>,
        /// What the agent is: claude, codex, ... Defaults to claude.
        #[clap(long, value_parser)]
        kind: Option<String>,
        /// busy, waiting, idle or exited. Required unless --hook-json or --clear is given.
        #[clap(long, value_parser, conflicts_with_all = ["hook_json", "clear"])]
        state: Option<String>,
        /// A few words on what it is doing, shown beside the state.
        #[clap(long, value_parser)]
        detail: Option<String>,
        /// The folder the agent is working in, when it is not the pane's.
        #[clap(long, value_parser)]
        cwd: Option<PathBuf>,
        /// Read a Claude Code, Codex or Muse hook event (JSON) from stdin and report what it means. An event
        /// that says nothing about the agent's state reports nothing.
        #[clap(long, conflicts_with = "clear")]
        hook_json: bool,
        /// The pane no longer runs an agent.
        #[clap(long)]
        clear: bool,
        /// The id of the run record you are keeping for this agent, so a warp to another
        /// session can find it again. Agents that report a session id of their own need
        /// nothing here; a script keeping its own history does. Omitting it leaves whatever
        /// the pane already carries alone.
        #[clap(long, value_parser, conflicts_with = "clear")]
        run_id: Option<String>,
    },
    /// Set the default foreground/background color of a pane
    SetPaneColor {
        /// The pane_id of the pane, eg. terminal_1, plugin_2 or 3 (equivalent to terminal_3).
        /// Defaults to the current pane resolved through $YINZ_PANE_REF.
        #[clap(short, long, value_parser)]
        pane_id: Option<String>,
        /// Foreground color (e.g. "#00e000", "rgb:00/e0/00")
        #[clap(long, value_parser)]
        fg: Option<String>,
        /// Background color (e.g. "#001a3a", "rgb:00/1a/3a")
        #[clap(long, value_parser)]
        bg: Option<String>,
        /// Reset pane colors to terminal defaults
        #[clap(long, value_parser, conflicts_with_all(&["fg", "bg"]))]
        reset: bool,
    },
}

#[cfg(test)]
mod tests {
    #[test]
    fn warp_tab_uses_tab_id() {
        use yinz_types::input::actions::Action;

        for (direction, expected) in [
            (
                "--to-session",
                Action::WarpTab {
                    tab_id: Some(TabId::new(7)),
                    to_session: "work".into(),
                },
            ),
            (
                "--from-session",
                Action::WarpTabFrom {
                    tab_id: TabId::new(7),
                    from_session: "work".into(),
                },
            ),
        ] {
            let parsed = try_parse_cli([
                "yinz", "action", "warp-tab", direction, "work", "--tab-id", "7",
            ])
            .unwrap();
            let Some(Command::Action(action)) = parsed.command else {
                panic!("expected an action");
            };
            assert_eq!(
                crate::actions::actions_from_cli(*action, Box::new(PathBuf::new), None).unwrap(),
                vec![expected],
            );
            assert!(try_parse_cli([
                "yinz",
                "action",
                "warp-tab",
                direction,
                "work",
                "--tab-index",
                "7",
            ])
            .is_err());
        }
        assert_eq!(
            crate::actions::actions_from_cli(
                CliAction::WarpTab {
                    tab_id: None,
                    to_session: None,
                    from_session: Some("work".into()),
                },
                Box::new(PathBuf::new),
                None
            )
            .unwrap_err(),
            "--from-session also needs --tab-id",
        );
    }

    #[test]
    fn removed_automation_spellings_have_no_aliases() {
        for args in [
            vec!["yinz", "action", "agent-list"],
            vec!["yinz", "action", "agent-wait", "terminal_7", "idle"],
            vec!["yinz", "action", "agent-runs"],
            vec!["yinz", "agents", "skills", "install"],
        ] {
            assert!(try_parse_cli(args).is_err());
        }
    }

    #[test]
    fn help_preserves_the_existing_cli() {
        for (args, expected) in [
            (
                vec!["yinz", "--help"],
                include_str!("test-fixtures/help-root.txt"),
            ),
            (
                vec!["yinz", "options", "--help"],
                include_str!("test-fixtures/help-options.txt"),
            ),
            (
                vec!["yinz", "action", "--help"],
                include_str!("test-fixtures/help-action.txt"),
            ),
        ] {
            let help = try_parse_cli(args).unwrap_err();
            assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
            // Clap can pad blank lines with spaces. Compare the visible help text.
            assert_eq!(
                help.to_string()
                    .lines()
                    .map(str::trim_end)
                    .collect::<Vec<_>>(),
                expected.lines().map(str::trim_end).collect::<Vec<_>>()
            );
        }
    }

    use super::*;
    use clap::Parser;
    use hegel::generators;

    const CLI_PARSE_STACK_SIZE: usize = 32 * 1024 * 1024;

    #[test]
    fn legacy_hocus_launchers_remain_compatible_with_running_servers() {
        for launcher in ["codex", "muse"] {
            let parsed = try_parse_cli([
                "yinz",
                "hocus",
                launcher,
                "--command",
                launcher,
                "--",
                "--help",
            ])
            .unwrap();
            match parsed.command {
                Some(Command::Agent(
                    Agent::Codex { command, args } | Agent::Muse { command, args },
                )) => {
                    assert_eq!(command, std::path::PathBuf::from(launcher));
                    assert_eq!(args, ["--help"]);
                },
                other => panic!("expected agent launcher, got {other:?}"),
            }
        }
    }

    /// clap's derived builder for `CliArgs` needs ~2.5MB of stack in a debug build (the
    /// derive expands the huge `CliAction` enum into one unoptimized function), more than
    /// libtest's 2MiB per-test threads. Parse on a thread with room.
    fn try_parse_cli<I, S>(args: I) -> Result<CliArgs, clap::Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        std::thread::Builder::new()
            .stack_size(CLI_PARSE_STACK_SIZE)
            .spawn(move || CliArgs::try_parse_from(args))
            .expect("failed to spawn cli parse thread")
            .join()
            .expect("cli parse thread panicked")
    }

    #[test]
    fn max_panes_must_be_positive_when_provided() {
        let error = try_parse_cli(["yinz", "--max-panes", "0"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        for limit in [1, 12, usize::MAX] {
            let cli = try_parse_cli(["yinz", "--max-panes", &limit.to_string()]).unwrap();
            assert_eq!(cli.max_panes, Some(limit));
        }
        assert_eq!(try_parse_cli(["yinz"]).unwrap().max_panes, None);
    }

    #[test]
    fn removed_startup_tips_flag_is_unknown() {
        let error = try_parse_cli(["yinz", "options", "--show-startup-tips", "true"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn a_plugin_launch_takes_the_url_and_nothing_else_by_default() {
        let cli = try_parse_cli(["yinz", "action", "launch-or-focus-plugin", "agents"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Action(action))
                if matches!(*action, CliAction::LaunchOrFocusPlugin {
                    floating: false,
                    in_place: false,
                    move_to_focused_tab: false,
                    ..
                })
        ));
    }

    #[test]
    fn list_sessions_exited_flag_is_opt_in() {
        let default = try_parse_cli(["yinz", "list-sessions"]).unwrap();
        assert!(matches!(
            default.command,
            Some(Command::Sessions(Sessions::ListSessions {
                exited: false,
                ..
            }))
        ));

        let exited = try_parse_cli(["yinz", "list-sessions", "--exited"]).unwrap();
        assert!(matches!(
            exited.command,
            Some(Command::Sessions(Sessions::ListSessions {
                exited: true,
                ..
            }))
        ));
    }

    /// Every field of a `yinz subscribe` invocation survives the argv round-trip, with
    /// the documented convention that a bare `--scrollback` (all scrollback) is `Some(0)`.
    #[hegel::test]
    fn subscribe_argv_roundtrips(tc: hegel::TestCase) {
        let pane_ids: Vec<String> = tc
            .draw(
                generators::vecs(generators::sampled_from(vec![
                    "terminal_1",
                    "terminal_42",
                    "plugin_2",
                    "3",
                ]))
                .min_size(1)
                .max_size(4),
            )
            .iter()
            .map(|s| s.to_string())
            .collect();
        // None = flag absent, Some(None) = bare flag, Some(Some(n)) = flag with a value
        let scrollback_arg = tc.draw(generators::optional(generators::optional(
            generators::integers::<usize>().max_value(10_000),
        )));
        let json = tc.draw(generators::booleans());
        let ansi = tc.draw(generators::booleans());

        let mut args = vec!["yinz".to_string(), "subscribe".to_string()];
        for id in &pane_ids {
            args.push("--pane-id".into());
            args.push(id.clone());
        }
        match scrollback_arg {
            None => {},
            Some(None) => args.push("--scrollback".into()),
            Some(Some(n)) => {
                args.push("--scrollback".into());
                args.push(n.to_string());
            },
        }
        if json {
            args.push("--format".into());
            args.push("json".into());
        }
        if ansi {
            args.push("--ansi".into());
        }

        let parsed = try_parse_cli(args.clone())
            .unwrap_or_else(|e| panic!("failed to parse {:?}: {}", args, e));
        let s = match parsed.command {
            Some(Command::Subscribe(s)) => s,
            other => panic!("expected Subscribe from {:?}, got {:?}", args, other),
        };

        assert_eq!(s.pane_id, pane_ids, "args: {:?}", args);
        assert_eq!(
            s.scrollback,
            match scrollback_arg {
                None => None,
                Some(None) => Some(0),
                Some(Some(n)) => Some(n),
            },
            "args: {:?}",
            args
        );
        assert_eq!(
            matches!(s.format, SubscribeFormat::Json),
            json,
            "args: {:?}",
            args
        );
        assert_eq!(s.ansi, ansi, "args: {:?}", args);
    }

    #[test]
    fn subscribe_requires_pane_id() {
        let result = try_parse_cli(["yinz", "subscribe"]);
        assert!(result.is_err());
    }

    #[test]
    fn plugins_write_takes_a_name_a_file_and_the_two_flags() {
        let parsed = try_parse_cli([
            "yinz",
            "plugins",
            "write",
            "keys",
            "--file",
            "/tmp/keys.scm",
            "--overwrite",
        ])
        .unwrap();
        match parsed.command {
            Some(Command::Plugins(Plugins::Write {
                name,
                file,
                overwrite,
                no_check,
            })) => {
                assert_eq!(name, "keys");
                assert_eq!(file, Some(PathBuf::from("/tmp/keys.scm")));
                assert!(overwrite && !no_check);
            },
            other => panic!("expected plugins write, got {other:?}"),
        }
        assert!(try_parse_cli(["yinz", "plugins", "check"]).is_ok());
        assert!(try_parse_cli(["yinz", "plugins", "log", "keys", "-l", "5"]).is_ok());
        assert!(try_parse_cli(["yinz", "plugins", "dir"]).is_ok());
        assert!(try_parse_cli(["yinz", "plugins", "write"]).is_err());
    }

    #[test]
    fn lifecycle_events_and_watch_share_filters() {
        let watch = try_parse_cli([
            "yinz",
            "agent",
            "watch",
            "--pane",
            "terminal_7",
            "--kind",
            "codex",
            "--kind",
            "claude",
            "--project",
            "yinz",
            "--lifecycle-type",
            "turn_completed",
            "--cursor",
            "saved",
            "--json",
        ])
        .unwrap();
        let Some(Command::Agent(Agent::Watch { filters, json })) = watch.command else {
            panic!("expected watch");
        };
        assert!(json);
        assert_eq!(filters.pane[0].to_string(), "terminal_7");
        assert_eq!(filters.kind, ["codex", "claude"]);
        assert_eq!(filters.project, ["yinz"]);
        assert_eq!(filters.cursor.as_deref(), Some("saved"));
        let events = try_parse_cli([
            "yinz",
            "events",
            "--event-type",
            "agent_lifecycle",
            "--pane",
            "7",
        ])
        .unwrap();
        let Some(Command::Events(events)) = events.command else {
            panic!("expected events");
        };
        assert_eq!(events.event_type, ["agent_lifecycle"]);
        assert_eq!(events.filters.pane, filters.pane);
        assert!(try_parse_cli(["yinz", "agent", "watch", "--pane", "plugin_7"]).is_err());
    }

    #[test]
    fn events_takes_any_number_of_types() {
        let parsed = try_parse_cli(["yinz", "events"]).unwrap();
        match parsed.command {
            Some(Command::Events(events)) => assert!(events.event_type.is_empty()),
            other => panic!("expected Events, got {other:?}"),
        }
        let parsed = try_parse_cli([
            "yinz",
            "events",
            "--type",
            "PaneUpdate",
            "cwd_changed",
            "-t",
            "TabUpdate",
        ])
        .unwrap();
        match parsed.command {
            Some(Command::Events(events)) => assert_eq!(
                events.event_type,
                vec!["PaneUpdate", "cwd_changed", "TabUpdate"]
            ),
            other => panic!("expected Events, got {other:?}"),
        }
    }

    /// Both spellings name an event, and only a streamable one: a keystroke is one client's
    /// input, not session state, however it is spelled.
    #[test]
    fn event_types_are_named_in_either_spelling_and_only_if_streamable() {
        use yinz_types::data::EventType;
        assert_eq!(
            EventType::from_cli_name("PaneUpdate"),
            Some(EventType::PaneUpdate)
        );
        assert_eq!(
            EventType::from_cli_name("pane_update"),
            Some(EventType::PaneUpdate)
        );
        assert_eq!(
            EventType::from_cli_name(" file_system_create "),
            Some(EventType::FileSystemCreate)
        );
        assert_eq!(EventType::from_cli_name("Key"), None);
        assert_eq!(EventType::from_cli_name("key"), None);
        assert_eq!(EventType::from_cli_name("NotAnEvent"), None);
        for name in EventType::cli_streamable_names() {
            assert!(EventType::from_cli_name(&name).is_some(), "{name}");
        }
    }
    #[test]
    fn neutral_agent_cli_keeps_hooks_separate_from_launchers() {
        for args in [
            vec!["yinz", "hooks", "status"],
            vec!["yinz", "agent", "codex", "--", "--model", "local"],
            vec!["yinz", "agent", "muse-report"],
            vec!["yinz", "hocus", "codex"],
        ] {
            assert!(try_parse_cli(args).is_ok());
        }
        for args in [
            vec!["yinz", "hocus", "hooks", "status"],
            vec!["yinz", "agent", "hooks", "status"],
        ] {
            assert!(try_parse_cli(args).is_err());
        }
    }
}
