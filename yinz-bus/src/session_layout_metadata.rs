use crate::ClientId;
use crate::PaneId;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use yinz_runtime::common_path::common_path_all;
use yinz_types::internal_ids::{PluginId, TerminalId};
use yinz_types::pane_size::PaneGeom;
use {
    yinz_session::session_serialization::extract_command_and_args,
    yinz_session::session_serialization::extract_edit_and_line_number,
    yinz_session::session_serialization::extract_plugin_and_config,
    yinz_types::data::LayoutMetadata, yinz_types::data::PaneMetadata,
    yinz_types::data::TabMetadata, yinz_types::input::command::RunCommand,
    yinz_types::input::layout::Layout, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::plugins::PluginAliases, yinz_types::session_manifest::GlobalLayoutManifest,
    yinz_types::session_manifest::PaneLayoutManifest,
    yinz_types::session_manifest::TabLayoutManifest,
};

#[derive(Default, Debug, Clone)]
pub struct SessionLayoutMetadata {
    default_layout: Box<Layout>,
    global_cwd: Option<PathBuf>,
    pub default_shell: Option<PathBuf>,
    pub default_editor: Option<PathBuf>,
    tabs: Vec<TabLayoutMetadata>,
}

impl SessionLayoutMetadata {
    pub fn new(default_layout: Box<Layout>) -> Self {
        SessionLayoutMetadata {
            default_layout,
            ..Default::default()
        }
    }
    pub fn update_default_shell(&mut self, default_shell: PathBuf) {
        if self.default_shell.is_none() {
            self.default_shell = Some(default_shell);
        }
        for tab in self.tabs.iter_mut() {
            for tiled_pane in tab.tiled_panes.iter_mut() {
                if let Some(Run::Command(run_command)) = tiled_pane.run.as_mut()
                    && !run_command.target.is_bubblewrap()
                    && Self::is_default_shell(
                        self.default_shell.as_ref(),
                        &run_command.command.display().to_string(),
                        &run_command.args,
                    )
                {
                    tiled_pane.run = None;
                }
            }
            for floating_pane in tab.floating_panes.iter_mut() {
                if let Some(Run::Command(run_command)) = floating_pane.run.as_mut()
                    && !run_command.target.is_bubblewrap()
                    && Self::is_default_shell(
                        self.default_shell.as_ref(),
                        &run_command.command.display().to_string(),
                        &run_command.args,
                    )
                {
                    floating_pane.run = None;
                }
            }
        }
    }
    pub fn list_clients_metadata(&self) -> String {
        let mut clients_metadata: BTreeMap<ClientId, ClientMetadata> = BTreeMap::new();
        for tab in &self.tabs {
            let panes = if tab.hide_floating_panes {
                &tab.tiled_panes
            } else {
                &tab.floating_panes
            };
            for pane in panes {
                for focused_client in &pane.focused_clients {
                    clients_metadata.insert(
                        *focused_client,
                        ClientMetadata {
                            pane_id: pane.id,
                            command: pane.run.clone(),
                        },
                    );
                }
            }
        }
        ClientMetadata::render_many(clients_metadata, &self.default_editor)
    }
    pub fn all_clients_metadata(&self) -> BTreeMap<ClientId, ClientMetadata> {
        let mut clients_metadata: BTreeMap<ClientId, ClientMetadata> = BTreeMap::new();
        for tab in &self.tabs {
            let panes = if tab.hide_floating_panes {
                &tab.tiled_panes
            } else {
                &tab.floating_panes
            };
            for pane in panes {
                for focused_client in &pane.focused_clients {
                    clients_metadata.insert(
                        *focused_client,
                        ClientMetadata {
                            pane_id: pane.id,
                            command: pane.run.clone(),
                        },
                    );
                }
            }
        }
        clients_metadata
    }
    pub fn is_dirty(&self) -> bool {
        // here we check to see if the serialized layout would be different than the base one, and
        // thus is "dirty". A layout is considered dirty if one of the following is true:
        // 1. The current number of panes is different than the number of panes in the base layout
        //    (meaning a pane was opened or closed)
        // 2. One or more terminal panes are running a command that is not the default shell
        let base_layout_pane_count = self.default_layout.pane_count();
        let current_pane_count = self.pane_count();
        if current_pane_count != base_layout_pane_count {
            return true;
        }
        for tab in &self.tabs {
            for tiled_pane in &tab.tiled_panes {
                match tiled_pane.run.as_ref() {
                    Some(Run::Command(run_command)) => {
                        if !Self::is_default_shell(
                            self.default_shell.as_ref(),
                            &run_command.command.display().to_string(),
                            &run_command.args,
                        ) {
                            return true;
                        }
                    },
                    Some(Run::EditFile(_, _, _)) => return true,
                    _ => {},
                }
            }
            for floating_pane in &tab.floating_panes {
                match floating_pane.run.as_ref() {
                    Some(Run::Command(run_command)) => {
                        if !Self::is_default_shell(
                            self.default_shell.as_ref(),
                            &run_command.command.display().to_string(),
                            &run_command.args,
                        ) {
                            return true;
                        }
                    },
                    Some(Run::EditFile(_, _, _)) => return true,
                    _ => {},
                }
            }
        }
        false
    }
    fn pane_count(&self) -> usize {
        let mut pane_count = 0;
        for tab in &self.tabs {
            for tiled_pane in &tab.tiled_panes {
                if !self.should_exclude_from_count(tiled_pane) {
                    pane_count += 1;
                }
            }
            for floating_pane in &tab.floating_panes {
                if !self.should_exclude_from_count(floating_pane) {
                    pane_count += 1;
                }
            }
        }
        pane_count
    }
    fn should_exclude_from_count(&self, pane: &PaneLayoutMetadata) -> bool {
        if let Some(Run::Plugin(run_plugin)) = &pane.run {
            let location_string = run_plugin.location_string();
            if location_string == "yinz:session-manager" {
                return true;
            }
            if location_string == "yinz:plugin-manager" {
                return true;
            }
            if location_string == "yinz:configuration-manager" {
                return true;
            }
        }
        false
    }
    fn is_default_shell(
        default_shell: Option<&PathBuf>,
        command_name: &str,
        args: &[String],
    ) -> bool {
        default_shell
            .as_ref()
            .map(|c| c.display().to_string())
            .as_deref()
            == Some(command_name)
            && args.is_empty()
    }
}

