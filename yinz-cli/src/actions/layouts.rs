//! Layout loading and validation for tab creation and session switching.
use miette::Report;
use std::path::PathBuf;
use std::str::FromStr;
use yinz_config::config::{Config, ConfigError};
use yinz_runtime::home::{find_default_config_dir, get_layout_dir};
use yinz_types::data::{CommandOrPlugin, LayoutInfo, PaneId, UnblockCondition};
use yinz_types::input::actions::Action;
use yinz_types::input::command::RunCommandAction;
use yinz_types::input::layout::{PluginAlias, RunPlugin, RunPluginOrAlias, TabLayoutInfo};
use yinz_types::internal_ids::{TabNumber, TabPosition};

pub(super) fn new_tab(
    name: Option<String>,
    layout: Option<PathBuf>,
    layout_string: Option<String>,
    layout_dir: Option<PathBuf>,
    cwd: Option<PathBuf>,
    initial_command: Vec<String>,
    initial_plugin: Option<String>,
    close_on_exit: bool,
    start_suspended: bool,
    block_until_exit_success: bool,
    block_until_exit_failure: bool,
    block_until_exit: bool,
    no_focus: bool,
    get_current_dir: &dyn Fn() -> PathBuf,
    config: Option<Config>,
) -> Result<Vec<Action>, String> {
    let current_dir = get_current_dir();
    let cwd = cwd
        .map(|cwd| current_dir.join(cwd))
        .or_else(|| Some(current_dir.clone()));

    // Map CLI flags to UnblockCondition
    let first_pane_unblock_condition = if block_until_exit_success {
        Some(UnblockCondition::OnExitSuccess)
    } else if block_until_exit_failure {
        Some(UnblockCondition::OnExitFailure)
    } else if block_until_exit {
        Some(UnblockCondition::OnAnyExit)
    } else {
        None
    };

    // Parse initial_panes from initial_command or initial_plugin
    let initial_panes = if let Some(plugin_url) = initial_plugin {
        let plugin = match yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(
            &plugin_url,
            cwd.clone(),
        ) {
            Ok(run_plugin) => RunPluginOrAlias::RunPlugin(RunPlugin {
                initial_cwd: cwd.clone(),
                ..run_plugin
            }),
            Err(_) => {
                let mut plugin_alias = PluginAlias::new(&plugin_url, &None, cwd.clone());
                plugin_alias.set_caller_cwd_if_not_set(Some(current_dir.clone()));
                RunPluginOrAlias::Alias(plugin_alias)
            },
        };
        Some(vec![CommandOrPlugin::Plugin(plugin)])
    } else if !initial_command.is_empty() {
        let mut command: Vec<String> = initial_command.clone();
        let (command, args) = (
            PathBuf::from(command.remove(0)),
            command.into_iter().collect(),
        );
        let hold_on_close = !close_on_exit;
        let hold_on_start = start_suspended;
        let run_command_action = RunCommandAction {
            command,
            args,
            cwd: cwd.clone(),
            direction: None,
            hold_on_close,
            hold_on_start,
            ..Default::default()
        };
        Some(vec![CommandOrPlugin::Command(run_command_action)])
    } else {
        None
    };
    let layout = if let Some(raw_layout) = layout_string {
        yinz_config::layout::from_str(&raw_layout, "layout-string".to_owned(), None, cwd)
            .map_err(|e| layout_error_report(e, None))?
    } else if let Some(layout_path) = layout {
        let layout_dir = layout_dir
            .or_else(|| config.and_then(|c| c.options.layout_dir))
            .or_else(|| get_layout_dir(find_default_config_dir()));

        let layout_source_name = layout_path
            .as_path()
            .as_os_str()
            .to_string_lossy()
            .to_string();
        let (path_to_raw_layout, raw_layout, swap_layouts) =
            yinz_config::layout::stringified_from_path_or_default(Some(&layout_path), layout_dir)
                .map_err(|e| format!("Failed to load layout: {}", e))?;
        yinz_config::layout::from_str(
            &raw_layout,
            path_to_raw_layout,
            swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
            cwd,
        )
        .map_err(|e| layout_error_report(e, Some((layout_source_name, raw_layout))))?
    } else {
        let should_change_focus_to_new_tab = !no_focus;
        return Ok(vec![Action::NewTab {
            layout_source: None,
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: name,
            should_change_focus_to_new_tab,
            cwd,
            initial_panes,
            first_pane_unblock_condition,
        }]);
    };
    // The cwd is done through the layout.
    let mut tabs = layout.tabs();
    if !tabs.is_empty() {
        let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
        let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
        let mut new_tab_actions = vec![];
        let mut has_focused_tab = tabs
            .iter()
            .any(|(_, layout, _)| layout.focus.unwrap_or(false));
        for (tab_name, mut layout, floating_panes_layout) in tabs.drain(..) {
            // A `--name` filling in for a layout that gave none is the user's.
            layout.user_named |= tab_name.is_none() && name.is_some();
            let name = tab_name.or_else(|| name.clone());
            let should_change_focus_to_new_tab = !no_focus
                && layout.focus.unwrap_or_else(|| {
                    if !has_focused_tab {
                        has_focused_tab = true;
                        true
                    } else {
                        false
                    }
                });
            new_tab_actions.push(Action::NewTab {
                layout_source: None,
                tiled_layout: Some(layout),
                floating_layouts: floating_panes_layout,
                swap_tiled_layouts: swap_tiled_layouts.clone(),
                swap_floating_layouts: swap_floating_layouts.clone(),
                tab_name: name,
                should_change_focus_to_new_tab,
                cwd: None,
                initial_panes: initial_panes.clone(),
                first_pane_unblock_condition,
            });
        }
        Ok(new_tab_actions)
    } else {
        let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
        let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
        let (mut layout, floating_panes_layout) = layout.new_tab();
        layout.user_named |= name.is_some();
        let should_change_focus_to_new_tab = !no_focus;
        Ok(vec![Action::NewTab {
            layout_source: None,
            tiled_layout: Some(layout),
            floating_layouts: floating_panes_layout,
            swap_tiled_layouts,
            swap_floating_layouts,
            tab_name: name,
            should_change_focus_to_new_tab,
            cwd: None,
            initial_panes,
            first_pane_unblock_condition,
        }])
    }
}

