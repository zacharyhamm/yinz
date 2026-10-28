use super::agent_launch::{
    agent_folder, apply_project_worktree_dir, branch_exists, linked_worktree, parse_worktrees,
    prepare_git_checkout_in, run_git, PreparedCheckout, WorktreeGuard,
};
use super::filesystem::{resolve_fs_path_with_permission, FsIntent, FsPathResolution};
use super::layouts::{
    layout_run_permissions, try_rename_layout, try_save_layout, validate_plugin_layout_path,
};
use super::permissions::{check_command_permission, command_permission, LAYOUT_FILE_PERMISSIONS};
use super::run_plugin_command;
use crate::plugin_map::PluginEnv;
use crate::PaneId;
use prost::Message;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::{fs, process};
use yinz_bus::ServerInstruction;
use yinz_plugin_api::plugin_command::{
    fs_mutate_response, ProtobufDeleteAllDeadSessionsResponse, ProtobufDeleteDeadSessionResponse,
    ProtobufFsMutateResponse, ProtobufKillSessionsResponse,
};
use yinz_runtime::errors::prelude::*;
use yinz_storage::plugin_data_store;
use yinz_types::agent::AgentSpawnSpec;
use yinz_types::command::{CommandToRun, GitCheckout};
use yinz_types::data::{
    CommandType, DataError, FetchRequest, OverlayPlacement, PermissionStatus, PermissionType,
    PluginCommand,
};
use yinz_types::input::command::RunCommand;
use yinz_types::input::layout::{Layout, Run, RunPluginOrAlias};
use yinz_types::internal_ids::{PluginId, TerminalId};

#[test]
fn provider_usage_requires_agent_control_permission() {
    let env = crate::test_env::test_env();
    let command = PluginCommand::AgentControl {
        id: "usage".into(),
        request: serde_json::to_string(&yinz_types::agent::control::AgentRequest::Usage {
            provider: "unsupported-fixture".into(),
        })
        .unwrap(),
    };
    assert_eq!(
        check_command_permission(&env, &command),
        (PermissionStatus::Denied, Some(PermissionType::RunCommands))
    );
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::RunCommands]));
    assert_eq!(
        check_command_permission(&env, &command),
        (PermissionStatus::Granted, None)
    );
}

#[test]
fn plugin_info_self_inspection_needs_no_permission() {
    let env = crate::test_env::test_env();
    for id in [None, Some(env.plugin_id)] {
        assert_eq!(
            check_command_permission(&env, &PluginCommand::GetPluginInfo(id)),
            (PermissionStatus::Granted, None)
        );
    }
    assert_eq!(
        check_command_permission(
            &env,
            &PluginCommand::GetPluginInfo(Some(PluginId::new(env.plugin_id.get() + 1)))
        ),
        (
            PermissionStatus::Denied,
            Some(PermissionType::ReadApplicationState)
        )
    );
}

#[test]
fn plugin_url_query_denial_is_an_error_not_an_empty_match() {
    let mut env = crate::test_env::test_env();
    env.response_sink = None;
    let command = PluginCommand::GetPluginIdsForUrl("file:/plugin.scm".into());
    assert_eq!(
        command_permission(CommandType::GetPluginIdsForUrl),
        Some(PermissionType::ReadApplicationState)
    );
    run_plugin_command(&mut env, command).unwrap();
    let bytes: Vec<u8> = env.response_sink.take().unwrap();
    let reply = yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse::decode(
        bytes.as_slice(),
    )
    .unwrap();
    assert_eq!(
        reply.error.as_deref(),
        Some("Permission denied: ReadApplicationState")
    );
    assert!(
        crate::response::decode_response(CommandType::GetPluginIdsForUrl, &bytes)
            .unwrap()
            .is_err()
    );
}

#[test]
fn a_missing_plugin_root_under_a_symlink_does_not_block_data_writes() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    fs::create_dir(&config).unwrap();
    let alias = temp.path().join("config-link");
    std::os::unix::fs::symlink(&config, &alias).unwrap();
    let plugin_dir = alias.join("plugins");
    let mut env = crate::test_env::test_env();
    env.plugin_own_data_dir = temp.path().to_owned();
    env.user_plugin_dir = Some(plugin_dir.clone());
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::WriteFiles]));
    env.response_sink = None;

    run_plugin_command(
        &mut env,
        PluginCommand::FsWrite {
            path: "/data/output.txt".into(),
            contents: b"output".to_vec(),
            append: false,
            create_dirs: false,
        },
    )
    .unwrap();
    let bytes: Vec<u8> = env.response_sink.take().unwrap();
    assert_eq!(
        ProtobufFsMutateResponse::decode(bytes.as_slice())
            .unwrap()
            .result,
        Some(fs_mutate_response::Result::Success(true))
    );
    assert_eq!(fs::read(temp.path().join("output.txt")).unwrap(), b"output");
    assert!(
        !plugin_dir.exists(),
        "an unrelated write must not create /plugins"
    );
}

#[test]
fn a_missing_plugin_root_under_a_symlink_still_requires_authoring_permission() {
    for suffix in ["plugins", "missing/nested/plugins"] {
        for prefix in [
            "/plugins".to_owned(),
            format!("/host/config-link/{suffix}"),
            format!("/host/config/{suffix}"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let config = temp.path().join("config");
            fs::create_dir(&config).unwrap();
            let alias = temp.path().join("config-link");
            std::os::unix::fs::symlink(&config, &alias).unwrap();
            let plugin_dir = alias.join(suffix);
            let mut env = crate::test_env::test_env();
            env.plugin_cwd = temp.path().to_owned();
            env.user_plugin_dir = Some(plugin_dir.clone());
            *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::WriteFiles]));
            env.response_sink = None;
            let path = PathBuf::from(prefix).join("new/nested/plugin.scm");

            for resolution in [
                FsPathResolution::FollowSymlink,
                FsPathResolution::PreserveEntry,
            ] {
                let error =
                    resolve_fs_path_with_permission(&env, &path, FsIntent::Write, resolution)
                        .unwrap_err();
                assert!(
                    error.to_string().contains("AuthorPlugins"),
                    "{path:?}: {error:#}"
                );
            }
            for authorized in [false, true] {
                if authorized {
                    env.permissions
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .insert(PermissionType::AuthorPlugins);
                }
                run_plugin_command(
                    &mut env,
                    PluginCommand::FsWrite {
                        path: path.clone(),
                        contents: b"export {};".to_vec(),
                        append: false,
                        create_dirs: true,
                    },
                )
                .unwrap();
                let bytes: Vec<u8> = env.response_sink.take().unwrap();
                let result = ProtobufFsMutateResponse::decode(bytes.as_slice())
                    .unwrap()
                    .result;
                if authorized {
                    assert_eq!(
                        result,
                        Some(fs_mutate_response::Result::Success(true)),
                        "{path:?}"
                    );
                    assert_eq!(
                        fs::read(config.join(suffix).join("new/nested/plugin.scm")).unwrap(),
                        b"export {};"
                    );
                } else {
                    assert!(
                        matches!(result, Some(fs_mutate_response::Result::Error(error)) if error.contains("AuthorPlugins")),
                        "{path:?}"
                    );
                    assert!(
                        !plugin_dir.exists(),
                        "a denied write must not create /plugins"
                    );
                }
            }
        }
    }
}