impl SessionLayoutMetadata {
    pub fn add_tab(
        &mut self,
        name: String,
        is_focused: bool,
        hide_floating_panes: bool,
        user_named: bool,
        tiled_panes: Vec<PaneLayoutMetadata>,
        floating_panes: Vec<PaneLayoutMetadata>,
    ) {
        self.tabs.push(TabLayoutMetadata {
            name: Some(name),
            is_focused,
            hide_floating_panes,
            user_named,
            tab_group: None,
            group_focused: false,
            tiled_panes,
            floating_panes,
        })
    }
    pub fn set_last_tab_group(
        &mut self,
        group: Option<yinz_types::tab_bar::TabGroupInfo>,
        focused: bool,
    ) {
        if let Some(tab) = self.tabs.last_mut() {
            tab.tab_group = group;
            tab.group_focused = focused;
        }
    }
    pub fn all_terminal_ids(&self) -> Vec<TerminalId> {
        let mut terminal_ids = vec![];
        for tab in &self.tabs {
            for pane_layout_metadata in &tab.tiled_panes {
                if let PaneId::Terminal(id) = pane_layout_metadata.id {
                    terminal_ids.push(id);
                }
            }
            for pane_layout_metadata in &tab.floating_panes {
                if let PaneId::Terminal(id) = pane_layout_metadata.id {
                    terminal_ids.push(id);
                }
            }
        }
        terminal_ids
    }
    pub fn all_plugin_ids(&self) -> Vec<PluginId> {
        let mut plugin_ids = vec![];
        for tab in &self.tabs {
            for pane_layout_metadata in &tab.tiled_panes {
                if let PaneId::Plugin(id) = pane_layout_metadata.id {
                    plugin_ids.push(id);
                }
            }
            for pane_layout_metadata in &tab.floating_panes {
                if let PaneId::Plugin(id) = pane_layout_metadata.id {
                    plugin_ids.push(id);
                }
            }
        }
        plugin_ids
    }
    pub fn remove_plugin_from_layout(&mut self, plugin_id_to_remove: PluginId) {
        for tab in &mut self.tabs {
            // Filter tiled panes
            tab.tiled_panes.retain(|pane| {
                if let PaneId::Plugin(id) = pane.id {
                    id != plugin_id_to_remove
                } else {
                    true
                }
            });

            // Filter floating panes
            tab.floating_panes.retain(|pane| {
                if let PaneId::Plugin(id) = pane.id {
                    id != plugin_id_to_remove
                } else {
                    true
                }
            });
        }
    }
    pub fn update_terminal_commands(
        &mut self,
        mut terminal_ids_to_commands: HashMap<TerminalId, Vec<String>>,
    ) {
        let mut update_cmd_in_pane_metadata = |pane_layout_metadata: &mut PaneLayoutMetadata| {
            // Process discovery sees the host launcher, not the command to resurrect.
            if matches!(&pane_layout_metadata.run, Some(Run::Command(command)) if command.target.is_bubblewrap())
            {
                return;
            }
            if let PaneId::Terminal(id) = pane_layout_metadata.id
                && let Some(command) = terminal_ids_to_commands.remove(&id)
            {
                let mut command_line = command.iter();
                if let Some(command_name) = command_line.next() {
                    let args: Vec<String> = command_line.map(|c| c.to_owned()).collect();
                    if Self::is_default_shell(self.default_shell.as_ref(), command_name, &args) {
                        pane_layout_metadata.run = None;
                    } else {
                        let mut run_command = RunCommand::new(PathBuf::from(command_name));
                        run_command.args = args;
                        pane_layout_metadata.run = Some(Run::Command(run_command));
                    }
                }
            }
        };
        for tab in self.tabs.iter_mut() {
            for pane_layout_metadata in tab.tiled_panes.iter_mut() {
                update_cmd_in_pane_metadata(pane_layout_metadata);
            }
            for pane_layout_metadata in tab.floating_panes.iter_mut() {
                update_cmd_in_pane_metadata(pane_layout_metadata);
            }
        }
    }
    pub fn update_terminal_cwds(&mut self, mut terminal_ids_to_cwds: HashMap<TerminalId, PathBuf>) {
        if let Some(common_path_between_cwds) =
            common_path_all(terminal_ids_to_cwds.values().map(|p| p.as_path()))
        {
            terminal_ids_to_cwds.values_mut().for_each(|p| {
                if let Ok(stripped) = p.strip_prefix(&common_path_between_cwds) {
                    *p = PathBuf::from(stripped)
                }
            });
            self.global_cwd = Some(common_path_between_cwds);
        }
        let mut update_cwd_in_pane_metadata = |pane_layout_metadata: &mut PaneLayoutMetadata| {
            if let PaneId::Terminal(id) = pane_layout_metadata.id
                && let Some(cwd) = terminal_ids_to_cwds.remove(&id)
            {
                pane_layout_metadata.cwd = Some(cwd);
            }
        };
        for tab in self.tabs.iter_mut() {
            for pane_layout_metadata in tab.tiled_panes.iter_mut() {
                update_cwd_in_pane_metadata(pane_layout_metadata);
            }
            for pane_layout_metadata in tab.floating_panes.iter_mut() {
                update_cwd_in_pane_metadata(pane_layout_metadata);
            }
        }
    }
    pub fn update_plugin_cmds(
        &mut self,
        mut plugin_ids_to_run_plugins: HashMap<PluginId, RunPlugin>,
    ) {
        let mut update_cmd_in_pane_metadata = |pane_layout_metadata: &mut PaneLayoutMetadata| {
            if let PaneId::Plugin(id) = pane_layout_metadata.id
                && let Some(run_plugin) = plugin_ids_to_run_plugins.remove(&id)
            {
                pane_layout_metadata.run =
                    Some(Run::Plugin(RunPluginOrAlias::RunPlugin(run_plugin)));
            }
        };
        for tab in self.tabs.iter_mut() {
            for pane_layout_metadata in tab.tiled_panes.iter_mut() {
                update_cmd_in_pane_metadata(pane_layout_metadata);
            }
            for pane_layout_metadata in tab.floating_panes.iter_mut() {
                update_cmd_in_pane_metadata(pane_layout_metadata);
            }
        }
    }
    pub fn update_default_editor(&mut self, default_editor: &Option<PathBuf>) {
        let default_editor = default_editor.clone().unwrap_or_else(|| {
            PathBuf::from(
                std::env::var("EDITOR")
                    .unwrap_or_else(|_| std::env::var("VISUAL").unwrap_or_else(|_| "vi".into())),
            )
        });
        self.default_editor = Some(default_editor);
    }
    pub fn detect_editor_panes(&mut self) {
        let default_editor = match &self.default_editor {
            Some(e) => e.clone(),
            None => return,
        };
        let editor_binary_name = default_editor
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let is_vim_family = |name: &str| matches!(name, "vim" | "nvim" | "emacs" | "nano" | "kak");
        let is_helix = |name: &str| matches!(name, "hx" | "helix");
        // Narrow vi/vim lineage used for cross-matching.
        // These are argument-compatible and commonly aliased to one another.
        let is_vi_vim = |name: &str| matches!(name, "vi" | "vim" | "nvim");

        let configured_is_vi_vim = is_vi_vim(&editor_binary_name);
        let configured_is_helix = is_helix(&editor_binary_name);

        let upgrade_pane = |pane: &mut PaneLayoutMetadata| {
            if let Some(Run::Command(run_command)) = &pane.run {
                if run_command.target.is_bubblewrap() {
                    return;
                }
                let command_binary_name = run_command
                    .command
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let is_editor = !editor_binary_name.is_empty()
                    && (command_binary_name == editor_binary_name
                        || run_command.command == default_editor
                        || (configured_is_vi_vim && is_vi_vim(&command_binary_name))
                        || (configured_is_helix && is_helix(&command_binary_name)));
                if !is_editor {
                    return;
                }

                let args = &run_command.args;
                let binary = &command_binary_name;

                let edit_file: Option<(PathBuf, Option<usize>)> = if is_vim_family(binary) {
                    match args.len() {
                        1 => args
                            .first()
                            .filter(|f| !f.starts_with('-'))
                            .map(|f| (PathBuf::from(f), None)),
                        2 => match (args.first(), args.get(1)) {
                            (Some(line_arg), Some(file)) => line_arg
                                .strip_prefix('+')
                                .and_then(|n| n.parse::<usize>().ok())
                                .map(|line| (PathBuf::from(file), Some(line))),
                            _ => None,
                        },
                        _ => None,
                    }
                } else if is_helix(binary) {
                    if args.len() == 1 {
                        args.first().map(|arg| {
                            if let Some(colon_pos) = arg.rfind(':') {
                                let file_part = &arg[..colon_pos];
                                if let Some(line) = arg
                                    .get(colon_pos + 1..)
                                    .and_then(|s| s.parse::<usize>().ok())
                                {
                                    return (PathBuf::from(file_part), Some(line));
                                }
                            }
                            (PathBuf::from(arg.as_str()), None)
                        })
                    } else {
                        None
                    }
                } else {
                    if args.len() == 1 {
                        args.first()
                            .filter(|f| !f.starts_with('-'))
                            .map(|f| (PathBuf::from(f), None))
                    } else {
                        None
                    }
                };

                if let Some((file_path, line_number)) = edit_file {
                    pane.run = Some(Run::EditFile(file_path, line_number, None));
                }
            }
        };

        for tab in self.tabs.iter_mut() {
            for pane in tab.tiled_panes.iter_mut() {
                upgrade_pane(pane);
            }
            for pane in tab.floating_panes.iter_mut() {
                upgrade_pane(pane);
            }
        }
    }
    pub fn update_plugin_aliases_in_default_layout(&mut self, plugin_aliases: &PluginAliases) {
        self.default_layout
            .populate_plugin_aliases_in_layout(plugin_aliases);
    }
    pub fn to_layout_metadata(&self) -> LayoutMetadata {
        // Get current timestamp for both creation and update time
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();

        // Convert all tabs
        let tabs = self.tabs.iter().map(|tab| tab.to_tab_metadata()).collect();

        LayoutMetadata {
            tabs,
            creation_time: current_time.clone(),
            update_time: current_time,
        }
    }
}

