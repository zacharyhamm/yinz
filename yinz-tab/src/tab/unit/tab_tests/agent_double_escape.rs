use super::*;
use std::time::{Duration, Instant};
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::keys::{BareKey, KeyWithModifier};

fn agent() -> AgentInfo {
    AgentInfo {
        kind: "codex".into(),
        state: AgentState::Waiting,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: true,
        cwd: None,
        session_id: None,
        run_id: None,
    }
}

#[test]
fn agent_double_escape_pairs_and_bypasses() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
    let rx = install_pty_writer_capture(&mut tab);
    let pane = PaneId::Terminal(TerminalId::new(1));
    let client = ClientId::new(1);
    let esc = Some(KeyWithModifier::new(BareKey::Esc));
    tab.set_pane_agent(pane, Some(agent()));
    let write = |tab: &mut Tab, key: &Option<KeyWithModifier>, bytes: &[u8], kitty, client| {
        tab.write_to_pane_id(key, bytes.to_vec(), kitty, pane, client, None)
            .unwrap();
    };
    write(&mut tab, &esc, b"\x1b", false, Some(client));
    assert_eq!(drain_pty_writer(&rx).len(), 1, "disabled by default");
    tab.agent_double_escape = true;
    tab.set_pane_agent(pane, Some(agent()));
    for kitty in [false, true] {
        let bytes: &[u8] = if kitty { b"\x1b[27u" } else { b"\x1b" };
        for _ in 0..2 {
            write(&mut tab, &esc, bytes, kitty, Some(client));
            assert!(drain_pty_writer(&rx).is_empty());
            write(&mut tab, &esc, bytes, kitty, Some(client));
            assert!(
                matches!(drain_pty_writer(&rx).as_slice(), [PtyWriteInstruction::Write(bytes, _, _)] if bytes == b"\x1b")
            );
        }
    }
    write(&mut tab, &esc, b"\x1b", false, Some(client));
    tab.pending_agent_escapes
        .insert((client, pane), Instant::now() - Duration::from_secs(1));
    write(&mut tab, &esc, b"\x1b", false, Some(client));
    assert!(
        drain_pty_writer(&rx).is_empty(),
        "expired first press is discarded"
    );
    let other = ClientId::new(2);
    write(&mut tab, &esc, b"\x1b", false, Some(other));
    assert!(
        drain_pty_writer(&rx).is_empty(),
        "clients have separate pairs"
    );
    write(&mut tab, &esc, b"\x1b", false, Some(client));
    assert_eq!(drain_pty_writer(&rx).len(), 1);
    write(
        &mut tab,
        &Some(KeyWithModifier::new(BareKey::Char('x'))),
        b"x",
        false,
        Some(other),
    );
    drain_pty_writer(&rx);
    write(&mut tab, &esc, b"\x1b", false, Some(other));
    assert!(drain_pty_writer(&rx).is_empty(), "another key resets");
    for (key, bytes, client) in [
        (None, b"\x1b".as_slice(), Some(client)),
        (None, b"\x1b[A".as_slice(), Some(client)),
        (esc.clone(), b"\x1b".as_slice(), None),
        (
            Some(KeyWithModifier::new(BareKey::Esc).with_alt_modifier()),
            b"\x1b\x1b".as_slice(),
            Some(client),
        ),
    ] {
        write(&mut tab, &key, bytes, false, client);
        assert_eq!(
            drain_pty_writer(&rx).len(),
            1,
            "unparsed/programmatic/modified input bypasses"
        );
    }
    tab.paste_to_pane_id(b"\x1b".to_vec(), pane, None, None)
        .unwrap();
    assert!(drain_pty_writer(&rx)
        .iter()
        .any(|msg| matches!(msg, PtyWriteInstruction::Write(bytes, _, _) if bytes == b"\x1b")));
    tab.set_pane_agent(pane, None);
    write(&mut tab, &esc, b"\x1b", false, Some(client));
    assert_eq!(drain_pty_writer(&rx).len(), 1, "ordinary terminals bypass");
}