#[test]
fn authoring_permission_follows_plugin_directory_aliases_and_ancestors() {
    let temp = tempfile::tempdir().unwrap();
    let plugin_dir = temp.path().join("plugins");
    fs::create_dir(&plugin_dir).unwrap();
    std::os::unix::fs::symlink(&plugin_dir, temp.path().join("alias")).unwrap();
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = temp.path().to_owned();
    env.user_plugin_dir = Some(plugin_dir);
    for path in [
        "/plugins/new.scm",
        "/host/plugins/new.scm",
        "/host/alias/new.scm",
        "plugins/new.scm",
        "/host/plugins",
    ] {
        for resolution in [
            FsPathResolution::FollowSymlink,
            FsPathResolution::PreserveEntry,
        ] {
            assert!(
                resolve_fs_path_with_permission(&env, Path::new(path), FsIntent::Write, resolution)
                    .unwrap_err()
                    .to_string()
                    .contains("AuthorPlugins"),
                "{path}"
            );
            assert!(
                resolve_fs_path_with_permission(&env, Path::new(path), FsIntent::Read, resolution)
                    .is_ok(),
                "{path}"
            );
        }
    }
    assert!(resolve_fs_path_with_permission(
        &env,
        Path::new("/host/unrelated"),
        FsIntent::Write,
        FsPathResolution::FollowSymlink
    )
    .is_ok());
    assert!(resolve_fs_path_with_permission(
        &env,
        Path::new("/host"),
        FsIntent::Write,
        FsPathResolution::PreserveEntry
    )
    .unwrap_err()
    .to_string()
    .contains("AuthorPlugins"));
    // Ensuring an existing ancestor directory is harmless; only removing or renaming
    // that entry changes the plugin folder below it.
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::WriteFiles]));
    env.response_sink = None;
    run_plugin_command(
        &mut env,
        PluginCommand::FsCreateDir {
            path: PathBuf::from("/host"),
            all: true,
        },
    )
    .unwrap();
    let bytes: Vec<u8> = env.response_sink.take().unwrap();
    assert!(matches!(
        ProtobufFsMutateResponse::decode(bytes.as_slice())
            .unwrap()
            .result,
        Some(fs_mutate_response::Result::Success(true))
    ));
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::AuthorPlugins]));
    assert!(resolve_fs_path_with_permission(
        &env,
        Path::new("/host/plugins/new.scm"),
        FsIntent::Write,
        FsPathResolution::FollowSymlink
    )
    .is_ok());
}

#[test]
fn a_folder_holding_a_root_cannot_be_removed_or_renamed() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("share/yinz/data");
    fs::create_dir_all(&data).unwrap();
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = temp.path().to_owned();
    env.plugin_own_data_dir = data;
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::WriteFiles]));
    for path in ["/host/share", "/host/share/yinz", "/data"] {
        let error = resolve_fs_path_with_permission(
            &env,
            Path::new(path),
            FsIntent::Write,
            FsPathResolution::PreserveEntry,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("cannot be removed or replaced"),
            "{path}: {error:#}"
        );
    }
    // Writing inside the folder, or removing a sibling, is untouched.
    fs::create_dir(temp.path().join("other")).unwrap();
    for (path, resolution) in [
        ("/host/share/new.txt", FsPathResolution::FollowSymlink),
        ("/host/other", FsPathResolution::PreserveEntry),
    ] {
        assert!(
            resolve_fs_path_with_permission(&env, Path::new(path), FsIntent::Write, resolution)
                .is_ok(),
            "{path}"
        );
    }
}

#[test]
fn disconnect_target_queries_isolate_late_replies_and_report_failures() {
    use yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse;
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::internal_ids::ClientId;
    let mut env = crate::test_env::test_env();
    env.response_sink = None;
    let (sender, server) = unbounded();
    env.senders.to_server = Some(SenderWithContext::new(sender));
    let read = |env: &mut PluginEnv| {
        let bytes: Vec<u8> = env.response_sink.take().unwrap();
        ProtobufGetDisconnectTargetsResponse::decode(bytes.as_slice()).unwrap()
    };
    assert_eq!(
        check_command_permission(&env, &PluginCommand::GetDisconnectTargets),
        (
            PermissionStatus::Denied,
            Some(PermissionType::ReadApplicationState)
        )
    );
    run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
    assert!(read(&mut env).error.unwrap().contains("Permission denied"));
    assert!(server.try_recv().is_err());
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::ReadApplicationState]));
    run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
    assert!(read(&mut env)
        .error
        .unwrap()
        .contains("receive disconnect targets"));
    let ServerInstruction::GetDisconnectTargets(_, stale) = server.recv().unwrap().0 else {
        panic!()
    };
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let ServerInstruction::GetDisconnectTargets(_, fresh) = server.recv().unwrap().0 else {
                panic!()
            };
            assert!(stale.send(vec![ClientId::new(9)]).is_err());
            fresh
                .send(vec![ClientId::new(2), ClientId::new(3)])
                .unwrap();
        });
        run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
    });
    let response = read(&mut env);
    assert_eq!(response.client_ids, vec![2, 3]);
    assert!(response.error.is_none());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = server.recv().unwrap();
        });
        run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
    });
    assert!(read(&mut env).error.unwrap().contains("disconnected"));
    drop(server);
    env.senders.should_silently_fail = false;
    run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
    assert!(read(&mut env)
        .error
        .unwrap()
        .contains("send disconnect-target query"));
}

#[test]
fn denied_session_commands_reply_with_permission_errors() {
    let mut env = crate::test_env::test_env();
    env.response_sink = None;
    for command in [
        PluginCommand::KillSessionsAndReply(vec!["test-session".into()]),
        PluginCommand::DeleteDeadSessionAndReply("test-session".into()),
        PluginCommand::DeleteAllDeadSessionsAndReply,
    ] {
        let kind = CommandType::from(&command);
        assert_eq!(
            check_command_permission(&env, &command),
            (
                PermissionStatus::Denied,
                Some(PermissionType::ChangeApplicationState)
            )
        );
        run_plugin_command(&mut env, command).unwrap();
        let bytes: Vec<u8> = env.response_sink.take().unwrap();
        let error = match kind {
            CommandType::KillSessionsAndReply => {
                ProtobufKillSessionsResponse::decode(bytes.as_slice())
                    .unwrap()
                    .error
            },
            CommandType::DeleteDeadSessionAndReply => {
                ProtobufDeleteDeadSessionResponse::decode(bytes.as_slice())
                    .unwrap()
                    .error
            },
            CommandType::DeleteAllDeadSessionsAndReply => {
                ProtobufDeleteAllDeadSessionsResponse::decode(bytes.as_slice())
                    .unwrap()
                    .error
            },
            _ => unreachable!(),
        };
        assert_eq!(
            error.as_deref(),
            Some("Permission denied: ChangeApplicationState"),
            "{kind}"
        );
    }
}

#[test]
fn other_denied_commands_keep_their_empty_reply() {
    let mut env = crate::test_env::test_env();
    env.response_sink = None;
    run_plugin_command(&mut env, PluginCommand::GetSessionList).unwrap();
    let bytes: Vec<u8> = env.response_sink.take().unwrap();
    assert!(bytes.is_empty());
}