impl From<SessionLayoutMetadata> for GlobalLayoutManifest {
    fn from(val: SessionLayoutMetadata) -> Self {
        GlobalLayoutManifest {
            default_layout: val.default_layout,
            default_shell: val.default_shell,
            global_cwd: val.global_cwd,
            tabs: val
                .tabs
                .into_iter()
                .map(|t| (t.name.clone().unwrap_or_default(), t.into()))
                .collect(),
        }
    }
}

impl From<TabLayoutMetadata> for TabLayoutManifest {
    fn from(val: TabLayoutMetadata) -> Self {
        TabLayoutManifest {
            tiled_panes: val.tiled_panes.into_iter().map(|t| t.into()).collect(),
            floating_panes: val.floating_panes.into_iter().map(|t| t.into()).collect(),
            is_focused: val.is_focused,
            hide_floating_panes: val.hide_floating_panes,
            user_named: val.user_named,
            tab_group: val.tab_group,
            group_focused: val.group_focused,
        }
    }
}

impl TabLayoutMetadata {
    fn to_tab_metadata(&self) -> TabMetadata {
        let mut panes = Vec::new();

        // Extract pane metadata from tiled panes
        for pane in &self.tiled_panes {
            panes.push(pane.to_pane_metadata());
        }

        // Extract pane metadata from floating panes
        for pane in &self.floating_panes {
            panes.push(pane.to_pane_metadata());
        }

        TabMetadata {
            panes,
            name: self.name.clone(),
        }
    }
}

