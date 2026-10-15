use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;
use yinz_types::style::HostTerminalThemeMode;

/// The rendered output of every `PluginBytes` asset in `instructions`.
fn renders(instructions: &[ScreenInstruction]) -> Vec<String> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets.iter()),
            _ => None,
        })
        .flatten()
        .map(|asset| String::from_utf8_lossy(&asset.bytes).to_string())
        .collect()
}

/// `HostTerminalThemeChanged` is broadcast once, when the session's hue changes, so a plugin that
/// subscribes to it afterwards would otherwise only learn the hue at the next change - which may
/// never come. The bridge remembers the last broadcast and replays it on subscription.
#[test]
#[serial(plugin_tests)]
pub fn a_plugin_subscribing_after_the_broadcast_is_told_the_current_host_theme_mode() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let received_screen_instructions = Watched::new(vec![]);
    // A fake screen thread that grants permissions and collects every instruction until Exit.
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let log = received_screen_instructions.clone();
            let cache_path = cache_path.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            move || loop {
                let (event, _err_ctx) = screen_receiver
                    .recv()
                    .expect("failed to receive event on channel");
                match event {
                    ScreenInstruction::RequestPluginPermissions(_plugin_id, plugin_permission) => {
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: plugin_permission.request_id,
                            status: PermissionStatus::Granted,
                            cache_path: Some(cache_path.clone()),
                        });
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {
                        log.lock().push(event);
                    },
                }
            }
        })
        .unwrap();

    // The hue is broadcast before any plugin exists to hear it.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        None,
        Event::HostTerminalThemeChanged(HostTerminalThemeMode::Light),
    )]));

    // Only then does a plugin load and subscribe to the event.
    let mut config = BTreeMap::new();
    config.insert(
        "subscribe_host_terminal_theme".to_owned(),
        "true".to_owned(),
    );
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(config),
        ..Default::default()
    });
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("test_plugin".to_owned()),
        run_plugin,
        Some(TabId::new(1)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    let expected_event = Event::HostTerminalThemeChanged(HostTerminalThemeMode::Light);
    received_screen_instructions.wait_until_described(
        "the plugin to render the replayed HostTerminalThemeChanged",
        yinz_test_support::default_timeout(),
        |instructions| {
            renders(instructions)
                .iter()
                .any(|render| fixture_events(render.as_bytes()).contains(&expected_event))
                .then_some(())
        },
        |instructions| {
            format!(
                "expected to find: {:?}\nin one of the renders: {:#?}",
                expected_event,
                renders(instructions),
            )
        },
    );

    // `teardown` is what sends the fake screen thread its Exit, so it has to come first.
    teardown();
    screen_thread.join().unwrap();
}