/// `prepare_git_checkout_in` asking for reuse, which is what every test written before
/// reuse became a request meant by it: an existing branch or worktree is the work being
/// pointed at. The tests whose subject *is* the other behaviour call the real one with
/// `false` and say so.
fn reusing_checkout_in(
    source_directory: PathBuf,
    checkout: GitCheckout,
) -> Result<PathBuf, String> {
    prepare_git_checkout_in(source_directory, checkout, true, WorktreeGuard::Waived)
        .map(|prepared| prepared.path)
}

/// The same call without reuse: a name already taken is stepped around rather than
/// joined. The guard is waived here as it is above - the tests whose subject *is* the
/// guard pass `WorktreeGuard::Required` themselves.
fn cutting_checkout_in(
    source_directory: PathBuf,
    checkout: GitCheckout,
) -> Result<PreparedCheckout, String> {
    prepare_git_checkout_in(source_directory, checkout, false, WorktreeGuard::Waived)
}

/// `agent_folder` likewise, answering the folder alone.
fn agent_folder_reusing(
    checkout: GitCheckout,
    cwd: Option<PathBuf>,
    source: PathBuf,
) -> Result<PathBuf, String> {
    agent_folder(checkout, cwd, source, true, WorktreeGuard::Waived).map(|prepared| prepared.path)
}

fn repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["commit", "--allow-empty", "-q", "-m", "initial"],
    ] {
        assert!(process::Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    temp
}

/// Every command that runs without a permission, pinned so a reviewer sees the list change —
/// in either direction: a command gated here must not be ungated in `command_permission`,
/// and every command not listed here must need something.
#[test]
fn the_commands_that_need_no_permission_are_exactly_these() {
    use strum::IntoEnumIterator;
    let ungated = [
        CommandType::ResolveNotification,
        CommandType::RequestPluginPermissions,
        CommandType::Subscribe,
        CommandType::Unsubscribe,
        CommandType::SetSelectable,
        CommandType::SetSelfMouseSelectionSupport,
        CommandType::ShowCursor,
        CommandType::HideSelf,
        CommandType::ShowSelf,
        CommandType::CloseSelf,
        CommandType::SetTimeout,
        CommandType::ReportPanic,
        CommandType::GetCellPixelSize,
        CommandType::GetPluginIds,
        CommandType::GetYinzVersion,
        CommandType::PostMessageTo,
        CommandType::PostMessageToPlugin,
        CommandType::ScanHostFolder,
        CommandType::WatchFilesystem,
        CommandType::GetLocalUtcOffset,
        CommandType::GetLocalTime,
    ];
    for kind in CommandType::iter() {
        assert_eq!(
            command_permission(kind).is_none(),
            ungated.contains(&kind),
            "{kind}"
        );
    }
}

/// `MessageToPlugin` reaches another plugin; `PostMessageToPlugin` only ever reaches the
/// sender itself, so a plugin's workers can answer it without a permission.
#[test]
fn a_message_to_another_plugin_needs_a_permission_but_a_message_to_self_does_not() {
    assert_eq!(
        command_permission(CommandType::MessageToPlugin),
        Some(PermissionType::MessageAndLaunchOtherPlugins)
    );
    assert_eq!(command_permission(CommandType::PostMessageToPlugin), None);
}

#[test]
fn pane_tabs_mount_requires_application_state_and_plugin_launch_permissions() {
    use yinz_types::pane_tabs::*;
    let env = crate::test_env::test_env();
    let command = PluginCommand::PaneTabs(PaneTabsCommand::Mount {
        pane_id: PaneId::Plugin(PluginId::new(1)),
        existing: 0,
        layout: PaneTabsLayout {
            active: 0,
            tabs: vec![PaneTabDescriptor {
                label: "Files".into(),
                location: "yinz:strider".into(),
                initial_cwd: None,
                configuration: BTreeMap::new(),
            }],
        },
    });
    for (permissions, expected) in [
        (
            vec![PermissionType::ChangeApplicationState],
            (
                PermissionStatus::Denied,
                Some(PermissionType::OpenTerminalsOrPlugins),
            ),
        ),
        (
            vec![PermissionType::OpenTerminalsOrPlugins],
            (
                PermissionStatus::Denied,
                Some(PermissionType::ChangeApplicationState),
            ),
        ),
        (
            vec![
                PermissionType::ChangeApplicationState,
                PermissionType::OpenTerminalsOrPlugins,
            ],
            (PermissionStatus::Granted, None),
        ),
    ] {
        *env.permissions.lock().unwrap() = Some(permissions.into_iter().collect());
        assert_eq!(check_command_permission(&env, &command), expected);
    }
}

#[test]
fn multipane_adoption_and_launch_require_the_existing_permission_categories() {
    use yinz_types::multipane::*;
    let env = crate::test_env::test_env();
    let terminal = PaneId::Terminal(TerminalId::new(2));
    let command = |child| {
        PluginCommand::CreateMultipane(CreateMultipane {
            children: [MultipaneChild::Existing(terminal), child],
            placement: OverlayPlacement::default(),
            split: MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 5000,
            },
            focus: None,
            min_cells: 0,
        })
    };
    let existing = command(MultipaneChild::Existing(PaneId::Plugin(PluginId::new(3))));
    let launch = command(MultipaneChild::Terminal(CommandToRun {
        path: "sh".into(),
        ..Default::default()
    }));
    let plugin = command(MultipaneChild::Plugin {
        url: "file:/test.scm".into(),
        configuration: BTreeMap::new(),
    });
    *env.permissions.lock().unwrap() =
        Some(HashSet::from([PermissionType::ChangeApplicationState]));
    assert_eq!(
        check_command_permission(&env, &existing),
        (PermissionStatus::Granted, None)
    );
    assert_eq!(
        check_command_permission(&env, &launch),
        (PermissionStatus::Denied, Some(PermissionType::RunCommands))
    );
    assert_eq!(
        check_command_permission(&env, &plugin),
        (
            PermissionStatus::Denied,
            Some(PermissionType::OpenTerminalsOrPlugins)
        )
    );
    *env.permissions.lock().unwrap() = Some(HashSet::from([
        PermissionType::RunCommands,
        PermissionType::OpenTerminalsOrPlugins,
    ]));
    for command in [existing, launch, plugin] {
        assert_eq!(
            check_command_permission(&env, &command),
            (
                PermissionStatus::Denied,
                Some(PermissionType::ChangeApplicationState)
            )
        );
    }
}

