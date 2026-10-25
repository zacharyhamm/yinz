use super::*;
use std::collections::HashMap;

fn create_mock_senders() -> ThreadSenders {
    ThreadSenders {
        should_silently_fail: true,
        ..Default::default()
    }
}

fn create_test_pane_groups() -> PaneGroups {
    PaneGroups::new(create_mock_senders())
}

fn create_test_screen_size() -> Size {
    Size { rows: 24, cols: 80 }
}

#[test]
fn get_client_pane_group_returns_empty_set_for_nonexistent_client() {
    let pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(999);

    let result = pane_groups.get_client_pane_group(&client_id);
    assert!(result.is_empty());
}

#[test]
fn get_client_pane_group_returns_correct_panes() {
    let mut pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(1);
    let pane_ids = vec![
        PaneId::Terminal(TerminalId::new(10)),
        PaneId::Plugin(PluginId::new(20)),
        PaneId::Terminal(TerminalId::new(30)),
    ];
    let screen_size = create_test_screen_size();

    for pane_id in &pane_ids {
        pane_groups.add_pane_id_to_group(*pane_id, screen_size, &client_id);
    }

    let result = pane_groups.get_client_pane_group(&client_id);
    assert_eq!(result.len(), 3);
    for pane_id in pane_ids {
        assert!(result.contains(&pane_id));
    }
}

/// The one thing `PaneGroups` does besides bookkeeping: opening the multiple-select
/// plugin the first time a client's group becomes non-empty. Every other test in this
/// module drops that instruction unobserved, so this is the only place the predicate is
/// connected to its effect - and the only place the cross-client scan in
/// `should_launch_plugin` is visible, because a single-client test cannot reach it.
#[test]
fn launching_the_plugin_sends_fill_plugin_cwd_to_pty() {
    use yinz_runtime::channels::{self, ChannelWithContext, SenderWithContext};
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let senders = ThreadSenders {
        to_pty: Some(SenderWithContext::new(to_pty)),
        ..Default::default()
    };
    let mut pane_groups = PaneGroups::new(senders);
    let screen_size = create_test_screen_size();
    let client_1: ClientId = ClientId::new(1);
    let client_2: ClientId = ClientId::new(2);
    let expected_coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(24 - (10 + 2))),
        width: Some(PercentOrFixed::Fixed(48)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: Some(true),
        borderless: Some(false),
    };
    let launched_for = |expected: &str| -> ClientId {
        match pty_receiver.try_recv() {
            Ok((
                PtyInstruction::FillPluginCwd(
                    should_float,
                    in_place,
                    _,
                    _,
                    run_plugin,
                    _,
                    _,
                    client_id,
                    _,
                    _,
                    _,
                    should_focus,
                    coordinates,
                    _,
                    _,
                ),
                _,
            )) => {
                assert_eq!(should_float, Some(true), "{expected}");
                assert!(!in_place, "{expected}");
                assert_eq!(should_focus, Some(false), "{expected}");
                assert_eq!(
                    coordinates,
                    Some(expected_coordinates.clone()),
                    "{expected}"
                );
                assert!(
                    format!("{run_plugin:?}").contains("multiple-select"),
                    "{expected}: launched {run_plugin:?}"
                );
                client_id
            },
            Ok((other, _)) => panic!("{expected}: got {other:?}"),
            Err(_) => panic!("{expected}: nothing was sent to the pty thread"),
        }
    };
    let nothing_launched = |why: &str| {
        assert!(
            pty_receiver.try_recv().is_err(),
            "{why}: the plugin was launched"
        );
    };

    // A client's first pane.
    pane_groups.add_pane_id_to_group(PaneId::Terminal(TerminalId::new(1)), screen_size, &client_1);
    assert_eq!(launched_for("a client's first pane"), client_1);
    pane_groups.add_pane_id_to_group(PaneId::Terminal(TerminalId::new(2)), screen_size, &client_1);
    nothing_launched("a non-empty group growing");

    // The scan is over every client, so a second client going empty->non-empty launches
    // for the first client's operation.
    pane_groups.toggle_pane_id_in_group(PaneId::Plugin(PluginId::new(3)), screen_size, &client_2);
    assert_eq!(launched_for("a second client's first pane"), client_2);
    pane_groups.clear_pane_group(&client_2);
    pane_groups.add_pane_id_to_group(PaneId::Terminal(TerminalId::new(2)), screen_size, &client_1);
    nothing_launched("client 2 is still empty");
    pane_groups.override_groups_with(HashMap::from([
        (client_1, vec![PaneId::Terminal(TerminalId::new(2))]),
        (client_2, vec![PaneId::Terminal(TerminalId::new(2))]),
    ]));
    pane_groups.clear_pane_group(&client_2);
    pane_groups.add_pane_id_to_group(PaneId::Terminal(TerminalId::new(4)), screen_size, &client_1);
    nothing_launched("client 2 stays empty across client 1's add");

    // For all clients: launched once, for a client that has a group.
    pane_groups.group_and_ungroup_panes_for_all_clients(
        vec![PaneId::Terminal(TerminalId::new(5))],
        vec![],
        screen_size,
    );
    let launched = launched_for("a group-for-all that fills an empty group");
    assert!([client_1, client_2].contains(&launched));
    nothing_launched("only one launch per operation");
    pane_groups.group_and_ungroup_panes_for_all_clients(
        vec![PaneId::Terminal(TerminalId::new(6))],
        vec![],
        screen_size,
    );
    nothing_launched("a group-for-all when every group is already non-empty");
}

