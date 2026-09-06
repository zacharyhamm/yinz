//! `zellij events`: CLI clients subscribed to the session's state events on the plugin thread.

use super::*;
use crate::plugins::CLI_EVENTS_ONLY_TARGET;
use crate::ClientId;
use std::path::PathBuf;
use zellij_test_support::{default_timeout, Watched};
use {
    zellij_types::data::InputMode, zellij_types::data::ModeInfo, zellij_types::data::PaneId,
    zellij_types::data::PaneManifest,
};

fn forwarded(log: &[ServerInstruction]) -> Vec<(ClientId, String, String)> {
    log.iter()
        .filter_map(|instruction| match instruction {
            ServerInstruction::EventUpdateToClient(client_id, event_type, event_json) => {
                Some((*client_id, event_type.clone(), event_json.clone()))
            },
            _ => None,
        })
        .collect()
}

fn cwd_changed(pane: u32) -> Event {
    Event::CwdChanged(
        PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(pane)),
        PathBuf::from("/tmp"),
        vec![ClientId::new(1)],
    )
}

/// A subscriber gets what it asked for, once per batch for a state report however many plugins
/// the report was addressed to, and never an event outside the streamable set.
#[test]
#[serial(plugin_tests)]
pub fn a_cli_subscriber_gets_the_events_it_asked_for_once_each() {
    let (plugin_thread_sender, server_receiver, _screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(None, None);
    let received = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received,
        ServerInstruction::EventUpdateToClient,
        server_receiver,
        3
    );

    let client_id: ClientId = ClientId::new(7);
    let _ = plugin_thread_sender.send(PluginInstruction::SubscribeCliClientToEvents {
        client_id,
        event_types: vec!["pane_update".to_owned(), "CwdChanged".to_owned()],
    });
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![
        (
            Some(crate::PluginId::new(1)),
            Some(crate::ClientId::new(1)),
            Event::PaneUpdate(PaneManifest::default()),
        ),
        (
            Some(crate::PluginId::new(2)),
            Some(crate::ClientId::new(1)),
            Event::PaneUpdate(PaneManifest::default()),
        ),
        (
            Some(CLI_EVENTS_ONLY_TARGET),
            None,
            Event::PaneUpdate(PaneManifest::default()),
        ),
        (None, None, cwd_changed(3)),
        (
            None,
            None,
            Event::CommandPaneExited(
                zellij_types::internal_ids::TerminalId::new(3),
                Some(0),
                Default::default(),
            ),
        ),
        (None, Some(crate::ClientId::new(1)), Event::InputReceived),
    ]));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        None,
        cwd_changed(4),
    )]));

    let events = received.wait_until("three forwarded events", default_timeout(), |log| {
        let events = forwarded(log);
        (events.len() >= 3).then_some(events)
    });
    teardown();
    server_thread.join().unwrap();

    let types: Vec<&str> = events.iter().map(|(_, t, _)| t.as_str()).collect();
    assert_eq!(types, vec!["PaneUpdate", "CwdChanged", "CwdChanged"]);
    assert!(events.iter().all(|(id, _, _)| *id == client_id));
    for (_, event_type, event_json) in &events {
        let value: serde_json::Value = serde_json::from_str(event_json).unwrap();
        assert!(
            value.get(event_type).is_some(),
            "the payload is the serde form of the event, keyed by its name: {event_json}"
        );
    }
}