#[test]
fn writing_a_layout_file_requires_the_permissions_applying_one_does() {
    let mut env = crate::test_env::test_env();
    let save = PluginCommand::SaveLayout {
        layout_name: "default".into(),
        layout_kdl: "layout { pane; }".into(),
        overwrite: true,
    };
    let rename = PluginCommand::RenameLayout {
        old_layout_name: "mine".into(),
        new_layout_name: "default".into(),
    };
    let delete = PluginCommand::DeleteLayout {
        layout_name: "default".into(),
    };
    let everything = [PermissionType::ChangeApplicationState]
        .into_iter()
        .chain(LAYOUT_FILE_PERMISSIONS);
    for (permissions, expected) in [
        (
            vec![PermissionType::ChangeApplicationState],
            (PermissionStatus::Denied, Some(PermissionType::OpenFiles)),
        ),
        (
            vec![
                PermissionType::ChangeApplicationState,
                PermissionType::OpenFiles,
                PermissionType::OpenTerminalsOrPlugins,
            ],
            (PermissionStatus::Denied, Some(PermissionType::RunCommands)),
        ),
        (
            LAYOUT_FILE_PERMISSIONS.to_vec(),
            (
                PermissionStatus::Denied,
                Some(PermissionType::ChangeApplicationState),
            ),
        ),
        (everything.collect(), (PermissionStatus::Granted, None)),
    ] {
        *env.permissions.lock().unwrap() = Some(permissions.into_iter().collect());
        for command in [&save, &rename] {
            assert_eq!(check_command_permission(&env, command), expected);
        }
    }
    // Deleting a layout cannot make anything run, so it stays a state change.
    *env.permissions.lock().unwrap() =
        Some(HashSet::from([PermissionType::ChangeApplicationState]));
    assert_eq!(
        check_command_permission(&env, &delete),
        (PermissionStatus::Granted, None)
    );
    // Built-in plugins, such as the layout palette, keep saving and renaming without asking.
    env.plugin_config.location = yinz_types::input::layout::RunPluginLocation::Yinz(
        yinz_types::data::PluginTag::new("layout-palette"),
    );
    *env.permissions.lock().unwrap() = None;
    for command in [&save, &rename] {
        assert_eq!(
            check_command_permission(&env, command),
            (PermissionStatus::Granted, None)
        );
    }
}

#[test]
fn a_syntax_check_needs_no_permission_but_a_semantic_check_reads_files() {
    use std::sync::{Arc, Mutex};
    use yinz_types::authoring::PluginCheckMode;
    let mut env = crate::test_env::test_env();
    env.permissions = Arc::new(Mutex::new(Some(HashSet::new())));
    let check = |mode| PluginCommand::CheckPlugin {
        name: "check.scm".into(),
        source: "(void)".into(),
        mode,
    };
    assert_eq!(
        check_command_permission(&env, &check(PluginCheckMode::Syntax)),
        (PermissionStatus::Granted, None)
    );
    for mode in [
        PluginCheckMode::Semantic,
        PluginCheckMode::Format,
        PluginCheckMode::Prepare,
    ] {
        assert_eq!(
            check_command_permission(&env, &check(mode)),
            (PermissionStatus::Denied, Some(PermissionType::ReadFiles))
        );
    }
}

#[test]
fn project_configuration_requires_file_permissions() {
    assert_eq!(
        command_permission(CommandType::ProjectConfigRead),
        Some(PermissionType::ReadFiles)
    );
    assert_eq!(
        command_permission(CommandType::ProjectConfigWrite),
        Some(PermissionType::WriteFiles)
    );
}

#[test]
fn moving_focus_needs_the_same_permission_in_every_direction() {
    for kind in [
        CommandType::FocusNextPane,
        CommandType::FocusPreviousPane,
        CommandType::FocusLastPane,
    ] {
        assert_eq!(
            command_permission(kind),
            Some(PermissionType::ChangeApplicationState),
            "{kind}"
        );
    }
}

#[test]
fn loading_a_new_plugin_needs_plugin_launch_permission() {
    assert_eq!(
        command_permission(CommandType::LoadNewPlugin),
        Some(PermissionType::OpenTerminalsOrPlugins)
    );
    assert_eq!(
        command_permission(CommandType::ReloadPlugin),
        Some(PermissionType::ChangeApplicationState)
    );
}

#[test]
fn layout_permissions_cover_every_active_startup_pane_kind() {
    let tiled_layout = yinz_types::input::layout::TiledPaneLayout {
        children: vec![
            yinz_types::input::layout::TiledPaneLayout {
                run: Some(Run::Command(RunCommand::default())),
                ..Default::default()
            },
            yinz_types::input::layout::TiledPaneLayout {
                run: Some(Run::EditFile(PathBuf::from("active"), None, None)),
                ..Default::default()
            },
            yinz_types::input::layout::TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::default())),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let layout = Layout {
        template: Some((tiled_layout, vec![])),
        ..Default::default()
    };
    assert_eq!(
        layout_run_permissions(&layout),
        BTreeSet::from([
            PermissionType::OpenFiles,
            PermissionType::RunCommands,
            PermissionType::OpenTerminalsOrPlugins,
        ])
    );
}

#[test]
fn layout_permissions_ignore_non_startup_work() {
    let ignored = Some(Run::Command(RunCommand::default()));
    let tiled_layout = yinz_types::input::layout::TiledPaneLayout {
        run: ignored.clone(),
        run_instructions_to_ignore: vec![ignored],
        ..Default::default()
    };
    let already_running = yinz_types::input::layout::FloatingPaneLayout {
        run: Some(Run::Plugin(RunPluginOrAlias::default())),
        already_running: true,
        ..Default::default()
    };
    let swap_layout = yinz_types::input::layout::TiledPaneLayout {
        run: Some(Run::EditFile(PathBuf::from("swap"), None, None)),
        ..Default::default()
    };
    let layout = Layout {
        template: Some((tiled_layout, vec![already_running])),
        swap_layouts: vec![(swap_layout, vec![])],
        ..Default::default()
    };
    assert!(layout_run_permissions(&layout).is_empty());
}

