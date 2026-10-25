//! Terminal and plugin pane construction from CLI options.
use super::{parse_pane_id, select_configuration};
use std::path::PathBuf;
use yinz_types::data::{NewPanePlacement, UnblockCondition};
use yinz_types::geometry::Direction;
use yinz_types::input::actions::Action;
use yinz_types::input::command::RunCommandAction;
use yinz_types::input::layout::{
    PluginAlias, PluginUserConfiguration, RunPlugin, RunPluginOrAlias,
};
use yinz_types::internal_ids::TabId;

pub(super) fn new_pane(
    direction: Option<Direction>,
    command: Vec<String>,
    remote_host: Option<String>,
    plugin: Option<String>,
    cwd: Option<PathBuf>,
    floating: bool,
    in_place: bool,
    close_replaced_pane: bool,
    pane_id: Option<String>,
    name: Option<String>,
    close_on_exit: bool,
    start_suspended: bool,
    configuration: Option<PluginUserConfiguration>,
    configuration_json: Option<PluginUserConfiguration>,
    skip_plugin_cache: bool,
    x: Option<String>,
    y: Option<String>,
    width: Option<String>,
    height: Option<String>,
    pinned: Option<bool>,
    stacked: bool,
    blocking: bool,
    block_until_exit_success: bool,
    block_until_exit_failure: bool,
    block_until_exit: bool,
    unblock_condition: Option<UnblockCondition>,
    near_current_pane: bool,
    no_focus: bool,
    borderless: Option<bool>,
    overlay: Option<bool>,
    modal: Option<bool>,
    tab_id: Option<TabId>,
    get_current_dir: &dyn Fn() -> PathBuf,
) -> Result<Vec<Action>, String> {
    let configuration = select_configuration(configuration, configuration_json)?;
    let target = remote_host
        .as_deref()
        .map(yinz_types::command::PaneTarget::ssh)
        .transpose()?
        .unwrap_or_default();
    let pane_id_to_replace = pane_id.as_deref().map(parse_pane_id).transpose()?;
    let current_dir = get_current_dir();
    // cwd should only be specified in a plugin alias if it was explicitly given to us,
    // otherwise the current_dir might override a cwd defined in the alias itself
    let alias_cwd = cwd.clone().map(|cwd| current_dir.join(cwd));
    // A remote pane's cwd names a directory on the *other* machine,
    // so this machine's working directory is neither a sensible
    // default for it nor a sensible base to resolve a relative path
    // against. Left exactly as given: `None` then means "wherever
    // that machine logs you in", which is what ssh does.
    let command_cwd = if target.is_remote() {
        cwd.clone()
    } else {
        cwd.clone()
            .map(|cwd| current_dir.join(cwd))
            .or_else(|| Some(current_dir.clone()))
    };
    let cwd = cwd
        .map(|cwd| current_dir.join(cwd))
        .or_else(|| Some(current_dir.clone()));
    // One construction for both the blocking and the ordinary pane,
    // so that "no command, but a host" means the same thing in each.
    let terminal_run_command = if !command.is_empty() {
        let mut command = command.clone();
        let (command, args) = (PathBuf::from(command.remove(0)), command);
        Some(RunCommandAction {
            command,
            args,
            cwd: command_cwd,
            direction,
            hold_on_close: !close_on_exit,
            hold_on_start: start_suspended,
            target: target.clone(),
            ..Default::default()
        })
    } else if target.is_remote() {
        // No command and a host: that machine's login shell. Not
        // held on close — `ssh host` ends when you log out, and so
        // does a local default-shell pane; holding it would leave a
        // "Waiting to run" banner every time.
        Some(RunCommandAction {
            command: PathBuf::new(),
            args: vec![],
            cwd: command_cwd,
            direction,
            hold_on_close: false,
            hold_on_start: false,
            target: target.clone(),
            remote_default_shell: true,
            ..Default::default()
        })
    } else {
        None
    };
    let unblock_condition = unblock_condition.or({
        if block_until_exit_success {
            Some(UnblockCondition::OnExitSuccess)
        } else if block_until_exit_failure {
            Some(UnblockCondition::OnExitFailure)
        } else if block_until_exit {
            Some(UnblockCondition::OnAnyExit)
        } else {
            None
        }
    });
    if blocking || unblock_condition.is_some() {
        // For blocking panes, we don't support plugins
        if plugin.is_some() {
            return Err("Blocking panes do not support plugin variants".to_string());
        }

        let command = terminal_run_command;

        let placement = if floating {
            NewPanePlacement::floating_or_overlay(
                x, y, width, height, pinned, borderless, overlay, modal,
            )
        } else if in_place {
            NewPanePlacement::InPlace {
                pane_id_to_replace,
                close_replaced_pane,
                borderless,
            }
        } else if stacked {
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless,
            }
        } else {
            NewPanePlacement::Tiled {
                direction,
                borderless,
            }
        };

        Ok(vec![Action::NewBlockingPane {
            placement,
            pane_name: name,
            command,
            unblock_condition,
            near_current_pane,
            no_focus,
            tab_id,
        }])
    } else if let Some(plugin) = plugin {
        let plugin =
            match yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(&plugin, cwd.clone())
            {
                Ok(run_plugin) => {
                    let user_configuration = configuration.unwrap_or_default();
                    RunPluginOrAlias::RunPlugin(RunPlugin {
                        configuration: user_configuration,
                        initial_cwd: cwd.clone(),
                        ..run_plugin
                    })
                },
                Err(_) => {
                    let mut plugin_alias = PluginAlias::new(
                        &plugin,
                        &configuration.map(PluginUserConfiguration::into_inner),
                        alias_cwd,
                    );
                    plugin_alias.set_caller_cwd_if_not_set(Some(current_dir));
                    RunPluginOrAlias::Alias(plugin_alias)
                },
            };
        if floating {
            Ok(vec![Action::NewFloatingPluginPane {
                plugin,
                pane_name: name,
                skip_cache: skip_plugin_cache,
                cwd,
                placement: NewPanePlacement::floating_or_overlay(
                    x, y, width, height, pinned, borderless, overlay, modal,
                ),
                no_focus,
                tab_id,
            }])
        } else if in_place {
            Ok(vec![Action::NewInPlacePluginPane {
                plugin,
                pane_name: name,
                skip_cache: skip_plugin_cache,
                close_replaced_pane,
                no_focus,
                tab_id,
            }])
        } else {
            // it is intentional that a new tiled plugin pane cannot include a
            // direction
            // this is because the cli client opening a tiled plugin pane is a
            // different client than the one opening the pane, and this can potentially
            // create very confusing races if the client changes focus while the plugin
            // is being loaded
            // this is not the case with terminal panes for historical reasons of
            // backwards compatibility to a time before we had auto layouts
            Ok(vec![Action::NewTiledPluginPane {
                plugin,
                pane_name: name,
                skip_cache: skip_plugin_cache,
                cwd,
                no_focus,
                tab_id,
            }])
        }
    } else if let Some(run_command_action) = terminal_run_command {
        if floating {
            Ok(vec![Action::NewFloatingPane {
                command: Some(run_command_action),
                pane_name: name,
                placement: NewPanePlacement::floating_or_overlay(
                    x, y, width, height, pinned, borderless, overlay, modal,
                ),
                near_current_pane,
                no_focus,
                tab_id,
            }])
        } else if in_place {
            Ok(vec![Action::NewInPlacePane {
                command: Some(run_command_action),
                pane_name: name,
                near_current_pane,
                no_focus,
                pane_id_to_replace,
                close_replaced_pane,
                tab_id,
            }])
        } else if stacked {
            Ok(vec![Action::NewStackedPane {
                command: Some(run_command_action),
                pane_name: name,
                near_current_pane,
                no_focus,
                tab_id,
            }])
        } else {
            Ok(vec![Action::NewTiledPane {
                direction,
                command: Some(run_command_action),
                pane_name: name,
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
            }])
        }
    } else {
        if floating {
            Ok(vec![Action::NewFloatingPane {
                command: None,
                pane_name: name,
                placement: NewPanePlacement::floating_or_overlay(
                    x, y, width, height, pinned, borderless, overlay, modal,
                ),
                near_current_pane,
                no_focus,
                tab_id,
            }])
        } else if in_place {
            Ok(vec![Action::NewInPlacePane {
                command: None,
                pane_name: name,
                near_current_pane,
                no_focus,
                pane_id_to_replace,
                close_replaced_pane,
                tab_id,
            }])
        } else if stacked {
            Ok(vec![Action::NewStackedPane {
                command: None,
                pane_name: name,
                near_current_pane,
                no_focus,
                tab_id,
            }])
        } else {
            Ok(vec![Action::NewTiledPane {
                direction,
                command: None,
                pane_name: name,
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
            }])
        }
    }
}