impl From<PaneLayoutMetadata> for PaneLayoutManifest {
    fn from(val: PaneLayoutMetadata) -> Self {
        PaneLayoutManifest {
            pane_tabs: val.pane_tabs,
            geom: val.geom,
            run: val.run,
            cwd: val.cwd,
            is_borderless: val.is_borderless,
            title: val.title,
            is_focused: val.is_focused,
            pane_contents: val.pane_contents,
            default_fg: val.default_fg,
            default_bg: val.default_bg,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TabLayoutMetadata {
    name: Option<String>,
    tiled_panes: Vec<PaneLayoutMetadata>,
    floating_panes: Vec<PaneLayoutMetadata>,
    is_focused: bool,
    hide_floating_panes: bool,
    user_named: bool,
    tab_group: Option<yinz_types::tab_bar::TabGroupInfo>,
    group_focused: bool,
}

#[derive(Debug, Clone)]
pub struct PaneLayoutMetadata {
    pub pane_tabs: Option<yinz_types::pane_tabs::PaneTabsLayout>,
    id: PaneId,
    geom: PaneGeom,
    run: Option<Run>,
    cwd: Option<PathBuf>,
    is_borderless: bool,
    title: Option<String>,
    is_focused: bool,
    pane_contents: Option<String>,
    focused_clients: Vec<ClientId>,
    default_fg: Option<String>,
    default_bg: Option<String>,
}

impl PaneLayoutMetadata {
    pub fn with_pane_tabs(mut self, tabs: Option<&yinz_types::pane_tabs::PaneTabsInfo>) -> Self {
        // Terminal-backed groups are runtime-only; layout descriptors can only restore plugins.
        let tabs = tabs.filter(|tabs| tabs.tabs.iter().all(|slot| slot.terminal_id.is_none()));
        self.pane_tabs = tabs.map(|tabs| yinz_types::pane_tabs::PaneTabsLayout {
            active: tabs.active,
            tabs: tabs
                .tabs
                .iter()
                .map(|slot| slot.descriptor.clone())
                .collect(),
        });
        self
    }

    pub fn new(
        id: PaneId,
        geom: PaneGeom,
        is_borderless: bool,
        run: Option<Run>,
        title: Option<String>,
        is_focused: bool,
        pane_contents: Option<String>,
        focused_clients: Vec<ClientId>,
        default_fg: Option<String>,
        default_bg: Option<String>,
    ) -> Self {
        PaneLayoutMetadata {
            pane_tabs: None,
            id,
            geom,
            run,
            cwd: None,
            is_borderless,
            title,
            is_focused,
            pane_contents,
            focused_clients,
            default_fg,
            default_bg,
        }
    }

    fn to_pane_metadata(&self) -> PaneMetadata {
        // Try to extract a meaningful name from the pane
        // Priority: explicit title > command name > file name > plugin location
        let name = self.title.clone().or_else(|| {
            self.run.as_ref().and_then(|run| match run {
                Run::Command(cmd) => Some(cmd.command.display().to_string()),
                Run::EditFile(path, _, _) => {
                    path.file_name().map(|n| n.to_string_lossy().to_string())
                },
                Run::Plugin(plugin) => Some(plugin.location_string()),
                Run::Cwd(_) => None,
            })
        });

        let is_plugin = matches!(self.id, PaneId::Plugin(_));

        // Detect if this is a builtin plugin
        let is_builtin_plugin = self
            .run
            .as_ref()
            .map(|run| match run {
                Run::Plugin(plugin) => plugin.is_builtin_plugin(),
                _ => false,
            })
            .unwrap_or(false);

        PaneMetadata {
            name,
            is_plugin,
            is_builtin_plugin,
        }
    }
}

pub struct ClientMetadata {
    pane_id: PaneId,
    command: Option<Run>,
}
impl ClientMetadata {
    pub fn stringify_pane_id(&self) -> String {
        match self.pane_id {
            PaneId::Terminal(terminal_id) => format!("terminal_{}", terminal_id),
            PaneId::Plugin(plugin_id) => format!("plugin_{}", plugin_id),
        }
    }
    pub fn stringify_command(&self, editor: &Option<PathBuf>) -> String {
        let stringified = match &self.command {
            Some(Run::Command(..)) => {
                let (command, args) = extract_command_and_args(&self.command);
                command.map(|c| format!("{} {}", c, args.join(" ")))
            },
            Some(Run::EditFile(..)) => {
                let (file_to_edit, _line_number) = extract_edit_and_line_number(&self.command);
                editor.as_ref().and_then(|editor| {
                    file_to_edit
                        .map(|file_to_edit| format!("{} {}", editor.display(), file_to_edit))
                })
            },
            Some(Run::Plugin(..)) => {
                let (plugin, _plugin_config) = extract_plugin_and_config(&self.command);
                plugin.map(|p| p.to_string())
            },
            _ => None,
        };
        stringified.unwrap_or("N/A".to_owned())
    }
    pub fn get_pane_id(&self) -> PaneId {
        self.pane_id
    }
    pub fn render_many(
        clients_metadata: BTreeMap<ClientId, ClientMetadata>,
        default_editor: &Option<PathBuf>,
    ) -> String {
        let mut lines = vec![Self::render_row(
            "CLIENT_ID",
            "YINZ_PANE_ID",
            "RUNNING_COMMAND",
        )];
        for (client_id, client_metadata) in clients_metadata.iter() {
            lines.push(Self::render_row(
                client_id,
                client_metadata.stringify_pane_id(),
                client_metadata.stringify_command(default_editor),
            ));
        }
        lines.join("\n")
    }
    /// One line of `yinz action list-clients`. The header goes through here too,
    /// so it and the rows share one set of column widths: CLIENT_ID 10 (a u32 id), YINZ_PANE_ID
    /// 14 (room for `terminal_12345`, wider than its 12-character name), and
    /// RUNNING_COMMAND 15.
    fn render_row(
        client_id: impl fmt::Display,
        pane_id: impl fmt::Display,
        running_command: impl fmt::Display,
    ) -> String {
        const CLIENT_ID_WIDTH: usize = 10;
        const PANE_ID_WIDTH: usize = 14;
        const RUNNING_COMMAND_WIDTH: usize = 15;
        format!(
            "{client_id: <CLIENT_ID_WIDTH$} {pane_id: <PANE_ID_WIDTH$} \
             {running_command: <RUNNING_COMMAND_WIDTH$}"
        )
    }
}

#[cfg(test)]
#[path = "unit/session_layout_metadata.rs"]
mod pane_tabs_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use yinz_types::pane_size::PaneGeom;

    fn make_command_pane(
        terminal_id: TerminalId,
        command: &str,
        args: Vec<&str>,
    ) -> PaneLayoutMetadata {
        let mut run_command = RunCommand::new(PathBuf::from(command));
        run_command.args = args.into_iter().map(|s| s.to_string()).collect();
        PaneLayoutMetadata::new(
            PaneId::Terminal(terminal_id),
            PaneGeom::default(),
            false,
            Some(Run::Command(run_command)),
            None,
            false,
            None,
            vec![],
            None,
            None,
        )
    }

    #[test]
    fn bubblewrap_commands_survive_running_metadata_and_session_roundtrip() {
        use yinz_types::command::PaneTarget;
        for (program, args) in [
            ("claude", vec!["--resume".into()]),
            ("/bin/sh", vec![]),
            ("nvim", vec!["notes.md".into()]),
        ] {
            for floating in [false, true] {
                let command = RunCommand {
                    command: program.into(),
                    args: args.clone(),
                    hold_on_close: true,
                    target: PaneTarget::bubblewrap(std::path::Path::new("/repo")).unwrap(),
                    ..Default::default()
                };
                let mut pane = make_command_pane(TerminalId::new(1), "claude", vec![]);
                pane.run = Some(Run::Command(command.clone()));
                pane.geom.rows = yinz_types::pane_size::Dimension::fixed(10);
                pane.geom.cols = yinz_types::pane_size::Dimension::fixed(10);
                let mut metadata = SessionLayoutMetadata::default();
                let (tiled, floats) = if floating {
                    (vec![], vec![pane])
                } else {
                    (vec![pane], vec![])
                };
                metadata.add_tab("sandbox".into(), true, false, false, tiled, floats);
                metadata.update_terminal_commands(HashMap::from([(
                    TerminalId::new(1),
                    vec!["bwrap".into(), "--args".into(), "17".into()],
                )]));
                metadata.update_default_shell("/bin/sh".into());
                metadata.update_default_editor(&Some("nvim".into()));
                metadata.detect_editor_panes();
                let pane = if floating {
                    &metadata.tabs[0].floating_panes[0]
                } else {
                    &metadata.tabs[0].tiled_panes[0]
                };
                assert_eq!(pane.run, Some(Run::Command(command.clone())));
                let (source, _) =
                    yinz_session::session_serialization::serialize_session_layout(metadata.into())
                        .unwrap();
                let layout = yinz_config::kdl::layout::from_kdl(&source, None, None, None).unwrap();
                let (_, tiled, floats) = &layout.tabs[0];
                let mut panes = vec![tiled];
                let mut commands = floats
                    .iter()
                    .filter_map(|pane| pane.run.as_ref())
                    .collect::<Vec<_>>();
                while let Some(pane) = panes.pop() {
                    commands.extend(pane.run.as_ref());
                    panes.extend(&pane.children);
                }
                let mut restored = command;
                restored.hold_on_start = true;
                assert!(commands.contains(&&Run::Command(restored)), "{source}");
            }
        }
    }

    fn make_edit_file_pane(
        terminal_id: TerminalId,
        path: &str,
        line_number: Option<usize>,
    ) -> PaneLayoutMetadata {
        PaneLayoutMetadata::new(
            PaneId::Terminal(terminal_id),
            PaneGeom::default(),
            false,
            Some(Run::EditFile(PathBuf::from(path), line_number, None)),
            None,
            false,
            None,
            vec![],
            None,
            None,
        )
    }

    fn session_with_editor(editor: &str, panes: Vec<PaneLayoutMetadata>) -> SessionLayoutMetadata {
        let mut meta = SessionLayoutMetadata {
            default_editor: Some(PathBuf::from(editor)),
            ..Default::default()
        };
        meta.add_tab("tab1".to_string(), true, false, false, panes, vec![]);
        meta
    }

    fn get_first_tiled_run(meta: &SessionLayoutMetadata) -> Option<&Run> {
        meta.tabs[0].tiled_panes[0].run.as_ref()
    }

    // detects_editor_pane_no_line_number: covered by the group-0 draw with
    // with_line = false. The two line-number cases below are kept as the
    // documentation of the two argument syntaxes.

    #[test]
    fn detects_editor_pane_with_line_number_vim_family() {
        let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec!["+50", "file.txt"]);
        let mut meta = session_with_editor("nvim", vec![pane]);
        meta.detect_editor_panes();
        assert_eq!(
            get_first_tiled_run(&meta),
            Some(&Run::EditFile(PathBuf::from("file.txt"), Some(50), None))
        );
    }

    #[test]
    fn detects_editor_pane_with_line_number_helix() {
        let pane = make_command_pane(crate::TerminalId::new(1), "hx", vec!["file.txt:50"]);
        let mut meta = session_with_editor("hx", vec![pane]);
        meta.detect_editor_panes();
        assert_eq!(
            get_first_tiled_run(&meta),
            Some(&Run::EditFile(PathBuf::from("file.txt"), Some(50), None))
        );
    }

    // detects_editor_pane_helix_no_line_number: covered by the group-1 draw
    // with with_line = false.

    #[test]
    fn skips_non_editor_command() {
        let pane = make_command_pane(
            crate::TerminalId::new(1),
            "grep",
            vec!["pattern", "file.txt"],
        );
        let mut meta = session_with_editor("nvim", vec![pane]);
        meta.detect_editor_panes();
        // Run::Command(grep ...) unchanged
        match get_first_tiled_run(&meta) {
            Some(Run::Command(rc)) => {
                assert_eq!(rc.command, PathBuf::from("grep"));
            },
            other => panic!("expected Command, got {:?}", other),
        }
    }

    #[test]
    fn skips_editor_with_no_args() {
        let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec![]);
        let mut meta = session_with_editor("nvim", vec![pane]);
        meta.detect_editor_panes();
        match get_first_tiled_run(&meta) {
            Some(Run::Command(rc)) => {
                assert_eq!(rc.command, PathBuf::from("nvim"));
                assert!(rc.args.is_empty());
            },
            other => panic!("expected Command, got {:?}", other),
        }
    }