#[test]
fn override_layout_checks_run_permissions_before_dispatch() {
    use crate::PluginInstruction;
    use std::time::Duration;
    use yinz_bus::screen::ScreenInstruction;
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::data::{Event, LayoutInfo};

    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("saved.kdl"), "layout { pane; }").unwrap();
    let mut cases = vec![(
        LayoutInfo::File("saved".into(), Default::default()),
        LAYOUT_FILE_PERMISSIONS.to_vec(),
    )];
    for (pane, permission) in [
        (
            r#"pane command="/usr/bin/true";"#,
            PermissionType::RunCommands,
        ),
        (r#"pane edit="notes.txt";"#, PermissionType::OpenFiles),
        (
            r#"pane { plugin location="file:test.scm"; };"#,
            PermissionType::OpenTerminalsOrPlugins,
        ),
    ] {
        for contents in [
            pane.to_owned(),
            format!("pane {{ {pane} }};"),
            format!("tab {{ {pane} }};"),
            format!("pane; floating_panes {{ {pane} }};"),
            format!("tab {{ pane; floating_panes {{ {pane} }}; }};"),
        ] {
            cases.push((
                LayoutInfo::Stringified(format!("layout {{ {contents} }}")),
                vec![permission],
            ));
        }
    }

    for (layout, required) in cases {
        let mut env = crate::test_env::test_env();
        assert!(!env.plugin_config.is_builtin());
        env.layout_dir = Some(directory.path().to_owned());
        let (to_screen, screen) = unbounded();
        let (to_pty, pty) = unbounded();
        let (to_plugin, plugin) = unbounded();
        env.senders.to_screen = Some(SenderWithContext::new(to_screen));
        env.senders.to_pty = Some(SenderWithContext::new(to_pty));
        env.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
        let command =
            PluginCommand::OverrideLayout(layout.clone(), false, false, true, BTreeMap::new());
        for missing in &required {
            *env.permissions.lock().unwrap() = Some(
                [PermissionType::ChangeApplicationState]
                    .into_iter()
                    .chain(required.iter().copied().filter(|grant| grant != missing))
                    .collect(),
            );
            let error = run_plugin_command(&mut env, command.clone()).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("layout needs permissions this plugin has not been granted: {missing}"),
                "{layout:?}"
            );
            assert!(screen.is_empty());
            assert!(pty.is_empty());
            assert!(plugin.is_empty());
        }

        *env.permissions.lock().unwrap() = Some(
            [PermissionType::ChangeApplicationState]
                .into_iter()
                .chain(required)
                .collect(),
        );
        run_plugin_command(&mut env, command).unwrap();
        let (instruction, _) = screen.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(matches!(instruction, ScreenInstruction::OverrideLayout(..)));
        drop(instruction);
        // Releasing the screen completion lets this action's worker finish before the next case.
        let (instruction, _) = plugin.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(matches!(
            instruction,
            PluginInstruction::Update(events)
                if matches!(events.as_slice(), [(_, _, Event::InputReceived)])
        ));
        let (instruction, _) = plugin.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(matches!(
            instruction,
            PluginInstruction::Update(events)
                if matches!(events.as_slice(), [(_, _, Event::ActionComplete(..))])
        ));
        assert!(screen.is_empty());
        assert!(pty.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn plugin_layout_paths_cannot_escape_the_layout_directory() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let layout_dir = temp.path().join("layouts");
    std::fs::create_dir(&layout_dir).unwrap();
    std::fs::write(layout_dir.join("safe.kdl"), "layout {}").unwrap();
    let outside = temp.path().join("outside.kdl");
    std::fs::write(&outside, "layout {}").unwrap();

    validate_plugin_layout_path("safe", Some(&layout_dir)).unwrap();
    symlink(layout_dir.join("safe.kdl"), layout_dir.join("inside.kdl")).unwrap();
    validate_plugin_layout_path("inside", Some(&layout_dir)).unwrap();

    assert!(validate_plugin_layout_path("../outside", Some(&layout_dir)).is_err());
    assert!(validate_plugin_layout_path(outside.to_str().unwrap(), Some(&layout_dir)).is_err());

    symlink(&outside, layout_dir.join("escape.kdl")).unwrap();
    assert!(validate_plugin_layout_path("escape", Some(&layout_dir)).is_err());

    symlink(&outside, layout_dir.join("safe.swap.kdl")).unwrap();
    assert!(validate_plugin_layout_path("safe", Some(&layout_dir)).is_err());
}

#[test]
fn saving_layouts_preserves_existing_files_until_replacement_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::test_env();
    let layout_dir = temp.path().join("layouts");
    env.layout_dir = Some(layout_dir.clone());
    try_save_layout(&env, "saved", "layout {}", false).unwrap();
    let path = layout_dir.join("saved.kdl");
    let original = std::fs::read(&path).unwrap();

    assert!(try_save_layout(&env, "saved", "layout { pane; }", false)
        .unwrap_err()
        .contains("already exists"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(try_save_layout(&env, "saved", "invalid {", true).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    try_save_layout(&env, "saved", "layout { pane; }", true).unwrap();
    assert!(std::fs::read_to_string(path).unwrap().contains("pane"));
    assert_eq!(std::fs::read_dir(layout_dir).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn renaming_layouts_preserves_occupied_destinations_including_dangling_links() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::test_env();
    env.layout_dir = Some(temp.path().to_path_buf());
    let source = temp.path().join("source.kdl");
    let destination = temp.path().join("destination.kdl");
    std::fs::write(&source, "layout {}").unwrap();
    std::os::unix::fs::symlink(temp.path().join("missing"), &destination).unwrap();
    assert!(try_rename_layout(&env, "source", "destination")
        .unwrap_err()
        .contains("already exists"));
    assert!(destination.is_symlink());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "layout {}");

    std::fs::remove_file(&destination).unwrap();
    std::fs::write(&destination, "layout { pane; }").unwrap();
    assert!(try_rename_layout(&env, "source", "destination").is_err());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "layout { pane; }"
    );

    std::fs::remove_file(&destination).unwrap();
    try_rename_layout(&env, "source", "destination").unwrap();
    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "layout {}");
}

#[cfg(unix)]
#[test]
fn saving_layouts_never_writes_through_destination_symlinks() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::test_env();
    let layout_dir = temp.path().join("layouts");
    std::fs::create_dir(&layout_dir).unwrap();
    env.layout_dir = Some(layout_dir.clone());
    let outside = temp.path().join("outside");
    let destination = layout_dir.join("linked.kdl");
    symlink(&outside, &destination).unwrap();

    // A dangling link is still occupied, even though Path::exists reports false.
    assert!(try_save_layout(&env, "linked", "layout {}", false).is_err());
    assert!(!outside.exists());
    std::fs::write(&outside, "keep").unwrap();
    assert!(try_save_layout(&env, "linked", "layout {}", false).is_err());
    try_save_layout(&env, "linked", "layout {}", true).unwrap();
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
    assert!(!std::fs::symlink_metadata(destination).unwrap().is_symlink());
    assert_eq!(std::fs::read_dir(layout_dir).unwrap().count(), 1);
}

#[test]
fn project_worktree_defaults_apply_from_primary_and_external_checkouts() {
    use project_data::{projects, store};
    let first = repository();
    let second = repository();
    let external = tempfile::tempdir().unwrap();
    let linked = external.path().join("linked source");
    run_git(
        first.path(),
        &[
            "worktree",
            "add",
            "-b",
            "external",
            linked.to_str().unwrap(),
        ],
        "link",
        None,
    )
    .unwrap();
    let mut a = projects::validate("a", resolved(first.path()).to_str().unwrap()).unwrap();
    let relative = format!(
        "../{}/relative trees",
        external.path().file_name().unwrap().to_str().unwrap()
    );
    a.worktree_dir = Some(relative.clone());
    let mut b = projects::validate("b", resolved(second.path()).to_str().unwrap()).unwrap();
    b.worktree_dir = Some(
        external
            .path()
            .join("absolute trees")
            .to_string_lossy()
            .into_owned(),
    );
    let registry: projects::Registry = [(a.name.clone(), a), (b.name.clone(), b)]
        .into_iter()
        .collect();
    // The host reads the real shared store, including when the caller is a direct plugin.
    let db_dir = tempfile::tempdir().unwrap();
    let db = plugin_data_store::PluginDataStore::new(db_dir.path());
    db.define_entity(store::SHARED, "agents", &store::project_def(), false)
        .unwrap();
    for project in registry.values() {
        db.apply(store::SHARED, &[projects::to_mutation(project)])
            .unwrap();
    }
    for (source, branch, expected) in [
        (
            first.path(),
            "primary-default",
            first.path().join(&relative).join("primary-default"),
        ),
        (
            linked.as_path(),
            "linked-default",
            first.path().join(&relative).join("linked-default"),
        ),
        (
            second.path(),
            "absolute-default",
            external.path().join("absolute trees/absolute-default"),
        ),
    ] {
        let mut checkout = GitCheckout::NewWorktree {
            branch: branch.into(),
            base: "main".into(),
            path: format!(".worktrees/{branch}").into(),
        };
        apply_project_worktree_dir(&mut checkout, None, source, || {
            db.fetch(store::SHARED, &FetchRequest::entity("project"))
        })
        .unwrap();
        let made = agent_folder(
            checkout,
            None,
            source.into(),
            false,
            WorktreeGuard::Required,
        )
        .unwrap();
        assert_eq!(resolved(&made.path), resolved(&expected));
    }
}

