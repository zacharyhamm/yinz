use super::*;
use yinz_types::{
    internal_ids::{PluginId, TabId, TerminalId},
    pane_id::PaneId,
    session_migration::*,
};

#[test]
fn cold_release_discards_destination_mappings_and_preserves_terminal_state() {
    let root = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let store = Store::open(&root.path().join("session"), root.path(), &operation).unwrap();
    let tab = TabId::new(1);
    let terminal = PaneId::Terminal(TerminalId::new(1));
    let plugin = PaneId::Plugin(PluginId::new(2));
    let snapshot = SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: store.journal.operation.clone(),
        inventory: SessionMigrationInventory {
            format: SESSION_MIGRATION_FORMAT,
            session_name: "session".into(),
            instance_id: "source".into(),
            tabs: vec![TabInventory {
                tab_id: tab,
                title: "retained".into(),
                panes: vec![terminal, plugin],
            }],
            clients: vec![],
            remote_hosts: vec![],
            global_last_active_tab: tab,
            tab_group_preferences: vec![],
            paneless_plugins: vec![],
            display_size: None,
        },
        phase: MigrationPhase::Adopted,
        tabs: vec![MigrationTab {
            source_tab_id: tab,
            slots: vec![terminal, plugin],
            payload: Default::default(),
            terminal_states: vec![(terminal, serde_json::json!({"retained": "terminal state"}))],
            local_replay: vec![(terminal, b"retained output".to_vec())],
            state: TabState {
                focus_pane: Some(plugin),
                fullscreen: Some(plugin),
                ..Default::default()
            },
        }],
        mapping: SessionMigrationMap {
            tabs: vec![(tab, TabId::new(9))],
            panes: vec![
                (terminal, PaneId::Terminal(TerminalId::new(8))),
                (plugin, PaneId::Plugin(PluginId::new(9))),
            ],
        },
        errors: vec!["replacement exited".into()],
        ownership: vec![(
            terminal,
            MigrationOwnership::Destination {
                terminal_id: TerminalId::new(8),
            },
        )],
    };
    journal::atomic_json(&store.journal.snapshot, &snapshot).unwrap();
    release_cold_snapshot(&store).unwrap();
    let released = read_recovery_snapshot(&store.journal.snapshot).unwrap();
    assert_eq!(released.phase, MigrationPhase::Released);
    assert_eq!(released.mapping, SessionMigrationMap::default());
    assert!(released.errors.is_empty());
    assert_eq!(
        released.ownership,
        vec![(terminal, MigrationOwnership::Released)]
    );
    assert_eq!(
        serde_json::to_value(&released.tabs).unwrap(),
        serde_json::to_value(&snapshot.tabs).unwrap()
    );
    assert_eq!(released.inventory, snapshot.inventory);
}