#[test]
fn agent_double_escape_sync_preserves_ordinary_delivery_and_attention() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
    let client = ClientId::new(1);
    let agent_pane = PaneId::Terminal(TerminalId::new(1));
    tab.vertical_split(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        client,
        None,
        None,
    )
    .unwrap();
    tab.set_pane_agent(agent_pane, Some(agent()));
    tab.agent_double_escape = true;
    tab.toggle_sync_panes_is_active();
    let rx = install_pty_writer_capture(&mut tab);
    let esc = Some(KeyWithModifier::new(BareKey::Esc));
    tab.write_to_terminals_on_current_tab(&esc, vec![27], false, client)
        .unwrap();
    assert!(
        matches!(drain_pty_writer(&rx).as_slice(), [PtyWriteInstruction::Write(_, id, _)] if *id == TerminalId::new(2))
    );
    assert!(tab.has_waiting_agent());
    tab.write_to_terminals_on_current_tab(&esc, vec![27], false, client)
        .unwrap();
    assert_eq!(drain_pty_writer(&rx).len(), 2);
    assert!(tab.has_waiting_agent());
}

#[test]
fn agent_double_escape_leaves_guest_menus_and_plugins_alone() {
    use crate::plugins::PluginInstruction;
    use crate::screen::ScreenInstruction;
    use yinz_types::data::Event;
    let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
    let client = ClientId::new(1);
    let pane = PaneId::Terminal(TerminalId::new(1));
    let esc = Some(KeyWithModifier::new(BareKey::Esc));
    tab.agent_double_escape = true;
    tab.set_pane_agent(pane, Some(agent()));
    tab.set_guest_modal_on_pane(pane, &[client]);
    let (tx, rx) = unbounded();
    tab.senders.to_screen = Some(SenderWithContext::new(tx));
    tab.write_to_pane_id(&esc, vec![27], false, pane, Some(client), None)
        .unwrap();
    assert!(rx
        .try_iter()
        .any(|(msg, _)| matches!(msg, ScreenInstruction::GuestModalChoice { .. })));
    assert!(tab.pending_agent_escapes.is_empty());

    let (tx, rx) = unbounded();
    tab.senders.replace_to_plugin(SenderWithContext::new(tx));
    let plugin = PaneId::Plugin(crate::PluginId::new(3));
    tab.new_pane(
        plugin,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    for _ in 0..2 {
        tab.write_to_pane_id(&esc, vec![27], false, plugin, Some(client), None)
            .unwrap();
    }
    let keys = rx
        .try_iter()
        .filter_map(|(msg, _)| match msg {
            PluginInstruction::Update(events) => Some(events),
            _ => None,
        })
        .flatten()
        .filter(|(_, _, event)| matches!(event, Event::Key(key) if key.bare_key == BareKey::Esc))
        .count();
    assert_eq!(keys, 2);
    assert!(tab.pending_agent_escapes.is_empty());
}

#[test]
fn agent_double_escape_hint_tracks_pending_input_per_client() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, true);
    let pane = PaneId::Terminal(TerminalId::new(1));
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    tab.set_pane_agent(pane, Some(agent()));
    tab.agent_double_escape = true;
    let render = |tab: &mut Tab, client| {
        let mut output = yinz_terminal::output::Output::default();
        tab.render(&mut output, Some(client)).unwrap();
        output
            .serialize()
            .unwrap()
            .remove(&client)
            .unwrap_or_default()
    };
    let esc = Some(KeyWithModifier::new(BareKey::Esc));
    for cancel in [false, true] {
        assert!(tab
            .write_to_pane_id(&esc, vec![27], false, pane, Some(client), None)
            .unwrap());
        assert!(render(&mut tab, client).contains("Esc again"));
        assert!(!render(&mut tab, other).contains("Esc again"));
        let key = if cancel {
            Some(KeyWithModifier::new(BareKey::Char('x')))
        } else {
            esc.clone()
        };
        assert!(tab
            .write_to_pane_id(&key, vec![27], false, pane, Some(client), None)
            .unwrap());
        let restored = render(&mut tab, client);
        assert!(!restored.is_empty(), "clearing the hint repaints the frame");
        assert!(!restored.contains("Esc again"));
    }
    tab.write_to_pane_id(&esc, vec![27], false, pane, Some(client), None)
        .unwrap();
    assert!(render(&mut tab, client).contains("Esc again"));
    let start = tab.pending_agent_escapes[&(client, pane)];
    tab.expire_pending_agent_escapes(start + Duration::from_secs(1));
    let restored = render(&mut tab, client);
    assert!(!restored.is_empty());
    assert!(!restored.contains("Esc again"));
}