#[test]
fn project_worktree_linked_defaults_follow_repository_membership() {
    use project_data::projects;
    let repo = repository();
    let unrelated = repository();
    let external = tempfile::tempdir().unwrap();
    let linked = external.path().join("registered");
    let sibling = external.path().join("sibling");
    for (branch, path) in [("linked", &linked), ("sibling", &sibling)] {
        run_git(
            repo.path(),
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
            "link",
            None,
        )
        .unwrap();
    }
    let alias = external.path().join("linked-alias");
    std::os::unix::fs::symlink(&linked, &alias).unwrap();
    let mut registry = projects::Registry::new();
    // Each stage adds a competing registration. Direct matches win, then the
    // primary root; otherwise linked roots must identify exactly one project.
    for (stage, root, dir, sources) in [
        (
            "linked",
            linked.as_path(),
            "saved",
            vec![
                (repo.path(), "saved"),
                (linked.as_path(), "saved"),
                (alias.as_path(), "saved"),
                (sibling.as_path(), "saved"),
                (unrelated.path(), ".worktrees"),
            ],
        ),
        (
            "ambiguous",
            sibling.as_path(),
            "sibling-saved",
            vec![
                (repo.path(), ".worktrees"),
                (linked.as_path(), "saved"),
                (alias.as_path(), "saved"),
                (sibling.as_path(), "sibling-saved"),
            ],
        ),
        (
            "primary",
            repo.path(),
            "primary-saved",
            vec![
                (repo.path(), "primary-saved"),
                (linked.as_path(), "saved"),
                (alias.as_path(), "saved"),
                (sibling.as_path(), "sibling-saved"),
            ],
        ),
    ] {
        let mut project = projects::validate(stage, resolved(root).to_str().unwrap()).unwrap();
        project.worktree_dir = Some(dir.into());
        registry.insert(project.name.clone(), project);
        for (index, (source, expected_dir)) in sources.into_iter().enumerate() {
            let branch = format!("{stage}-{index}");
            let mut checkout = GitCheckout::NewWorktree {
                branch: branch.clone(),
                base: "main".into(),
                path: format!(".worktrees/{branch}").into(),
            };
            apply_project_worktree_dir(&mut checkout, None, source, || {
                Ok(projects::to_rows(&registry))
            })
            .unwrap();
            let made = agent_folder(
                checkout,
                None,
                source.into(),
                false,
                WorktreeGuard::Required,
            )
            .unwrap();
            let primary = if source == unrelated.path() {
                unrelated.path()
            } else {
                repo.path()
            };
            assert_eq!(
                resolved(&made.path),
                resolved(&primary.join(expected_dir).join(branch)),
            );
        }
    }
}

#[test]
fn project_worktree_lookup_preserves_overrides_and_surfaces_store_errors() {
    let mut checkout = GitCheckout::NewWorktree {
        branch: "topic".into(),
        base: "main".into(),
        path: "/explicit/topic".into(),
    };
    apply_project_worktree_dir(&mut checkout, Some("/explicit"), "/unused".as_ref(), || {
        panic!("explicit launch must not read projects")
    })
    .unwrap();
    assert!(
        matches!(&checkout, GitCheckout::NewWorktree { path, .. } if path == std::path::Path::new("/explicit/topic"))
    );
    for mut unchanged in [
        GitCheckout::Current,
        GitCheckout::NewBranch {
            branch: "topic".into(),
            base: "main".into(),
        },
    ] {
        apply_project_worktree_dir(&mut unchanged, None, "/unused".as_ref(), || {
            panic!("no worktree must not read projects")
        })
        .unwrap();
    }
    apply_project_worktree_dir(&mut checkout, None, "/unused".as_ref(), || {
        Err(DataError::UnknownEntity("project".into()))
    })
    .unwrap();
    let error = apply_project_worktree_dir(&mut checkout, None, "/unused".as_ref(), || {
        Err(DataError::Storage("offline".into()))
    })
    .unwrap_err();
    assert!(error.contains("offline"));
    apply_project_worktree_dir(&mut checkout, None, "/unused".as_ref(), || {
        Ok(project_data::projects::to_rows(&Default::default()))
    })
    .unwrap();
}

#[test]
fn git_checkout_request_creates_a_worktree_from_the_base() {
    let repository = repository();
    let target = repository.path().join(".worktrees/topic");
    let cwd = reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewWorktree {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/topic"),
        },
    )
    .unwrap();
    // Resolved: the worktree path here is built from `git rev-parse
    // --show-toplevel`, and on macOS git answers the physical path while
    // `tempfile` handed out one under a symlinked `/var`. The absolute-path
    // case below is deliberately *not* resolved - there the point is that the
    // path is honoured exactly as given.
    assert_eq!(resolved(&cwd), resolved(&target));
    assert!(std::fs::read_to_string(target.join(".git"))
        .unwrap()
        .starts_with("gitdir:"));
}

/// A linked worktree of `primary`, nested inside it the way this repository
/// keeps its own.
fn worktree(primary: &std::path::Path, name: &str, branch: &str) -> PathBuf {
    let path = primary.join(".worktrees").join(name);
    assert!(process::Command::new("git")
        .arg("-C")
        .arg(primary)
        .args(["worktree", "add", "-b", branch])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    path
}

/// `tempfile` hands out a path under a symlinked `/var` on macOS while git
/// answers the physical one, so paths only compare after resolving.
fn resolved(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

/// With no branch there is no git work: the pane runs in the folder the caller named,
/// else in the source as given - a subfolder of a checkout stays that subfolder, and is
/// not swapped for the repository root the way a checkout request resolves it.
#[test]
fn an_agent_with_no_branch_runs_where_it_was_asked_and_never_at_the_repository_root() {
    let repository = repository();
    let subdirectory = repository.path().join("src/nested");
    std::fs::create_dir_all(&subdirectory).unwrap();
    let elsewhere = repository.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    assert_eq!(
        agent_folder_reusing(GitCheckout::Current, None, subdirectory.clone()).unwrap(),
        subdirectory
    );
    assert_eq!(
        agent_folder_reusing(
            GitCheckout::Current,
            Some(elsewhere.clone()),
            subdirectory.clone()
        )
        .unwrap(),
        elsewhere
    );
    // A branch is git work in the repository, resolved from the subfolder to its root,
    // and the pane lands in what it makes - `cwd` says nothing there.
    let checkout = GitCheckout::NewBranch {
        branch: "topic".to_owned(),
        base: "main".to_owned(),
    };
    let switched = agent_folder_reusing(checkout, Some(elsewhere), subdirectory).unwrap();
    assert_eq!(resolved(&switched), resolved(repository.path()));
}

#[test]
fn a_worktree_request_from_inside_a_worktree_anchors_to_the_primary() {
    let repository = repository();
    let first = worktree(repository.path(), "first", "first");
    let cwd = reusing_checkout_in(
        first.clone(),
        GitCheckout::NewWorktree {
            branch: "second".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/second"),
        },
    )
    .unwrap();
    // Checked before the equality, whose expected path does not exist when this
    // regresses — a missing-path panic would hide what actually went wrong.
    assert!(
        !resolved(&cwd).starts_with(resolved(&first)),
        "{cwd:?} must not be nested inside {first:?}"
    );
    assert_eq!(
        resolved(&cwd),
        resolved(&repository.path().join(".worktrees/second")),
        "the second worktree is a sibling of the first, not a child of it"
    );
    assert!(std::fs::read_to_string(cwd.join(".git"))
        .unwrap()
        .starts_with("gitdir:"));
}

#[test]
fn an_absolute_worktree_path_is_honoured_from_inside_a_worktree() {
    let repository = repository();
    let first = worktree(repository.path(), "first", "first");
    let target = repository.path().join("elsewhere/second");
    let cwd = reusing_checkout_in(
        first,
        GitCheckout::NewWorktree {
            branch: "second".to_owned(),
            base: "main".to_owned(),
            path: target.clone(),
        },
    )
    .unwrap();
    assert_eq!(cwd, target);
    assert!(std::fs::read_to_string(target.join(".git"))
        .unwrap()
        .starts_with("gitdir:"));
}

#[test]
fn branch_only_switches_the_current_checkout() {
    let repository = repository();
    reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
        },
    )
    .unwrap();
    let branch = process::Command::new("git")
        .arg("-C")
        .arg(repository.path())
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "topic");
}