#[test]
fn should_launch_plugin_returns_true_when_first_pane_added() {
    let pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(1);
    let previous_groups = HashMap::new();

    assert!(pane_groups.should_launch_plugin(&previous_groups, &client_id));
}

#[test]
fn should_launch_plugin_returns_true_when_empty_to_non_empty() {
    let mut pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(1);
    let screen_size = create_test_screen_size();

    let mut previous_groups = HashMap::new();
    previous_groups.insert(client_id, vec![]);

    pane_groups.add_pane_id_to_group(
        PaneId::Terminal(TerminalId::new(10)),
        screen_size,
        &client_id,
    );

    assert!(pane_groups.should_launch_plugin(&previous_groups, &client_id));
}

#[test]
fn should_launch_plugin_returns_false_when_non_empty_to_non_empty() {
    let mut pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(1);
    let screen_size = create_test_screen_size();

    pane_groups.add_pane_id_to_group(
        PaneId::Terminal(TerminalId::new(10)),
        screen_size,
        &client_id,
    );
    let previous_groups = pane_groups.clone_inner();

    pane_groups.add_pane_id_to_group(PaneId::Plugin(PluginId::new(20)), screen_size, &client_id);

    assert!(!pane_groups.should_launch_plugin(&previous_groups, &client_id));
}

#[test]
fn should_launch_plugin_returns_false_when_non_empty_to_empty() {
    let pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(1);

    let mut previous_groups = HashMap::new();
    previous_groups.insert(client_id, vec![PaneId::Terminal(TerminalId::new(10))]);

    assert!(!pane_groups.should_launch_plugin(&previous_groups, &client_id));
}

#[test]
fn should_launch_plugin_returns_false_when_empty_to_empty() {
    let pane_groups = create_test_pane_groups();
    let client_id: ClientId = ClientId::new(1);

    let mut previous_groups = HashMap::new();
    previous_groups.insert(client_id, vec![]);

    assert!(!pane_groups.should_launch_plugin(&previous_groups, &client_id));
}

// ---- property-based tests (hegel) ----

use hegel::generators;

/// Small pools on purpose: `PaneGroups` is a per-client set, so the interesting behaviour
/// (aliasing, re-adding, toggling something another rule already added) only shows up when the
/// same ids keep coming back.
#[hegel::composite]
fn any_client_id(tc: &hegel::TestCase) -> ClientId {
    ClientId::new(tc.draw(generators::integers::<u32>().max_value(3)))
}

#[hegel::composite]
fn any_pane_id(tc: &hegel::TestCase) -> PaneId {
    let n = tc.draw(generators::integers::<u32>().max_value(4));
    if tc.draw(generators::booleans()) {
        PaneId::Terminal(TerminalId::new(n))
    } else {
        PaneId::Plugin(PluginId::new(n))
    }
}

#[hegel::composite]
fn any_pane_ids(tc: &hegel::TestCase) -> Vec<PaneId> {
    let count = tc.draw(generators::integers::<usize>().max_value(4));
    (0..count).map(|_| tc.draw(any_pane_id())).collect()
}

struct PaneGroupsModel {
    real: PaneGroups,
    /// what the groups would be if every operation kept its per-client list duplicate-free
    model: HashMap<ClientId, Vec<PaneId>>,
    screen_size: Size,
}

impl PaneGroupsModel {
    fn push_unique(list: &mut Vec<PaneId>, pane_id: PaneId) {
        if !list.contains(&pane_id) {
            list.push(pane_id);
        }
    }
}

