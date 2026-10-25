use super::*;
use {yinz_types::data::BareKey, yinz_types::data::KeyWithModifier};

#[test]
fn cached_plugin_events_keep_generation_and_origin_across_attachment_and_reload() {
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    let cached = [
        EventOrPipeMessage::PluginEvent(first, 41, Event::Visible(true), None),
        EventOrPipeMessage::PluginEvent(second, 42, Event::Visible(false), None),
    ];
    let replay = |owner, generation, shared| {
        cached
            .iter()
            .filter_map(|message| {
                let client = message.client_for_replay(owner, generation, shared)?;
                let EventOrPipeMessage::PluginEvent(_, _, event, _) = message else {
                    unreachable!()
                };
                Some((client, event.clone()))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        replay(first, Some(41), false),
        vec![(first, Event::Visible(true))]
    );
    assert_eq!(
        replay(second, Some(42), false),
        vec![(second, Event::Visible(false))]
    );
    assert!(replay(second, Some(41), false).is_empty());
    assert!(replay(first, Some(43), false).is_empty());
    assert!(replay(first, None, false).is_empty());
    // Shared ownership may move, but commands resumed by this completion must
    // still act for the client which started the operation.
    assert_eq!(
        replay(second, Some(41), true),
        vec![(first, Event::Visible(true))]
    );
    assert_eq!(
        EventOrPipeMessage::Event(None, Event::InputReceived)
            .client_for_replay(second, None, false),
        Some(second)
    );
    let targeted = EventOrPipeMessage::Event(Some(first), Event::CellPixelSizeChanged(None));
    assert_eq!(targeted.client_for_replay(second, None, true), Some(first));
    assert_eq!(targeted.client_for_replay(first, None, false), Some(first));
    assert_eq!(targeted.client_for_replay(second, None, false), None);
}

fn plugin_config(configuration: &[(&str, &str)]) -> PluginConfig {
    let configuration = configuration
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    PluginConfig {
        repository_root: None,
        installed_commit: None,
        path: PathBuf::from("test-plugin"),
        _allow_exec_host_cmd: false,
        location: yinz_runtime::plugin_location::parse_plugin_location("yinz:tab-bar", None)
            .unwrap(),
        initial_userspace_configuration: PluginUserConfiguration::new(configuration),
        initial_cwd: None,
        relative_path: None,
    }
}

#[test]
fn shared_plugins_are_opted_into_through_their_configuration() {
    assert!(!plugin_config(&[]).is_shared());
    assert!(plugin_config(&[("shared", "true")]).is_shared());
    assert!(!plugin_config(&[("shared", "false")]).is_shared());
    assert!(!plugin_config(&[("shared", "yes please")]).is_shared());
}

#[test]
fn intercepted_keypress_batches_are_input_jobs() {
    let events = [(
        ClientId::new(1),
        Event::InterceptedKeyPress(KeyWithModifier::new(BareKey::Char('x'))),
    )];

    assert_eq!(
        summarize_batch_for_stats(events.iter().map(|(_, event)| event)),
        ("InterceptedKeyPress".to_owned(), true)
    );
}

/// The two halves of agent address each other by url, and the launch behind that url has to
/// find the half that is already on screen rather than open a second one.
///
/// The running instance carries the caller context the keybinding stamped on it; the greeting
/// from the other half carries its own, and less of it. Neither is identity.
#[test]
fn caller_context_does_not_hide_a_running_plugin_from_the_launch_lookup() {
    let configuration = |pairs: &[(&str, &str)]| {
        PluginUserConfiguration::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    };
    // as launched by the `c` keybinding, which goes through `Pty::spawn_plugin`
    let running = configuration(&[
        ("overlay", "true"),
        ("awaits_placement", "true"),
        ("shared", "true"),
        ("caller_cwd", "/repo"),
        ("caller_terminal_pane_id", "0"),
    ]);
    let at_this_location = HashMap::from([(running, vec![(PluginId::new(7), ClientId::new(1))])]);

    // as asked for by `agents`, whose message is stamped with `caller_cwd` alone
    let greeting = configuration(&[
        ("overlay", "true"),
        ("awaits_placement", "true"),
        ("shared", "true"),
        ("caller_cwd", "/somewhere/else"),
    ]);
    assert_eq!(
        instances_of(&at_this_location, &greeting),
        &[(PluginId::new(7), ClientId::new(1))],
        "the conversation that is already open"
    );

    // and a configuration that differs by something the user actually wrote is still a
    // different plugin
    let other = configuration(&[("overlay", "true"), ("shared", "true")]);
    assert!(
        instances_of(&at_this_location, &other).is_empty(),
        "an overlay that does not await placement is not the same plugin"
    );
}

#[test]
fn shared_plugins_receive_messages_directed_at_any_client() {
    let (plugin_id, owner_client_id, other_client_id) = (1, 1, 2);
    // a message for another client reaches a shared plugin, but not a per-client one
    assert!(PluginBridge::message_is_directed_at_plugin(
        Some(PluginId::new(plugin_id)),
        Some(ClientId::new(other_client_id)),
        &PluginId::new(plugin_id),
        &ClientId::new(owner_client_id),
        true,
    ));
    assert!(!PluginBridge::message_is_directed_at_plugin(
        Some(PluginId::new(plugin_id)),
        Some(ClientId::new(other_client_id)),
        &PluginId::new(plugin_id),
        &ClientId::new(owner_client_id),
        false,
    ));
    // a message for another plugin never reaches it
    assert!(!PluginBridge::message_is_directed_at_plugin(
        Some(PluginId::new(plugin_id + 1)),
        Some(ClientId::new(other_client_id)),
        &PluginId::new(plugin_id),
        &ClientId::new(owner_client_id),
        true,
    ));
    // broadcasts still reach it exactly as before
    assert!(PluginBridge::message_is_directed_at_plugin(
        None,
        None,
        &PluginId::new(plugin_id),
        &ClientId::new(owner_client_id),
        true,
    ));
}

/// The other side of the rule above: a toast outlives the turn that raised it, so the client
/// who clicks one is not always the client whose instance put it up. The single id an update
/// carries has to be the raiser to *reach* a per-client plugin, and the clicker to *act* as
/// the right person on a shared one - and one choice cannot be both.
#[test]
fn a_notification_click_names_the_raiser_only_when_that_is_what_reaches_it() {
    let (raiser, clicker) = (ClientId::new(2), ClientId::new(1));

    // shared: every client reaches the one instance, so the id is spent on the acting client
    assert_eq!(
        crate::client_for_notification_click(true, raiser, clicker),
        clicker,
        "the reveal belongs to whoever clicked"
    );
    // per-client: only the raiser's instance knows about the toast, and naming anybody else
    // drops the click silently
    assert_eq!(
        crate::client_for_notification_click(false, raiser, clicker),
        raiser,
    );
    assert!(PluginBridge::message_is_directed_at_plugin(
        Some(PluginId::new(1)),
        Some(crate::client_for_notification_click(false, raiser, clicker)),
        &PluginId::new(1),
        &raiser,
        false,
    ));

    // and the usual case - one client - is the same id either way
    assert_eq!(
        crate::client_for_notification_click(true, clicker, clicker),
        crate::client_for_notification_click(false, clicker, clicker),
    );
}