/// The branch a checkout is on, or `None` when it is detached.
fn branch_of(worktree: &std::path::Path) -> Option<String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!branch.is_empty()).then_some(branch)
}

/// Create a local branch without checking it out anywhere.
fn branch(primary: &std::path::Path, name: &str) {
    assert!(process::Command::new("git")
        .arg("-C")
        .arg(primary)
        .args(["branch", name])
        .status()
        .unwrap()
        .success());
}

/// The point of the whole reuse path: an agent asked for where work already
/// is, rather than an error saying it is already there.
#[test]
fn a_worktree_that_already_exists_at_the_path_is_reused_as_it_stands() {
    let repository = repository();
    let existing = worktree(repository.path(), "topic", "topic");
    let prepared = prepare_git_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewWorktree {
            // Neither the branch nor the base the worktree was made with:
            // the folder is the request, and it is used as it stands.
            branch: "other".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/topic"),
        },
        true,
        WorktreeGuard::Waived,
    )
    .unwrap();
    assert_eq!(resolved(&prepared.path), resolved(&existing));
    // The branch the folder holds, not the one that was asked for and not used:
    // that is what the agent is reported as sitting on.
    assert_eq!(prepared.branch.as_deref(), Some("topic"));
    assert_eq!(branch_of(&existing).as_deref(), Some("topic"));
    // Nothing was created for the branch that was asked for and not used.
    assert!(!branch_exists(repository.path(), "other"));
}

#[test]
fn triage_prompts_follow_each_actual_worktree_outcome() {
    let first = repository();
    let second = repository();
    let mut spec = AgentSpawnSpec {
        kind: Some("claude".into()),
        model: Some("opus".into()),
        branch: Some("issue-2180".into()),
        reuse: Some(true),
        prompt: Some("triage issue 2180".into()),
        reuse_prompt: Some("resume issue 2180".into()),
        ..Default::default()
    };
    let prepare = |root: &std::path::Path, spec: &AgentSpawnSpec| {
        let (_, checkout) = yinz_runtime::agent::launch::plan(spec).unwrap();
        prepare_git_checkout_in(root.to_owned(), checkout, true, WorktreeGuard::Required).unwrap()
    };
    // A branch without a checkout is still a new worktree.
    branch(first.path(), "issue-2180");
    let new = prepare(first.path(), &spec);
    assert!(!new.reused_worktree);
    assert_eq!(
        new.agent_command(&spec).unwrap().args.last().unwrap(),
        "triage issue 2180"
    );
    let reused = prepare(first.path(), &spec);
    let other_new = prepare(second.path(), &spec);
    assert!(reused.reused_worktree);
    assert!(!other_new.reused_worktree);
    assert_eq!(
        reused.agent_command(&spec).unwrap().args.last().unwrap(),
        "resume issue 2180"
    );
    assert_eq!(
        other_new.agent_command(&spec).unwrap().args.last().unwrap(),
        "triage issue 2180"
    );
    // Branch lookup at a different path is reuse too.
    spec.worktree_dir = Some("elsewhere".into());
    let moved = prepare(first.path(), &spec);
    assert!(moved.reused_worktree);
    assert_eq!(moved.path, reused.path);
    assert_eq!(
        moved.agent_command(&spec).unwrap().args.last().unwrap(),
        "resume issue 2180"
    );
    spec.reuse_prompt = None;
    assert_eq!(
        moved.agent_command(&spec).unwrap().args.last().unwrap(),
        "triage issue 2180"
    );
}

/// Git allows one checkout of a branch at a time, so the worktree holding it
/// is the only place an agent can work on it.
#[test]
fn a_branch_checked_out_elsewhere_answers_with_that_worktree() {
    let repository = repository();
    let existing = worktree(repository.path(), "first", "topic");
    let cwd = reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewWorktree {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
            // A path of its own, which is not where the branch turned out to
            // live.
            path: PathBuf::from(".worktrees/second"),
        },
    )
    .unwrap();
    assert_eq!(resolved(&cwd), resolved(&existing));
    assert!(!repository.path().join(".worktrees/second").exists());
}

#[test]
fn an_existing_branch_gets_a_worktree_without_being_recreated() {
    let repository = repository();
    branch(repository.path(), "topic");
    let cwd = reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewWorktree {
            branch: "topic".to_owned(),
            // Not a branch at all: an existing branch has its own history,
            // so the base is never consulted.
            base: "no-such-base".to_owned(),
            path: PathBuf::from(".worktrees/topic"),
        },
    )
    .unwrap();
    assert_eq!(
        resolved(&cwd),
        resolved(&repository.path().join(".worktrees/topic"))
    );
    assert_eq!(branch_of(&cwd).as_deref(), Some("topic"));
}

#[test]
fn an_existing_branch_is_switched_to_rather_than_recreated() {
    let repository = repository();
    branch(repository.path(), "topic");
    let cwd = reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "no-such-base".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(resolved(&cwd), resolved(repository.path()));
    assert_eq!(branch_of(repository.path()).as_deref(), Some("topic"));
}

#[test]
fn a_branch_request_for_a_branch_held_elsewhere_answers_with_that_worktree() {
    let repository = repository();
    let existing = worktree(repository.path(), "first", "topic");
    let cwd = reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(resolved(&cwd), resolved(&existing));
    // The checkout the request came from was left where it was.
    assert_eq!(branch_of(repository.path()).as_deref(), Some("main"));
}

/// A folder in the way that is not a worktree is a real clash, and still one
/// git gets to report.
#[test]
fn a_path_that_is_not_a_worktree_still_fails() {
    let repository = repository();
    let occupied = repository.path().join(".worktrees/topic");
    std::fs::create_dir_all(&occupied).unwrap();
    std::fs::write(occupied.join("file"), "content").unwrap();
    let error = reusing_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewWorktree {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/topic"),
        },
    )
    .unwrap_err();
    assert!(
        error.starts_with("git could not create the worktree"),
        "{error}"
    );
}