#[hegel::state_machine]
impl PaneGroupsModel {
    #[rule]
    fn toggle_pane_id_in_group(&mut self, tc: hegel::TestCase) {
        let pane_id = tc.draw(any_pane_id());
        let client_id = tc.draw(any_client_id());
        self.real
            .toggle_pane_id_in_group(pane_id, self.screen_size, &client_id);
        let group = self.model.entry(client_id).or_default();
        if group.contains(&pane_id) {
            group.retain(|p| p != &pane_id);
        } else {
            group.push(pane_id);
        }
    }

    #[rule]
    fn add_pane_id_to_group(&mut self, tc: hegel::TestCase) {
        let pane_id = tc.draw(any_pane_id());
        let client_id = tc.draw(any_client_id());
        self.real
            .add_pane_id_to_group(pane_id, self.screen_size, &client_id);
        Self::push_unique(self.model.entry(client_id).or_default(), pane_id);
    }

    #[rule]
    fn group_and_ungroup_panes(&mut self, tc: hegel::TestCase) {
        let to_group = tc.draw(any_pane_ids());
        let to_ungroup = tc.draw(any_pane_ids());
        let client_id = tc.draw(any_client_id());
        self.real.group_and_ungroup_panes(
            to_group.clone(),
            to_ungroup.clone(),
            self.screen_size,
            &client_id,
        );
        let group = self.model.entry(client_id).or_default();
        for pane_id in &to_group {
            Self::push_unique(group, *pane_id);
        }
        group.retain(|p| !to_ungroup.contains(p));
    }

    #[rule]
    fn group_and_ungroup_panes_for_all_clients(&mut self, tc: hegel::TestCase) {
        let to_group = tc.draw(any_pane_ids());
        let to_ungroup = tc.draw(any_pane_ids());
        self.real.group_and_ungroup_panes_for_all_clients(
            to_group.clone(),
            to_ungroup.clone(),
            self.screen_size,
        );
        // production only touches clients that already have an entry
        for group in self.model.values_mut() {
            for pane_id in &to_group {
                Self::push_unique(group, *pane_id);
            }
            group.retain(|p| !to_ungroup.contains(p));
        }
    }

    #[rule]
    fn clear_pane_group(&mut self, tc: hegel::TestCase) {
        let client_id = tc.draw(any_client_id());
        self.real.clear_pane_group(&client_id);
        if let Some(group) = self.model.get_mut(&client_id) {
            group.clear();
        }
    }

    #[rule]
    fn override_groups(&mut self, tc: hegel::TestCase) {
        // duplicate-free by construction - feeding duplicates in through the front door would
        // make the no-duplicates invariant a statement about the test, not about `PaneGroups`
        let client_count = tc.draw(generators::integers::<usize>().max_value(4));
        let mut new_groups: HashMap<ClientId, Vec<PaneId>> = HashMap::new();
        for _ in 0..client_count {
            let client_id = tc.draw(any_client_id());
            let mut group: Vec<PaneId> = vec![];
            for pane_id in tc.draw(any_pane_ids()) {
                Self::push_unique(&mut group, pane_id);
            }
            new_groups.insert(client_id, group);
        }
        self.real.override_groups_with(new_groups.clone());
        self.model = new_groups;
    }

    #[invariant]
    fn groups_are_duplicate_free_and_match_the_model(&self, _: hegel::TestCase) {
        let inner = self.real.clone_inner();
        for (client_id, group) in &inner {
            let as_set: HashSet<PaneId> = group.iter().copied().collect();
            assert_eq!(
                as_set.len(),
                group.len(),
                "client {} has duplicates in its group: {:?}",
                client_id,
                group
            );
            // As a `Vec`, not a set: `clone_inner` hands the group out in insertion
            // order and callers depend on it (`unit/screen_tests/mouse.rs` asserts an
            // exact ordering).
            let expected: &[PaneId] = self
                .model
                .get(client_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            assert_eq!(
                group.as_slice(),
                expected,
                "client {} group {:?} disagrees with the model {:?}",
                client_id,
                group,
                self.model
            );
        }
        let real_clients: HashSet<ClientId> = inner.keys().copied().collect();
        let model_clients: HashSet<ClientId> = self.model.keys().copied().collect();
        assert_eq!(
            real_clients, model_clients,
            "client set {:?} disagrees with the model {:?}",
            inner, self.model
        );
    }
}

#[hegel::test]
fn pane_groups_stay_duplicate_free_and_track_the_model(tc: hegel::TestCase) {
    let machine = PaneGroupsModel {
        real: create_test_pane_groups(),
        model: HashMap::new(),
        screen_size: create_test_screen_size(),
    };
    hegel::stateful::run(machine, tc);
}