    #[test]
    fn skips_editor_with_multiple_files() {
        let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec!["a.txt", "b.txt"]);
        let mut meta = session_with_editor("nvim", vec![pane]);
        meta.detect_editor_panes();
        match get_first_tiled_run(&meta) {
            Some(Run::Command(rc)) => {
                assert_eq!(rc.command, PathBuf::from("nvim"));
                assert_eq!(rc.args, vec!["a.txt", "b.txt"]);
            },
            other => panic!("expected Command, got {:?}", other),
        }
    }

    #[test]
    fn detects_editor_matching_by_binary_name() {
        // configured as /usr/bin/nvim, running as nvim
        let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec!["file.txt"]);
        let mut meta = session_with_editor("/usr/bin/nvim", vec![pane]);
        meta.detect_editor_panes();
        assert_eq!(
            get_first_tiled_run(&meta),
            Some(&Run::EditFile(PathBuf::from("file.txt"), None, None))
        );
    }

    #[test]
    fn does_not_affect_existing_edit_file_run() {
        let pane = make_edit_file_pane(crate::TerminalId::new(1), "file.txt", Some(10));
        let mut meta = session_with_editor("nvim", vec![pane]);
        meta.detect_editor_panes();
        assert_eq!(
            get_first_tiled_run(&meta),
            Some(&Run::EditFile(PathBuf::from("file.txt"), Some(10), None))
        );
    }

    // detects_vi_when_editor_is_vim and detects_nvim_when_editor_is_vi: the
    // group-0 draw picks the configured editor and the pane's binary
    // independently from {vi, vim, nvim}, so it produces both pairings. The
    // negative case below stays - the generator never crosses the groups.

    #[test]
    fn does_not_cross_match_vim_with_emacs() {
        // configured as emacs, pane running vim — NOT cross-matched
        let pane = make_command_pane(crate::TerminalId::new(1), "vim", vec!["file.txt"]);
        let mut meta = session_with_editor("emacs", vec![pane]);
        meta.detect_editor_panes();
        match get_first_tiled_run(&meta) {
            Some(Run::Command(rc)) => assert_eq!(rc.command, PathBuf::from("vim")),
            other => panic!("expected Command, got {:?}", other),
        }
    }

    #[test]
    fn list_clients_header_lines_up_with_its_rows() {
        let clients = BTreeMap::from([(
            ClientId::new(1),
            ClientMetadata {
                pane_id: PaneId::Terminal(crate::TerminalId::new(2)),
                command: Some(Run::Command(RunCommand::new(PathBuf::from("htop")))),
            },
        )]);
        let rendered = ClientMetadata::render_many(clients, &None);
        let lines: Vec<&str> = rendered.lines().collect();
        let [header, row] = lines[..] else {
            panic!("expected a header and one row:\n{rendered}");
        };
        let column = |line: &str, cell: &str| {
            line.find(cell)
                .unwrap_or_else(|| panic!("{cell:?} missing from {line:?}"))
        };
        assert_eq!(
            column(header, "YINZ_PANE_ID"),
            column(row, "terminal_2"),
            "pane id column misaligned:\n{rendered}"
        );
        assert_eq!(
            column(header, "RUNNING_COMMAND"),
            column(row, "htop"),
            "running command column misaligned:\n{rendered}"
        );
    }

    // detects_hx_when_editor_is_helix: the group-1 draw pairs the configured
    // editor and the pane binary independently across {hx, helix}.
    //
    // absolute_file_path_preserved: "/home/user/file.txt" is one of the
    // generator's file_paths, drawn on every group.

    mod props {
        use super::*;
        use hegel::generators;

        /// File paths a pane could plausibly have been launched on. None start
        /// with `-`, which `detect_editor_panes` treats as a flag rather than a
        /// file.
        fn file_paths() -> Vec<&'static str> {
            vec![
                "file.txt",
                "src/main.rs",
                "/home/user/file.txt",
                "/tmp/a.log",
                "a-b.txt",
                "x",
                "Cargo.toml",
                "./relative.rs",
            ]
        }

        /// Paths that themselves contain a `:`, so that appending `:N` for
        /// helix produces more than one colon. The parser splits on the *last*
        /// one; these are what make that distinguishable from the first.
        fn file_paths_with_colons() -> Vec<&'static str> {
            vec!["weird:name.txt", "a:b:c.rs", "/tmp/ns:1/file.txt", "x:9"]
        }

        /// A command invocation `detect_editor_panes` must turn into an
        /// `EditFile` run, as `(configured editor, pane command, args, expected
        /// path, expected line)`.
        ///
        /// The three groups mirror the three ways the function decides a pane
        /// is an editor: the vi/vim/nvim lineage and the helix pair each
        /// cross-match within themselves, everything else has to match its own
        /// binary name exactly. Which argument syntax is legal depends on the
        /// *pane's* binary, not the configured one - `vi` is outside
        /// `is_vim_family`, so it never gets the `+N` form.
        #[hegel::composite]
        fn editor_invocation(
            tc: &hegel::TestCase,
        ) -> (String, String, Vec<String>, PathBuf, Option<usize>) {
            let line = tc.draw(generators::integers::<usize>().max_value(999_999));
            let group = tc.draw(generators::integers::<u8>().max_value(2));
            let (configured, command) = match group {
                0 => {
                    let lineage = vec!["vi", "vim", "nvim"];
                    (
                        tc.draw(generators::sampled_from(lineage.clone())),
                        tc.draw(generators::sampled_from(lineage)),
                    )
                },
                1 => {
                    let helix = vec!["hx", "helix"];
                    (
                        tc.draw(generators::sampled_from(helix.clone())),
                        tc.draw(generators::sampled_from(helix)),
                    )
                },
                _ => {
                    let solo = tc.draw(generators::sampled_from(vec!["emacs", "nano", "kak"]));
                    (solo, solo)
                },
            };

            let is_vim_family = matches!(command, "vim" | "nvim" | "emacs" | "nano" | "kak");
            let is_helix = matches!(command, "hx" | "helix");
            let with_line = tc.draw(generators::booleans());

            // Only the helix `file:N` form can carry a colon in the path: for
            // everything else the path is passed through untouched, so a colon
            // would add nothing.
            let file = if with_line && is_helix && tc.draw(generators::booleans()) {
                tc.draw(generators::sampled_from(file_paths_with_colons()))
            } else {
                tc.draw(generators::sampled_from(file_paths()))
            };

            let (args, expected_line) = if with_line && is_vim_family {
                (vec![format!("+{}", line), file.to_string()], Some(line))
            } else if with_line && is_helix {
                (vec![format!("{}:{}", file, line)], Some(line))
            } else {
                (vec![file.to_string()], None)
            };

            (
                configured.to_string(),
                command.to_string(),
                args,
                PathBuf::from(file),
                expected_line,
            )
        }

        #[hegel::test]
        fn any_known_editor_invocation_becomes_an_edit_file_run(tc: hegel::TestCase) {
            let (configured, command, args, path, line) = tc.draw(editor_invocation());
            let pane = make_command_pane(
                crate::TerminalId::new(1),
                &command,
                args.iter().map(|s| s.as_str()).collect(),
            );
            let mut meta = session_with_editor(&configured, vec![pane]);
            meta.detect_editor_panes();
            assert_eq!(
                get_first_tiled_run(&meta),
                Some(&Run::EditFile(path.clone(), line, None)),
                "editor {:?} running {:?} {:?} did not become EditFile({:?}, {:?})",
                configured,
                command,
                args,
                path,
                line
            );
        }

        /// `detect_editor_panes` mutates the metadata in place, so running it
        /// again on its own output must be a no-op - the second pass sees an
        /// `EditFile` run rather than a `Command` and has to leave it alone.
        #[hegel::test]
        fn detecting_editor_panes_twice_changes_nothing(tc: hegel::TestCase) {
            let (configured, command, args, _, _) = tc.draw(editor_invocation());
            let pane = make_command_pane(
                crate::TerminalId::new(1),
                &command,
                args.iter().map(|s| s.as_str()).collect(),
            );
            let mut meta = session_with_editor(&configured, vec![pane]);
            meta.detect_editor_panes();
            let after_once = get_first_tiled_run(&meta).cloned();
            meta.detect_editor_panes();
            assert_eq!(
                get_first_tiled_run(&meta),
                after_once.as_ref(),
                "second detect_editor_panes changed {:?} {:?}",
                command,
                args
            );
        }

        /// Commands that are not the configured editor keep their `Run::Command`
        /// verbatim. Cross-matching is deliberately narrow, so e.g. an `emacs`
        /// pane under a `vim` config must not be rewritten.
        #[hegel::test]
        fn a_non_editor_command_is_never_rewritten(tc: hegel::TestCase) {
            let configured = tc.draw(generators::sampled_from(vec![
                "vim", "nvim", "vi", "hx", "helix", "emacs",
            ]));
            let command = tc.draw(generators::sampled_from(vec![
                "grep", "cat", "tail", "ls", "cargo", "git", "bash",
            ]));
            let file = tc.draw(generators::sampled_from(file_paths()));

            let pane = make_command_pane(crate::TerminalId::new(1), command, vec![file]);
            let mut meta = session_with_editor(configured, vec![pane]);
            meta.detect_editor_panes();

            match get_first_tiled_run(&meta) {
                Some(Run::Command(run_command)) => {
                    assert_eq!(run_command.command, PathBuf::from(command));
                    assert_eq!(run_command.args, vec![file.to_string()]);
                },
                other => panic!(
                    "{:?} under editor {:?} became {:?}",
                    command, configured, other
                ),
            }
        }
    }
}
