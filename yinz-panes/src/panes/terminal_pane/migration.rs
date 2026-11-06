//! Pane metadata carried beside its exact semantic terminal state.
use super::*;
#[derive(serde::Serialize, serde::Deserialize)]
struct Snapshot {
    version: u32,
    grid: serde_json::Value,
    parser: serde_json::Value,
    parser_boundary: serde_json::Value,
    interceptor: serde_json::Value,
    selectable: bool,
    geom: PaneGeom,
    geom_override: Option<PaneGeom>,
    content_offset: Offset,
    max_content_width: usize,
    subtitle: Option<String>,
    subtitle_enabled: bool,
    pane_title: String,
    pane_name: String,
    prev_pane_name: String,
    remote_host: Option<String>,
    title_prefix: Option<String>,
    running_program: Option<String>,
    agent: Option<AgentInfo>,
    agent_input_revision: u64,
    agent_enter_revision: u64,
    was_an_agent: bool,
    last_agent_kind: Option<String>,
    last_agent_cwd: Option<String>,
    last_agent_session_id: Option<String>,
    last_hook_at_ms: Option<i64>,
    borderless: bool,
    exclude_from_sync: bool,
    search_term: String,
    is_held: Option<(Option<i32>, bool, RunCommand)>,
    suspended: bool,
    banner: Option<String>,
    has_bell_notification: bool,
    invoked_with: Option<Run>,
}
impl TerminalPane {
    /// Reflow restored terminal state into this pane's content rectangle.
    ///
    /// Only when the rectangle differs: a resize at the same dimensions would erase restored
    /// margins and tab stops. Shared by warp and session migration so the two restores agree.
    pub(super) fn fit_restored_grid(&mut self) {
        let rows = self.get_content_rows();
        let cols = self.get_content_columns();
        if self.grid.height != rows || self.grid.width != cols {
            self.grid.change_size(rows, cols);
        }
    }
    pub(super) fn check_migration(&self) -> Result<(), String> {
        if self.forward_paused || !self.pending_pty_input.is_empty() || self.nested_guest {
            return Err("terminal has pending host-query output or a nested session that cannot be transferred".into());
        }
        self.grid.validate_migration().map_err(|e| format!("{e:#}"))
    }
    pub(super) fn capture_migration(&self) -> Result<serde_json::Value, String> {
        self.check_migration()?;
        let snapshot = Snapshot {
            version: 2,
            parser_boundary: serde_json::to_value(&self.parser_boundary)
                .map_err(|e| format!("capture parser boundary: {e}"))?,
            parser: serde_json::to_value(&self.vte_parser)
                .map_err(|e| format!("capture parser continuation: {e}"))?,
            interceptor: serde_json::to_value(&self.kitty_interceptor)
                .map_err(|e| format!("capture APC continuation: {e}"))?,
            grid: self.grid.migration_state().map_err(|e| format!("{e:#}"))?,
            selectable: self.selectable,
            geom: self.geom,
            geom_override: self.geom_override,
            content_offset: self.content_offset,
            max_content_width: self.max_content_width,
            subtitle: self.subtitle.clone(),
            subtitle_enabled: self.subtitle_enabled,
            pane_title: self.pane_title.clone(),
            pane_name: self.pane_name.clone(),
            prev_pane_name: self.prev_pane_name.clone(),
            remote_host: self.remote_host.clone(),
            title_prefix: self.title_prefix.clone(),
            running_program: self.running_program.clone(),
            agent: self.agent.clone(),
            agent_input_revision: self.agent_input_revision,
            agent_enter_revision: self.agent_enter_revision,
            was_an_agent: self.was_an_agent,
            last_agent_kind: self.last_agent_kind.clone(),
            last_agent_cwd: self.last_agent_cwd.clone(),
            last_agent_session_id: self.last_agent_session_id.clone(),
            last_hook_at_ms: self.last_hook_at_ms,
            borderless: self.borderless,
            exclude_from_sync: self.exclude_from_sync,
            search_term: self.search_term.clone(),
            is_held: self.is_held.clone(),
            suspended: self.suspended,
            banner: self.banner.clone(),
            has_bell_notification: self.has_bell_notification,
            invoked_with: self.invoked_with.clone(),
        };
        serde_json::to_value(snapshot)
            .map_err(|error| format!("encode pane migration snapshot: {error}"))
    }
    pub(super) fn apply_migration(&mut self, value: serde_json::Value) -> Result<(), String> {
        let mut state: Snapshot = serde_json::from_value(value)
            .map_err(|error| format!("decode pane migration snapshot: {error}"))?;
        if state.version != 2 {
            return Err("unsupported pane migration snapshot version".into());
        }
        let parser: vte::Parser = serde_json::from_value(state.parser)
            .map_err(|e| format!("decode parser continuation: {e}"))?;
        if !parser.valid_continuation() {
            return Err("invalid terminal parser continuation".into());
        }
        let boundary = serde_json::from_value(state.parser_boundary)
            .map_err(|e| format!("decode parser boundary: {e}"))?;
        let interceptor = serde_json::from_value(state.interceptor)
            .map_err(|e| format!("decode APC continuation: {e}"))?;
        // A callback belongs to the source plugin's interpreter generation. Recreated plugins
        // have no corresponding pending invocation, even if their numeric IDs happen to match.
        if let Some((_, _, command)) = &mut state.is_held {
            command.originating_plugin = None;
        }
        if let Some(Run::Command(command)) = &mut state.invoked_with {
            command.originating_plugin = None;
        }
        self.grid
            .restore_migration_state(state.grid)
            .map_err(|error| format!("restore pane terminal state: {error:#}"))?;
        self.selectable = state.selectable;
        // Geometry, frame offsets and borderlessness stay as the destination's layout placed
        // them, as for a warped pane. The source laid this pane out for its own screen; copying
        // that rectangle over a placed pane leaves the tab's panes not tiling its display, so
        // the solver then refuses every resize. Fullscreen comes back through the tab's state.
        self.max_content_width = state.max_content_width;
        self.subtitle = state.subtitle;
        self.subtitle_enabled = state.subtitle_enabled;
        self.pane_title = state.pane_title;
        self.pane_name = state.pane_name;
        self.prev_pane_name = state.prev_pane_name;
        self.remote_host = state.remote_host;
        self.title_prefix = state.title_prefix;
        self.running_program = state.running_program;
        self.agent = state.agent;
        self.agent_input_revision = state.agent_input_revision;
        self.agent_enter_revision = state.agent_enter_revision;
        self.was_an_agent = state.was_an_agent;
        self.last_agent_kind = state.last_agent_kind;
        self.last_agent_cwd = state.last_agent_cwd;
        self.last_agent_session_id = state.last_agent_session_id;
        self.last_hook_at_ms = state.last_hook_at_ms;
        self.exclude_from_sync = state.exclude_from_sync;
        self.search_term = state.search_term;
        self.is_held = state.is_held;
        self.suspended = state.suspended;
        self.banner = state.banner;
        self.has_bell_notification = state.has_bell_notification;
        self.invoked_with = state.invoked_with;
        self.fit_restored_grid();
        self.set_should_render(true);
        self.frame.clear();
        self.fake_cursor_locations.clear();
        self.render_width = None;
        self.vte_parser = parser;
        self.parser_boundary = boundary;
        self.kitty_interceptor = interceptor;
        self.warp_pending = true;
        Ok(())
    }
}