/// Without reuse, a name already taken is stepped around rather than joined - and
/// the worktree is renamed with its branch, because a worktree is named after the
/// branch it holds and that is the whole of 116.
#[test]
fn a_taken_name_is_cut_afresh_with_the_worktree_renamed_to_match() {
    let repository = repository();
    let request = || GitCheckout::NewWorktree {
        branch: "topic".to_owned(),
        base: "main".to_owned(),
        path: PathBuf::from(".worktrees/topic"),
    };
    let first = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(first.branch.as_deref(), Some("topic"));
    assert_eq!(
        resolved(&first.path),
        resolved(&repository.path().join(".worktrees/topic"))
    );

    let second = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(second.branch.as_deref(), Some("topic-2"));
    assert_eq!(
        resolved(&second.path),
        resolved(&repository.path().join(".worktrees/topic-2"))
    );
    let third = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(third.branch.as_deref(), Some("topic-3"));

    // Reuse is the other answer to the same request: the first worktree, as it stands.
    let reused = reusing_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(resolved(&reused), resolved(&first.path));
}

/// A folder in the way that is not a worktree is a hard failure under reuse (above);
/// without it there is nothing to reuse and everything to step around, so the name
/// moves on instead.
#[test]
fn a_folder_in_the_way_is_stepped_around_rather_than_failed_without_reuse() {
    let repository = repository();
    let occupied = repository.path().join(".worktrees/topic");
    std::fs::create_dir_all(&occupied).unwrap();
    std::fs::write(occupied.join("file"), "content").unwrap();
    let prepared = cutting_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewWorktree {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/topic"),
        },
    )
    .unwrap();
    assert_eq!(prepared.branch.as_deref(), Some("topic-2"));
}

/// The branch-only shape says the same thing: an existing `topic` is not joined, and
/// the checkout moves to the next free name.
#[test]
fn a_branch_without_reuse_is_cut_afresh_rather_than_switched_to() {
    let repository = repository();
    let request = || GitCheckout::NewBranch {
        branch: "topic".to_owned(),
        base: "main".to_owned(),
    };
    let first = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(first.branch.as_deref(), Some("topic"));
    let second = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(second.branch.as_deref(), Some("topic-2"));
    assert_eq!(branch_of(repository.path()).as_deref(), Some("topic-2"));
}

/// A branch with a `/` in it is laid out as folders, and the folder it asks for is the
/// one `launch::worktree_path_in` planned and the confirmation promised - not a level
/// deeper. The suffix a collision cuts goes on the branch and on that folder's last
/// component together.
#[test]
fn a_branch_with_a_slash_keeps_the_worktree_path_it_was_planned_with() {
    let repository = repository();
    let request = || GitCheckout::NewWorktree {
        branch: "feature/foo".to_owned(),
        base: "main".to_owned(),
        path: PathBuf::from(".worktrees/feature/foo"),
    };
    let first = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(first.branch.as_deref(), Some("feature/foo"));
    assert_eq!(
        resolved(&first.path),
        resolved(&repository.path().join(".worktrees/feature/foo"))
    );

    let second = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
    assert_eq!(second.branch.as_deref(), Some("feature/foo-2"));
    assert_eq!(
        resolved(&second.path),
        resolved(&repository.path().join(".worktrees/feature/foo-2"))
    );
}

/// The guard is asked *before* the git work, not of the folder it produced: a refusal
/// that had already switched the user's checkout onto a new branch would leave them
/// worse off than the spawn they asked for.
#[test]
fn a_refused_branch_leaves_the_checkout_where_it_was() {
    let repository = repository();
    let error = prepare_git_checkout_in(
        repository.path().to_owned(),
        GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
        },
        false,
        WorktreeGuard::Required,
    )
    .unwrap_err();
    assert!(error.contains("is not a linked git worktree"), "{error}");
    assert_eq!(branch_of(repository.path()).as_deref(), Some("main"));
    assert!(!branch_exists(repository.path(), "topic"));
}

/// The guard is about the destination rather than the spec: the same request from
/// inside a linked worktree moves that worktree, and is allowed.
#[test]
fn a_branch_cut_inside_a_linked_worktree_passes_the_guard() {
    let repository = repository();
    let linked = worktree(repository.path(), "other", "other");
    let prepared = prepare_git_checkout_in(
        linked.clone(),
        GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
        },
        false,
        WorktreeGuard::Required,
    )
    .unwrap();
    assert_eq!(prepared.branch.as_deref(), Some("topic"));
    assert_eq!(resolved(&prepared.path), resolved(&linked));
    assert_eq!(branch_of(&linked).as_deref(), Some("topic"));
    // The primary checkout was not touched.
    assert_eq!(branch_of(repository.path()).as_deref(), Some("main"));
}

/// The no-git-work shapes are guarded too - a spawn with no branch is the case the
/// guard was written for - and `agent_folder` owns that check for the folder it is
/// handed as much as for the repository root.
#[test]
fn a_folder_that_is_no_linked_worktree_is_refused_without_git_work() {
    let repository = repository();
    let error = agent_folder(
        GitCheckout::Current,
        None,
        repository.path().to_owned(),
        false,
        WorktreeGuard::Required,
    )
    .unwrap_err();
    assert!(error.contains("is not a linked git worktree"), "{error}");
    let outside = tempfile::tempdir().unwrap();
    let error = agent_folder(
        GitCheckout::Current,
        Some(outside.path().to_owned()),
        repository.path().to_owned(),
        false,
        WorktreeGuard::Required,
    )
    .unwrap_err();
    assert!(error.contains("is not a linked git worktree"), "{error}");
    // And a linked worktree is what it asks for.
    let linked = worktree(repository.path(), "topic", "topic");
    let prepared = agent_folder(
        GitCheckout::Current,
        Some(linked.clone()),
        repository.path().to_owned(),
        false,
        WorktreeGuard::Required,
    )
    .unwrap();
    assert_eq!(prepared.path, linked);
}

/// What the guard in `spawn_agent` asks of a folder before an agent is left in it.
#[test]
fn only_a_linked_checkout_is_a_linked_worktree() {
    let repository = repository();
    assert!(!linked_worktree(repository.path()).unwrap());
    let linked = worktree(repository.path(), "topic", "topic");
    assert!(linked_worktree(&linked).unwrap());
    // A subfolder of one is inside it, and answers for it.
    let nested = linked.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(linked_worktree(&nested).unwrap());
    // A folder that is no repository at all is not a worktree either, and saying so
    // is the answer rather than an error: that spawn is one the caller must own.
    let outside = tempfile::tempdir().unwrap();
    assert!(!linked_worktree(outside.path()).unwrap());
}

#[test]
fn the_porcelain_listing_reads_a_path_and_a_branch_per_checkout() {
    let worktrees = parse_worktrees(
        "worktree /work/repo\nHEAD abcd\nbranch refs/heads/main\n\
         \nworktree /work/repo/.worktrees/topic\nHEAD ef01\nbranch refs/heads/topic\n\
         \nworktree /work/detached\nHEAD 2345\ndetached\n",
    );
    let read: Vec<_> = worktrees
        .iter()
        .map(|worktree| (worktree.path.clone(), worktree.branch.clone()))
        .collect();
    assert_eq!(
        read,
        [
            (PathBuf::from("/work/repo"), Some("main".to_owned())),
            (
                PathBuf::from("/work/repo/.worktrees/topic"),
                Some("topic".to_owned())
            ),
            (PathBuf::from("/work/detached"), None),
        ]
    );
}