/// A layout that failed to parse, as the CLI prints it.
///
/// `from_kdl` has already turned KDL deserialization errors into `KdlError`s that carry the
/// layout's source and the hints for the usual mistakes; `source` only renames that source to
/// what the user typed.
fn layout_error_report(error: ConfigError, source: Option<(String, String)>) -> String {
    match error {
        ConfigError::KdlError(kdl_error) => {
            let kdl_error = match source {
                Some((name, raw_layout)) => kdl_error.add_src(name, raw_layout),
                None => kdl_error,
            };
            let report: Report = kdl_error.into();
            format!("{:?}", report)
        },
        error => format!("{}", error),
    }
}

pub(super) fn override_layout(
    layout: Option<PathBuf>,
    layout_string: Option<String>,
    layout_dir: Option<PathBuf>,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    apply_only_to_active_tab: bool,
    config: Option<Config>,
) -> Result<Vec<Action>, String> {
    // Determine layout_dir: CLI arg > config > default
    let layout_dir = layout_dir
        .or_else(|| config.and_then(|c| c.options.layout_dir))
        .or_else(|| get_layout_dir(find_default_config_dir()));

    // Load layout from string or file path
    let layout_source_name;
    let (path_to_raw_layout, raw_layout, swap_layouts) = if let Some(raw) = layout_string {
        layout_source_name = "layout-string".to_owned();
        (layout_source_name.clone(), raw, None)
    } else if let Some(layout_path) = &layout {
        layout_source_name = layout_path
            .as_path()
            .as_os_str()
            .to_string_lossy()
            .to_string();
        yinz_config::layout::stringified_from_path_or_default(Some(layout_path), layout_dir)
            .map_err(|e| format!("Failed to load layout: {}", e))?
    } else {
        return Err("Either layout or layout-string must be provided".to_string());
    };

    // Parse KDL layout
    let layout = yinz_config::layout::from_str(
        &raw_layout,
        path_to_raw_layout,
        swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
        None, // cwd
    )
    .map_err(|e| layout_error_report(e, Some((layout_source_name, raw_layout))))?;

    // Convert all tabs to Vec<TabLayoutInfo>
    let tabs: Vec<TabLayoutInfo> = layout
        .tabs
        .iter()
        .enumerate()
        .map(|(index, (tab_name, tiled, floating))| TabLayoutInfo {
            tab_index: TabPosition::new(index),
            tab_name: tab_name.clone(),
            tiled_layout: tiled.clone(),
            floating_layouts: floating.clone(),
            swap_tiled_layouts: Some(layout.swap_tiled_layouts.clone()),
            swap_floating_layouts: Some(layout.swap_floating_layouts.clone()),
        })
        .collect();

    // If no tabs, create default tab
    let tabs = if tabs.is_empty() {
        let (tiled, floating) = layout.new_tab();
        vec![TabLayoutInfo {
            tab_index: TabPosition::new(0),
            tab_name: None,
            tiled_layout: tiled,
            floating_layouts: floating,
            swap_tiled_layouts: Some(layout.swap_tiled_layouts),
            swap_floating_layouts: Some(layout.swap_floating_layouts),
        }]
    } else {
        tabs
    };

    Ok(vec![Action::OverrideLayout {
        tabs,
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        apply_only_to_active_tab,
    }])
}

