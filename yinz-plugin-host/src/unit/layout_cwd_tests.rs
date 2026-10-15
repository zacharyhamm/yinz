use super::*;
use prost::Message;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_types::input::layout::RunPluginLocation;
use yinz_types::pick::{PickCommand, PickKind, PickOptions, PickRequest};

fn session_connection(
    env: &mut PluginEnv,
    layout: Option<LayoutInfo>,
    cwd: Option<PathBuf>,
) -> SessionConnection {
    let (to_server, receive) = channels::unbounded();
    env.senders.to_server = Some(SenderWithContext::new(to_server));
    let server = thread::spawn(move || {
        let (instruction, _) = receive.recv_timeout(Duration::from_secs(30)).unwrap();
        let ServerInstruction::SwitchSession(connection, _, _) = instruction else {
            panic!("expected session switch, got {instruction:?}");
        };
        connection
    });
    switch_session(env, None, None, None, layout, cwd).unwrap();
    server.join().unwrap()
}

fn pick_command(roots: Option<Vec<String>>) -> PickCommand {
    PickCommand::Start {
        request_id: "directory".into(),
        options: PickOptions {
            kind: PickKind::Directory,
            roots,
            extensions: None,
            hidden: None,
            recursive: None,
            timeout_ms: None,
        },
    }
}

fn pick_reply(env: &mut PluginEnv, command: PickCommand) -> Result<Option<PickRequest>, String> {
    env.response_sink = None;
    crate::plugin_bridge::picker::dispatch(env, command).unwrap();
    let bytes: Vec<u8> = env.response_sink.take().unwrap();
    let reply =
        yinz_plugin_api::plugin_command::ProtobufPickResponse::decode(bytes.as_slice()).unwrap();
    serde_json::from_str(&reply.json).unwrap()
}

fn browse(env: &mut PluginEnv, roots: Option<Vec<String>>, focused_cwd: &Path) -> PickRequest {
    let (to_screen, screen) = channels::unbounded();
    let (to_pty, pty) = channels::unbounded();
    let (to_plugin, plugin) = channels::unbounded();
    env.senders.to_screen = Some(SenderWithContext::new(to_screen));
    env.senders.to_pty = Some(SenderWithContext::new(to_pty));
    env.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    let focused_cwd = focused_cwd.to_owned();
    let host = thread::spawn(move || {
        let (instruction, _) = screen.recv_timeout(Duration::from_secs(30)).unwrap();
        let ScreenInstruction::GetFocusedPaneInfo {
            response_channel, ..
        } = instruction
        else {
            panic!("expected focused pane query, got {instruction:?}");
        };
        response_channel
            .send(GetFocusedPaneInfoResponse::Ok {
                tab_id: TabId::new(0),
                pane_id: PaneId::Terminal(TerminalId::new(1)),
            })
            .unwrap();
        let (instruction, _) = pty.recv_timeout(Duration::from_secs(30)).unwrap();
        let PtyInstruction::GetPaneCwd {
            response_channel, ..
        } = instruction
        else {
            panic!("expected pane directory query, got {instruction:?}");
        };
        response_channel
            .send(GetPaneCwdResponse::Ok(focused_cwd))
            .unwrap();
    });
    assert_eq!(pick_reply(env, pick_command(roots)), Ok(None));
    host.join().unwrap();
    let (instruction, _) = plugin.try_recv().unwrap();
    let PluginInstruction::Pick {
        command: PickCommand::Start { options, .. },
        cwd,
        ..
    } = instruction
    else {
        panic!("expected picker start, got {instruction:?}");
    };
    PickRequest { options, cwd }
}

#[test]
fn session_palette_omitted_directory_preserves_default_and_selected_layout_cwds() {
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = PathBuf::from("/startup");
    for plugin in ["session-palette", "session-manager"] {
        env.plugin_config.location =
            RunPluginLocation::Yinz(yinz_types::data::PluginTag::new(plugin));
        for layout in [
            None,
            Some(LayoutInfo::BuiltIn("default".into())),
            Some(LayoutInfo::Stringified(
                "layout cwd=\"/layout\" { pane cwd=\"project\"; }".into(),
            )),
        ] {
            let connection = session_connection(&mut env, layout.clone(), None);
            assert_eq!(connection.layout, layout);
            assert_eq!(
                connection.cwd,
                (plugin != "session-palette").then(|| env.plugin_cwd.clone())
            );
        }
    }
}