/// Only a run of consecutive state reports collapses to its newest: a report is never moved
/// past a non-report event, or a key would be forwarded ahead of the mode it was pressed in.
#[test]
#[serial(plugin_tests)]
pub fn state_reports_collapse_per_run_without_reordering_other_events() {
    let (plugin_thread_sender, server_receiver, _screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(None, None);
    let received = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received,
        ServerInstruction::EventUpdateToClient,
        server_receiver,
        4
    );
    let mode = |mode: InputMode| {
        Event::ModeUpdate(ModeInfo {
            mode,
            ..Default::default()
        })
    };

    let client_id: ClientId = ClientId::new(7);
    let _ = plugin_thread_sender.send(PluginInstruction::SubscribeCliClientToEvents {
        client_id,
        event_types: vec!["mode_update".to_owned(), "cwd_changed".to_owned()],
    });
    // one batch, as the plugin thread would merge two `Update`s waiting on its bus into anyway
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![
        (
            Some(crate::PluginId::new(1)),
            Some(crate::ClientId::new(1)),
            mode(InputMode::Locked),
        ),
        (None, None, cwd_changed(3)),
        (
            Some(crate::PluginId::new(1)),
            Some(crate::ClientId::new(1)),
            mode(InputMode::Locked),
        ),
        (
            Some(crate::PluginId::new(2)),
            Some(crate::ClientId::new(1)),
            mode(InputMode::Normal),
        ),
        (None, None, cwd_changed(4)),
    ]));

    let events = received.wait_until("four forwarded events", default_timeout(), |log| {
        let events = forwarded(log);
        (events.len() >= 4).then_some(events)
    });
    teardown();
    server_thread.join().unwrap();

    let types: Vec<&str> = events.iter().map(|(_, t, _)| t.as_str()).collect();
    assert_eq!(
        types,
        vec!["ModeUpdate", "CwdChanged", "ModeUpdate", "CwdChanged"],
        "{events:?}"
    );
    let modes: Vec<&str> = events
        .iter()
        .filter(|(_, t, _)| t == "ModeUpdate")
        .map(|(_, _, json)| {
            if json.contains("Locked") {
                "Locked"
            } else {
                "Normal"
            }
        })
        .collect();
    assert_eq!(
        modes,
        vec!["Locked", "Normal"],
        "the first report stayed ahead of the cwd change; the run of two collapsed to its newest"
    );
}

/// A subscriber that has gone is forgotten with it; the ones still here are unaffected.
#[test]
#[serial(plugin_tests)]
pub fn a_departed_cli_subscriber_stops_receiving_events() {
    let (plugin_thread_sender, server_receiver, _screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(None, None);
    let received = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received,
        ServerInstruction::EventUpdateToClient,
        server_receiver,
        1
    );

    for client_id in [7, 8] {
        let _ = plugin_thread_sender.send(PluginInstruction::SubscribeCliClientToEvents {
            client_id: crate::ClientId::new(client_id),
            event_types: vec![],
        });
    }
    let _ = plugin_thread_sender.send(PluginInstruction::RemoveClient(crate::ClientId::new(7)));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        None,
        cwd_changed(3),
    )]));

    let events = received.wait_until("one forwarded event", default_timeout(), |log| {
        let events = forwarded(log);
        (!events.is_empty()).then_some(events)
    });
    teardown();
    server_thread.join().unwrap();

    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].0,
        crate::ClientId::new(8),
        "only the remaining subscriber hears it"
    );
}

/// A name that is no streamable event refuses the subscription and says what would have been.
#[test]
#[serial(plugin_tests)]
pub fn an_unknown_event_type_is_refused_with_the_streamable_ones() {
    let (plugin_thread_sender, server_receiver, _screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(None, None);
    let received = Watched::new(vec![]);
    let server_thread =
        log_actions_in_thread!(received, ServerInstruction::LogError, server_receiver, 1);

    let _ = plugin_thread_sender.send(PluginInstruction::SubscribeCliClientToEvents {
        client_id: crate::ClientId::new(7),
        event_types: vec!["Key".to_owned()],
    });
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        None,
        cwd_changed(3),
    )]));

    let refusal = received.wait_until("a refusal", default_timeout(), |log| {
        log.iter().find_map(|instruction| match instruction {
            ServerInstruction::LogError(lines, client_id, _) => Some((lines.clone(), *client_id)),
            _ => None,
        })
    });
    teardown();
    server_thread.join().unwrap();

    assert_eq!(refusal.1, crate::ClientId::new(7));
    assert!(refusal.0[0].contains("'Key'"), "{:?}", refusal.0);
    assert!(refusal.0[0].contains("PaneUpdate"), "{:?}", refusal.0);
    assert!(
        forwarded(&received.get()).is_empty(),
        "nothing was streamed"
    );
}