pub(super) fn switch_session(
    name: String,
    tab_position: Option<TabNumber>,
    pane_id: Option<String>,
    layout: Option<PathBuf>,
    layout_string: Option<String>,
    layout_dir: Option<PathBuf>,
    cwd: Option<PathBuf>,
    get_current_dir: &dyn Fn() -> PathBuf,
    config: Option<Config>,
) -> Result<Vec<Action>, String> {
    let pane_id = match pane_id {
        Some(stringified_pane_id) => match PaneId::from_str(&stringified_pane_id) {
            Ok(pane_id) => Some(pane_id),
            Err(_e) => {
                return Err(format!(
                        "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                        stringified_pane_id
                    ));
            },
        },
        None => None,
    };

    let cwd = cwd.map(|cwd| {
        let current_dir = get_current_dir();
        current_dir.join(cwd)
    });

    let layout_dir = layout_dir.map(|layout_dir| {
        let current_dir = get_current_dir();
        current_dir.join(layout_dir)
    });

    let layout_info = if let Some(layout_string) = layout_string {
        // validate the layout string before sending it to the target session
        yinz_config::layout::from_str(&layout_string, "layout-string".to_owned(), None, None)
            .map_err(|e| layout_error_report(e, None))?;
        Some(LayoutInfo::Stringified(layout_string))
    } else if let Some(layout_path) = layout {
        let layout_dir = layout_dir
            .or_else(|| config.and_then(|c| c.options.layout_dir.clone()))
            .or_else(|| get_layout_dir(find_default_config_dir()));
        // validate the layout file before sending it to the target session
        yinz_config::layout::from_path_or_default_without_config(
            Some(&layout_path),
            layout_dir.clone(),
        )
        .map_err(|e| layout_error_report(e, None))?;
        yinz_config::layout::layout_info_from_config(&layout_dir, &Some(layout_path))
    } else {
        None
    };

    Ok(vec![Action::SwitchSession {
        name: name.clone().into(),
        tab_position,
        pane_id,
        layout: layout_info,
        cwd,
    }])
}
