//! Runtime-neutral multipane commands. Held terminals reserve launch slots without spawning
//! a process; the existing launch paths then replace or start those slots in place.
use super::*;
use yinz_types::multipane::*;

fn screen(env: &PluginEnv, operation: MultipaneOperation) -> Result<Option<MultipaneInfo>> {
    let (response, receive) = crossbeam::channel::bounded(1);
    env.senders.send_to_screen(ScreenInstruction::Multipane {
        operation,
        client_id: env.client_id,
        response,
    })?;
    receive
        .recv()
        .context("multipane screen response disconnected")?
        .map_err(|error| anyhow!(error))
}
fn validate_launch(env: &PluginEnv, child: &MultipaneChild) -> Result<()> {
    match child {
        MultipaneChild::Plugin { url, configuration } => {
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                url,
                &Some(configuration.clone()),
                None,
                Some(env.plugin_cwd.clone()),
            )
            .map_err(|error| anyhow!(error))?;
        },
        MultipaneChild::Terminal(command)
            if command.path.as_os_str().is_empty() && !command.remote_default_shell =>
        {
            return Err(anyhow!("multipane command cannot be empty"))
        },
        _ => {},
    }
    Ok(())
}
fn reserve(env: &PluginEnv) -> Result<PaneId> {
    let command = RunCommand {
        command: env.path_to_default_shell.clone(),
        hold_on_start: true,
        hold_on_close: true,
        ..Default::default()
    };
    let (send, receive) = oneshot::channel();
    env.senders.send_to_pty(PtyInstruction::SpawnTerminal(
        Some(TerminalAction::RunCommand(command)),
        Some("Loading multipane child".into()),
        NewPanePlacement::Floating(None),
        true,
        ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
        Some(NotificationEnd::new(send)),
        false,
    ))?;
    wait_for_action_completion(receive, "reserve multipane child", false)
        .affected_pane_id
        .context("failed to reserve multipane child")
}
fn launch(env: &PluginEnv, slot: PaneId, child: MultipaneChild, reserved: bool) -> Result<()> {
    match child {
        MultipaneChild::Existing(_) => {},
        MultipaneChild::Terminal(command) => {
            if reserved {
                run_command_in_pane(env, slot, command, BTreeMap::new(), false);
            } else {
                let run = run_command_action(env, command, true, BTreeMap::new())
                    .context("cannot prepare multipane command")?;
                let (send, receive) = oneshot::channel();
                env.senders
                    .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
                        Some(TerminalAction::RunCommand(run.into())),
                        None,
                        true,
                        ClientTabIndexOrPaneId::PaneId(slot),
                        Some(NotificationEnd::new(send)),
                    ))?;
                wait_for_action_completion(receive, "replace multipane terminal", false)
                    .affected_pane_id
                    .context("failed to replace multipane terminal")?;
            }
        },
        MultipaneChild::Plugin { url, configuration } => {
            let plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                &url,
                &Some(configuration),
                None,
                Some(env.plugin_cwd.clone()),
            )
            .map_err(|error| anyhow!(error))?;
            env.senders.send_to_plugin(PluginInstruction::Load(
                Some(true),
                true,
                true,
                None,
                plugin,
                None,
                Some(slot),
                env.client_id,
                yinz_types::pane_size::Size { rows: 1, cols: 1 },
                Some(env.plugin_cwd.clone()),
                None,
                false,
                Some(false),
                None,
                None,
                None,
            ))?;
            // Loading takes the plugin map and running-plugin locks. Return ownership now;
            // waiting here while holding the caller's lock would deadlock the loader.
        },
    }
    Ok(())
}
fn create(env: &PluginEnv, request: CreateMultipane) -> Result<Option<MultipaneInfo>> {
    request
        .placement
        .validate()
        .map_err(|error| anyhow!(error))?;
    request.split.validate().map_err(|error| anyhow!(error))?;
    if request.focus.is_some_and(|slot| slot >= 2) {
        return Err(anyhow!("invalid multipane focus slot"));
    }
    for child in &request.children {
        validate_launch(env, child)?;
    }
    let existing = request
        .children
        .iter()
        .filter_map(|child| match child {
            MultipaneChild::Existing(id) => Some(*id),
            _ => None,
        })
        .collect();
    screen(env, MultipaneOperation::Validate(existing))?;
    let mut reserved = vec![];
    let result = (|| {
        let mut slots = Vec::with_capacity(2);
        for child in &request.children {
            slots.push(match child {
                MultipaneChild::Existing(id) => *id,
                _ => {
                    let id = reserve(env)?;
                    reserved.push(id);
                    id
                },
            });
        }
        screen(
            env,
            MultipaneOperation::Create {
                children: [slots[0], slots[1]],
                placement: request.placement,
                split: request.split,
                focus: request.focus,
                min_cells: request.min_cells,
            },
        )
    })();
    let info = match result {
        Ok(Some(info)) => info,
        other => {
            for id in reserved {
                env.senders
                    .send_to_screen(ScreenInstruction::ClosePane(
                        id,
                        Some(env.client_id),
                        None,
                        None,
                    ))
                    .non_fatal();
            }
            return other;
        },
    };
    // Both slots now belong to the container. Loading, permission and error panes replace a
    // slot through the ordinary in-place launch path and cannot disturb its sibling.
    for (slot, child) in info.children.into_iter().zip(request.children) {
        if let Err(error) = launch(env, slot, child, reserved.contains(&slot)) {
            env.senders
                .send_to_screen(ScreenInstruction::RenamePane(
                    slot,
                    format!("Launch failed: {error}").into_bytes(),
                    None,
                ))
                .non_fatal();
        }
    }
    screen(env, MultipaneOperation::Get(info.id))
}
pub(super) fn dispatch(env: &mut PluginEnv, command: PluginCommand) -> Result<()> {
    let result = match command {
        PluginCommand::CreateMultipane(request) => create(env, request),
        PluginCommand::UpdateMultipane(request) => screen(env, MultipaneOperation::Update(request)),
        PluginCommand::ReplaceMultipaneChild(request) => (|| {
            validate_launch(env, &request.child)?;
            if request.slot >= 2 {
                return Err(anyhow!("invalid multipane child slot"));
            }
            let info =
                screen(env, MultipaneOperation::Get(request.id))?.context("multipane not found")?;
            match request.child {
                MultipaneChild::Existing(child) => screen(
                    env,
                    MultipaneOperation::Replace {
                        id: request.id,
                        slot: request.slot,
                        child,
                    },
                ),
                child => {
                    launch(env, info.children[request.slot], child, false)?;
                    screen(env, MultipaneOperation::Get(request.id))
                },
            }
        })(),
        _ => return Err(anyhow!("not a multipane command")),
    }
    .map_err(|error| error.to_string());
    let response = yinz_plugin_api::plugin_command::ProtobufMultipaneResponse {
        json: serde_json::to_string(&result)?,
    };
    write_response(env, &response.encode_to_vec())
}