#[test]
fn session_palette_browse_and_submit_resolve_the_same_host_directory() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let temp = tempfile::Builder::new()
        .prefix(".yinz-directory-test-")
        .tempdir_in(&home)
        .unwrap();
    let project = temp.path().join("project with spaces");
    let focused = temp.path().join("focused");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&focused).unwrap();
    let mut env = crate::test_env::test_env();
    env.plugin_config.location =
        RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("session-palette"));
    env.plugin_cwd = temp.path().to_owned();
    let home_relative = project.strip_prefix(&home).unwrap().display();
    for input in [
        format!("~/{home_relative}"),
        format!("$HOME/{home_relative}"),
        "project with spaces".into(),
        project.to_str().unwrap().to_owned(),
    ] {
        let connection = session_connection(&mut env, None, Some(PathBuf::from(&input)));
        let request = browse(&mut env, Some(vec![input.clone()]), &focused);
        assert_eq!(connection.cwd, Some(project.clone()), "{input}");
        assert_eq!(request.cwd, focused);
        assert_eq!(
            request.options.roots,
            Some(vec![project.to_str().unwrap().into()]),
            "{input}"
        );
    }
}

#[test]
fn session_palette_invalid_directories_fail_before_switching_or_browsing() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("file"), "not a directory").unwrap();
    let mut env = crate::test_env::test_env();
    env.plugin_config.location =
        RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("session-palette"));
    env.plugin_cwd = temp.path().to_owned();
    let (to_server, server) = channels::unbounded();
    let (to_plugin, plugin) = channels::unbounded();
    env.senders.to_server = Some(SenderWithContext::new(to_server));
    env.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    let undefined = "YINZ_SESSION_PALETTE_UNDEFINED_DIRECTORY_TEST";
    assert!(std::env::var_os(undefined).is_none());
    for input in [
        "missing".to_owned(),
        "file".into(),
        format!("${undefined}/project"),
    ] {
        let error =
            switch_session(&env, None, None, None, None, Some(PathBuf::from(&input))).unwrap_err();
        let browse_error = pick_reply(&mut env, pick_command(Some(vec![input]))).unwrap_err();
        assert_eq!(browse_error, format!("{error:#}"));
        assert!(server.try_recv().is_err());
        assert!(plugin.try_recv().is_err());
    }
}

#[test]
fn session_palette_empty_browse_and_other_plugins_keep_focused_directory_semantics() {
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = PathBuf::from("/startup");
    let focused = Path::new("/focused");
    for plugin in ["session-palette", "session-manager"] {
        env.plugin_config.location =
            RunPluginLocation::Yinz(yinz_types::data::PluginTag::new(plugin));
        let request = browse(&mut env, None, focused);
        assert_eq!(request.cwd, focused);
        assert_eq!(request.options.roots, None);
    }
    let roots = Some(vec![
        "relative".into(),
        "~/project".into(),
        "$HOME/project".into(),
    ]);
    let request = browse(&mut env, roots.clone(), focused);
    assert_eq!(request.cwd, focused);
    assert_eq!(request.options.roots, roots);
}

#[test]
fn layout_tab_commands_preserve_kdl_cwds_without_a_tab_override() {
    for (raw_layout, expected_base) in [
        (
            r#"layout {
                pane cwd="/project/tiled"
                floating_panes { pane cwd="/project/floating"; }
            }"#,
            "/project",
        ),
        (
            r#"layout cwd="/project" {
                pane cwd="tiled"
                floating_panes { pane cwd="floating"; }
            }"#,
            "/project",
        ),
        (
            r#"layout cwd="/project" {
                tab cwd="work" {
                    pane cwd="tiled"
                    floating_panes { pane cwd="floating"; }
                }
            }"#,
            "/project/work",
        ),
    ] {
        for use_layout_info in [false, true] {
            let mut env = crate::test_env::test_env();
            env.plugin_cwd = PathBuf::from("/caller");
            env.response_sink = None;
            let (to_screen, screen_receiver) = channels::unbounded();
            env.senders.to_screen = Some(SenderWithContext::new(to_screen));
            let screen_thread = thread::spawn(move || {
                let (instruction, _) = screen_receiver
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap();
                let ScreenInstruction::NewTab(cwd, _, tiled, floating, ..) = instruction else {
                    panic!("expected a new tab, got {instruction:?}");
                };
                // Dropping the remaining instruction fields completes the routed action.
                (cwd, tiled.unwrap(), floating)
            });
            if use_layout_info {
                new_tabs_with_layout_info(&mut env, LayoutInfo::Stringified(raw_layout.to_owned()))
                    .unwrap();
            } else {
                new_tabs_with_layout(&mut env, raw_layout).unwrap();
            }
            let (cwd, tiled, floating) = screen_thread.join().unwrap();
            assert_eq!(cwd, None);
            assert_eq!(
                tiled.extract_run_instructions(),
                vec![Some(Run::Cwd(PathBuf::from(expected_base).join("tiled")))],
            );
            assert_eq!(floating.len(), 1);
            assert_eq!(
                floating[0].run,
                Some(Run::Cwd(PathBuf::from(expected_base).join("floating"))),
            );
        }
    }
}
