use super::*;
use std::time::{Duration, Instant};
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

const CLIENT: ClientId = ClientId::new(1);
const OTHER: ClientId = ClientId::new(2);
const TAB: TabId = TabId::new(0);
const SIZE: Size = Size {
    cols: 160,
    rows: 40,
};

fn pane(id: u32) -> PaneId {
    PaneId::Terminal(TerminalId::new(id))
}

fn agent(cwd: Option<&std::path::Path>) -> AgentInfo {
    AgentInfo {
        kind: "codex".into(),
        state: AgentState::Idle,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: cwd.map(|path| path.to_str().unwrap().into()),
        session_id: None,
        run_id: None,
    }
}

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .arg("init")
        .arg(dir.path())
        .output()
        .unwrap()
        .status
        .success());
    dir
}

struct Scene {
    screen: Screen,
    instructions: Receiver<(ScreenInstruction, ErrorContext)>,
    renders: Receiver<(ServerInstruction, ErrorContext)>,
    jobs: Receiver<(BackgroundJob, ErrorContext)>,
    grids: HashMap<ClientId, Grid>,
}

impl Scene {
    fn new() -> Self {
        let (mut screen, jobs) = screen_with_background_jobs(SIZE);
        new_tab(&mut screen, 1, TAB);
        screen
            .core
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .set_pane_frames(PaneFrameStyle::Full);
        let (to_screen, instructions) = channels::unbounded();
        screen.core.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
        let (to_server, renders) = channels::unbounded();
        screen.core.bus.senders.to_server = Some(SenderWithContext::new(to_server));
        Self {
            screen,
            instructions,
            renders,
            jobs,
            grids: HashMap::new(),
        }
    }

    fn add(&mut self, id: u32, placement: NewPanePlacement) {
        self.screen
            .core
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .new_pane(
                pane(id),
                Some(format!("agent{id}")),
                None,
                false,
                true,
                placement,
                Some(CLIENT),
                None,
            )
            .unwrap();
    }

    fn report(&mut self, id: u32, agent: Option<AgentInfo>) {
        super::super::agent_reports::apply_pane_agent_report(&mut self.screen, pane(id), agent)
            .unwrap();
    }

    fn focus(&mut self, id: u32, client: ClientId) {
        self.screen
            .core
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .focus_pane_with_id(pane(id), false, false, client)
            .unwrap();
    }

    fn render(&mut self) {
        self.screen.render_to_clients().unwrap();
        for (instruction, _) in self.renders.try_iter() {
            if let ServerInstruction::Render(Some(clients)) = instruction {
                for (client, output) in clients {
                    let grid = self
                        .grids
                        .entry(client)
                        .or_insert_with(|| grid_for_render_snapshot(SIZE));
                    take_snapshot_and_cursor_coordinates(&output, grid);
                }
            }
        }
    }

    fn text(&self, client: ClientId) -> String {
        format!("{:?}", self.grids.get(&client).unwrap())
    }

    fn wait_for_title(&mut self, title: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.render();
            if self.text(CLIENT).contains(title) {
                return;
            }
            let (instruction, _) = self
                .instructions
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("worktree title {title:?} did not render: {error:?}; roots={:?}; focus={:?}; output={}", self.screen.core.tabs[&TAB].agent_worktrees, self.screen.core.tabs[&TAB].get_active_pane_id(CLIENT), self.text(CLIENT)));
            if let ScreenInstruction::AgentWorktreeResolved {
                cwd,
                generation,
                root,
            } = instruction
            {
                drain_background_jobs(&self.jobs);
                self.screen
                    .agent_worktree_resolved(cwd, generation, root)
                    .unwrap();
                assert!(drain_background_jobs(&self.jobs).contains(&BackgroundJob::RenderToClients));
            }
        }
    }
}

#[test]
fn same_worktree_titles_follow_each_viewers_focus_and_clear_after_agent_changes() {
    let repo = repository();
    let nested = repo.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let mut scene = Scene::new();
    scene.add(2, NewPanePlacement::default());
    scene.add(3, NewPanePlacement::default());
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(2, Some(agent(Some(&nested))));
    scene.report(3, Some(agent(Some(repo.path()))));
    scene.screen.add_client(OTHER).unwrap();
    scene.focus(1, CLIENT);
    scene.focus(2, OTHER);
    scene.render();
    assert!(
        !scene.text(CLIENT).contains('👯'),
        "pending lookups have no indicator"
    );
    scene.wait_for_title("👯 [2]");
    assert!(scene.text(CLIENT).contains("👯 [3]"));
    assert!(!scene.text(CLIENT).contains("👯 [1]"));
    assert!(scene.text(OTHER).contains("👯 [1]"));
    assert!(!scene.text(OTHER).contains("👯 [2]"));
    scene.focus(3, CLIENT);
    scene.render();
    assert!(scene.text(CLIENT).contains("👯 [1]"));
    assert!(!scene.text(CLIENT).contains("👯 [3]"));

    let scans = scene.screen.core.worktree_scans.get();
    scene.screen.render().unwrap();
    scene.render();
    assert_eq!(
        scene.screen.core.worktree_scans.get(),
        scans,
        "idle repaint scanned agent directories"
    );
    let waiting = AgentInfo {
        state: AgentState::Waiting,
        wait_needs_a_person: true,
        ..agent(Some(repo.path()))
    };
    scene.report(1, Some(waiting));
    assert_eq!(
        scene.screen.core.worktree_scans.get(),
        scans,
        "a state-only report scanned agent directories"
    );
    scene.render();
    assert!(scene.text(CLIENT).contains("🔔 👯 [1]"));
    for replacement in [
        None,
        Some(agent(None)),
        Some(AgentInfo {
            state: AgentState::Exited,
            ..agent(Some(repo.path()))
        }),
        Some(agent(Some(&repo.path().join("missing")))),
    ] {
        scene.report(3, replacement);
        scene.render();
        assert!(!scene.text(CLIENT).contains('👯'));
        assert!(!scene.text(OTHER).contains("👯 [3]"));
        scene.report(3, Some(agent(Some(repo.path()))));
        scene.render();
        assert!(scene.text(CLIENT).contains("👯 [1]"));
    }
    scene
        .screen
        .set_pane_host(TerminalId::new(3), Some("remote".into()))
        .unwrap();
    assert!(!scene.screen.core.tabs[&TAB]
        .agent_worktrees
        .contains_key(&pane(3)));
    scene.render();
    assert!(!scene.text(CLIENT).contains('👯'));
    assert!(!scene.text(OTHER).contains("👯 [3]"));
    scene
        .screen
        .set_pane_host(TerminalId::new(3), None)
        .unwrap();
    scene.report(1, Some(agent(Some(repo.path()))));
    super::super::replay_pending_plugin_events(
        &mut scene.screen,
        vec![ScreenInstruction::HoldPane(
            pane(1),
            Some(0),
            RunCommand::default(),
        )],
    )
    .unwrap();
    scene.render();
    assert!(!scene.text(CLIENT).contains("👯 [1]"));
    scene.screen.close_pane_explicitly(pane(3), None);
    scene.render();
    assert!(!scene.screen.core.tabs[&TAB]
        .agent_worktrees
        .contains_key(&pane(3)));
}

#[test]
fn same_worktree_titles_cross_tiled_and_floating_layers_and_stack_lists() {
    for floating in [false, true] {
        for stacked in [false, true] {
            if floating && stacked {
                continue;
            }
            let repo = repository();
            let mut scene = Scene::new();
            if floating && !stacked {
                scene.add(3, NewPanePlacement::default());
                scene.focus(1, CLIENT);
            }
            *scene.screen.core.stacked_pane_list.borrow_mut() = stacked;
            scene.add(
                2,
                if stacked {
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: None,
                        borderless: None,
                    }
                } else if floating {
                    NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                        x: Some(PercentOrFixed::Fixed(10)),
                        y: Some(PercentOrFixed::Fixed(6)),
                        width: Some(PercentOrFixed::Fixed(60)),
                        height: Some(PercentOrFixed::Fixed(22)),
                        pinned: Some(true),
                        ..Default::default()
                    }))
                } else {
                    NewPanePlacement::default()
                },
            );
            scene.report(1, Some(agent(Some(repo.path()))));
            scene.report(2, Some(agent(Some(repo.path()))));
            scene.wait_for_title("👯 [1]");
            assert!(!scene.text(CLIENT).contains("👯 [2]"));
            scene.focus(1, CLIENT);
            scene.render();
            assert!(
                scene.text(CLIENT).contains("👯 [2]"),
                "floating={floating}, stacked={stacked}"
            );
            assert!(!scene.text(CLIENT).contains("👯 [1]"));
            if floating {
                scene.add(
                    4,
                    NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                        x: Some(PercentOrFixed::Fixed(90)),
                        y: Some(PercentOrFixed::Fixed(6)),
                        width: Some(PercentOrFixed::Fixed(60)),
                        height: Some(PercentOrFixed::Fixed(22)),
                        ..Default::default()
                    })),
                );
                scene.report(4, Some(agent(Some(repo.path()))));
                scene.render();
                assert!(scene.text(CLIENT).contains("👯 [2]"));
                assert!(!scene.text(CLIENT).contains("👯 [4]"));
                scene.focus(2, CLIENT);
                scene.render();
                assert!(scene.text(CLIENT).contains("👯 [4]"));
                assert!(!scene.text(CLIENT).contains("👯 [2]"));
                scene.focus(1, CLIENT);
            }
            scene.report(1, None);
            scene.render();
            assert!(!scene.text(CLIENT).contains('👯'));
        }
    }
}

#[test]
fn same_worktree_overlay_titles_follow_each_viewers_focus_and_clear_after_agent_changes() {
    let repo = repository();
    let mut scene = Scene::new();
    scene.screen.add_client(OTHER).unwrap();
    scene.add(
        2,
        NewPanePlacement::Overlay(yinz_types::data::OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(6)),
                width: Some(PercentOrFixed::Fixed(60)),
                height: Some(PercentOrFixed::Fixed(22)),
                ..Default::default()
            },
        ))),
    );
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(2, Some(agent(Some(repo.path()))));
    scene.focus(1, CLIENT);
    scene.focus(2, OTHER);
    scene.wait_for_title("👯 [2]");
    assert!(!scene.text(CLIENT).contains("👯 [1]"));
    assert!(scene.text(OTHER).contains("👯 [1]"));
    assert!(!scene.text(OTHER).contains("👯 [2]"));

    scene.focus(2, CLIENT);
    scene.render();
    assert!(!scene.text(CLIENT).contains("👯 [2]"));
    scene.focus(1, CLIENT);
    scene.render();
    assert!(scene.text(CLIENT).contains("👯 [2]"));

    scene.report(1, None);
    scene.render();
    assert!(!scene.text(CLIENT).contains('👯'));
    assert!(!scene.text(OTHER).contains('👯'));
}

#[test]
fn same_worktree_markers_follow_resolved_agents_across_tab_moves() {
    let repo = repository();
    for stack in [true, false] {
        let mut scene = Scene::new();
        scene.add(3, NewPanePlacement::default());
        scene.report(1, Some(agent(Some(repo.path()))));
        scene.report(3, Some(agent(Some(repo.path()))));
        scene.wait_for_title("👯 [1]");

        let source = TabId::new(1);
        new_tab(&mut scene.screen, 2, source);
        add_pane_to_tab(&mut scene.screen, source, pane(4), false);
        scene.report(2, Some(agent(Some(repo.path()))));
        assert!(scene.screen.core.tabs[&source]
            .agent_worktrees
            .contains_key(&pane(2)));

        let scans = scene.screen.core.worktree_scans.get();
        if stack {
            assert_eq!(
                scene.screen.stack_panes(vec![pane(1), pane(2)]),
                Some(pane(2))
            );
        } else {
            scene
                .screen
                .replace_pane_with_existing_pane(pane(3), pane(2), true, None)
                .unwrap();
        }
        assert_eq!(scene.screen.core.worktree_scans.get(), scans + 1);
        assert!(!scene.screen.core.tabs[&source]
            .agent_worktrees
            .contains_key(&pane(2)));
        assert!(scene.screen.core.tabs[&source].has_pane_with_pid(&pane(4)));

        for (focused, peer) in [(2, 1), (1, 2)] {
            scene
                .screen
                .focus_pane_with_id(pane(focused), false, false, CLIENT)
                .unwrap();
            scene.render();
            assert!(
                scene.text(CLIENT).contains(&format!("👯 [{peer}]")),
                "stack={stack}, focused={focused}"
            );
            assert!(!scene.text(CLIENT).contains(&format!("👯 [{focused}]")));
        }
    }
}

#[test]
fn same_worktree_marker_follows_an_in_place_agent_moved_from_the_web() {
    use super::agent_control::{web_agents_reference, web_agents_snapshot, web_tab_destination};
    use yinz_bus::web::WebManageAction;

    let repo = repository();
    let mut scene = Scene::new();
    scene.add(
        2,
        NewPanePlacement::InPlace {
            pane_id_to_replace: Some(pane(1)),
            close_replaced_pane: false,
            borderless: None,
        },
    );
    scene.add(4, NewPanePlacement::default());
    scene.report(
        2,
        Some(AgentInfo {
            session_id: Some("moved-conversation".into()),
            ..agent(Some(repo.path()))
        }),
    );
    scene.report(4, Some(agent(Some(repo.path()))));
    scene.focus(2, CLIENT);
    scene.wait_for_title("👯 [4]");
    assert!(scene.screen.core.tabs[&TAB]
        .agent_worktrees
        .contains_key(&pane(2)));

    let destination = TabId::new(1);
    new_tab(&mut scene.screen, 3, destination);
    scene
        .screen
        .core
        .tabs
        .get_mut(&destination)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Full);
    scene.report(3, Some(agent(Some(repo.path()))));
    let agents = web_agents_snapshot(&mut scene.screen);
    let moved = web_agents_reference(agents.iter().find(|row| row.pane.id == pane(2)).unwrap());
    let target = web_tab_destination(&scene.screen, destination);
    scene
        .screen
        .web_manage_agent(
            &moved,
            WebManageAction::Move(target),
            CLIENT,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();

    let source = &scene.screen.core.tabs[&TAB];
    assert!(source.has_non_suppressed_pane_with_pid(&pane(1)));
    assert!(!source.agent_worktrees.contains_key(&pane(2)));
    let roots = &scene.screen.core.tabs[&destination].agent_worktrees;
    assert!(roots.contains_key(&pane(2)));
    assert_eq!(roots.get(&pane(2)), roots.get(&pane(3)));
    assert_eq!(scene.screen.active_pane_id(CLIENT), Some(pane(3)));
    scene.render();
    let text = scene.text(CLIENT);
    assert!(text.contains("👯 [2]"), "{text}");
}

#[test]
fn same_worktree_surviving_overlay_is_published_in_its_destination() {
    let repo = repository();
    let nested = repo.path().join("overlay");
    std::fs::create_dir(&nested).unwrap();
    let mut scene = Scene::new();
    scene.add(
        2,
        NewPanePlacement::Overlay(yinz_types::data::OverlayPlacement::dismissible(None)),
    );
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(2, Some(agent(Some(&nested))));
    scene.focus(1, CLIENT);
    scene.wait_for_title("👯 [2]");
    let destination = TabId::new(1);
    new_tab(&mut scene.screen, 3, destination);
    scene.report(3, Some(agent(Some(repo.path()))));
    scene.screen.close_tab_by_id(TAB).unwrap();
    let roots = &scene.screen.core.tabs[&destination].agent_worktrees;
    assert!(roots.contains_key(&pane(3)));
    assert_eq!(roots.get(&pane(2)), roots.get(&pane(3)));
}

#[test]
fn same_worktree_departures_are_published_while_warped_tab_close_is_deferred() {
    let repo = repository();
    let mut scene = Scene::new();
    scene.add(2, NewPanePlacement::default());
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(2, Some(agent(Some(repo.path()))));
    scene.wait_for_title("👯 [1]");
    assert_eq!(scene.screen.core.tabs[&TAB].agent_worktrees.len(), 2);
    scene
        .screen
        .new_tab(TabId::new(1), (vec![], vec![]), None, Some(CLIENT))
        .unwrap();
    let scans = scene.screen.core.worktree_scans.get();
    let (tab, ids, prepared) = scene
        .screen
        .check_tab_can_be_warped(Some(TAB), CLIENT)
        .unwrap();
    let released = ids
        .into_iter()
        .map(|id| {
            let host = ReleasedPtyHost {
                host_id: yinz_types::internal_ids::SupervisorId::new(format!("host-{id}")),
                host_pid: yinz_types::internal_ids::ProcessId::new(1),
                control_socket: Some(PathBuf::from(format!("/tmp/warp-{id}.sock"))),
            };
            (id, Ok(released_for_warp(ReleasedPane::Local(host))))
        })
        .collect();
    let payload = scene
        .screen
        .take_released_tab_for_warp(tab, released, prepared)
        .unwrap();
    assert_eq!(payload.tiled_panes.len(), 2);
    assert!(scene.screen.core.deferred_tab_closes.contains(&TAB));
    assert!(scene.screen.core.tabs[&TAB].agent_worktrees.is_empty());
    assert_eq!(scene.screen.core.worktree_scans.get(), scans + 1);
}

#[test]
fn same_worktree_stale_completion_after_directory_change_does_not_restore_indicator() {
    let repo = repository();
    let elsewhere = repository();
    let mut scene = Scene::new();
    scene.add(2, NewPanePlacement::default());
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(2, Some(agent(Some(repo.path()))));
    scene.render();
    let (completion, _) = scene
        .instructions
        .recv_timeout(Duration::from_secs(15))
        .unwrap();
    scene.report(1, Some(agent(Some(elsewhere.path()))));
    scene.report(2, Some(agent(Some(elsewhere.path()))));
    if let ScreenInstruction::AgentWorktreeResolved {
        cwd,
        generation,
        root,
    } = completion
    {
        scene
            .screen
            .agent_worktree_resolved(cwd, generation, root)
            .unwrap();
    } else {
        panic!("expected a worktree completion")
    }
    scene.render();
    assert!(!scene.text(CLIENT).contains('👯'));
    scene.wait_for_title("👯 [1]");
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.render();
    assert!(
        !scene.text(CLIENT).contains('👯'),
        "a different checkout does not match"
    );
}

#[test]
fn same_worktree_marker_is_published_when_an_agent_opens_in_place() {
    use yinz_types::data::{OriginatingPlugin, AGENT_PANE_CONTEXT};
    let repo = repository();
    let mut scene = Scene::new();
    scene.add(2, NewPanePlacement::default());
    scene.add(3, NewPanePlacement::default());
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(2, Some(agent(Some(repo.path()))));
    scene.focus(1, CLIENT);
    scene.wait_for_title("👯 [2]");
    let scans = scene.screen.core.worktree_scans.get();

    // A plugin opens a marked agent, in the same checkout, in place of the ordinary pane.
    let run = Run::Command(RunCommand {
        command: PathBuf::from("codex"),
        cwd: Some(repo.path().to_path_buf()),
        originating_plugin: Some(OriginatingPlugin::new(
            yinz_types::internal_ids::PluginId::new(2),
            CLIENT,
            BTreeMap::from([(AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())]),
        )),
        ..Default::default()
    });
    assert!(scene.screen.replace_pane(
        pane(4),
        None,
        Some(run),
        None,
        yinz_bus::pty::ReplacedPane::Suppress,
        ClientTabIndexOrPaneId::PaneId(pane(3)),
        &mut None,
    ));
    assert_eq!(scene.screen.core.worktree_scans.get(), scans + 1);
    // The root already resolved, so membership is published without another lookup.
    let roots = &scene.screen.core.tabs[&TAB].agent_worktrees;
    assert!(roots.contains_key(&pane(4)), "roots={roots:?}");
    assert_eq!(roots.get(&pane(4)), roots.get(&pane(1)));
    assert!(scene.instructions.try_recv().is_err());

    scene.render();
    let text = scene.text(CLIENT);
    assert!(text.contains("👯 [2]") && text.contains("👯 [4]"), "{text}");
}

impl Scene {
    /// Mount `terminal` as slot 0 of a pane-tab group whose slot 1 holds `plugin`, and display
    /// the plugin or the terminal. Returns the plugin's run so `find_plugin` can match it.
    fn group_with_plugin(
        &mut self,
        tab: TabId,
        terminal: PaneId,
        plugin: crate::PluginId,
        show_plugin: bool,
    ) -> (
        RunPluginOrAlias,
        Receiver<(PluginInstruction, ErrorContext)>,
    ) {
        use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};
        // Building the plugin pane needs a plugin sender; keep its receiver alive for the test.
        let (to_plugin, plugins) = channels::unbounded();
        let to_plugin = SenderWithContext::new(to_plugin);
        self.screen.core.bus.senders.to_plugin = Some(to_plugin.clone());
        for tab in self.screen.core.tabs.values_mut() {
            tab.senders.to_plugin = Some(to_plugin.clone());
        }
        let location = "yinz:fixture-plugin-for-tests";
        let layout = PaneTabsLayout {
            active: 0,
            tabs: ["Agent", "Plugin"]
                .map(|label| PaneTabDescriptor {
                    label: label.into(),
                    location: location.into(),
                    initial_cwd: None,
                    configuration: BTreeMap::new(),
                })
                .to_vec(),
        };
        let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            location, &None, None, None,
        )
        .unwrap();
        let tab = self.screen.core.tabs.get_mut(&tab).unwrap();
        let info = tab.mount_pane_tabs(terminal, layout, 0, CLIENT).unwrap();
        tab.select_pane_tab(info.id, 1, CLIENT).unwrap();
        let generation = plugins
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::LoadPaneTab { generation, .. } => Some(generation),
                _ => None,
            })
            .expect("selecting the unloaded slot asks the plugin thread to load it");
        tab.pane_tab_loaded(info.id, 1, generation, Ok((plugin, run.clone())))
            .unwrap();
        assert_eq!(
            tab.pane_tabs_info(info.id).unwrap().pane_id,
            PaneId::Plugin(plugin),
            "the plugin slot loaded and is displayed"
        );
        if !show_plugin {
            tab.select_pane_tab(info.id, 0, CLIENT).unwrap();
            assert_eq!(tab.pane_tabs_info(info.id).unwrap().pane_id, terminal);
        }
        (run, plugins)
    }
}

#[test]
fn same_worktree_marker_follows_a_group_terminal_moved_by_plugin_focus() {
    let repo = repository();
    let mut scene = Scene::new();
    scene.add(3, NewPanePlacement::default());
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(3, Some(agent(Some(repo.path()))));
    scene.wait_for_title("👯 [1]");
    let plugin = crate::PluginId::new(40);
    // The plugin sits in the group's inactive slot, so `find_plugin` answers with the
    // displayed agent terminal and the move carries that terminal.
    let (run, _plugins) = scene.group_with_plugin(TAB, pane(1), plugin, false);

    let destination = TabId::new(1);
    new_tab(&mut scene.screen, 2, destination);
    scene.report(2, Some(agent(Some(repo.path()))));
    assert_eq!(scene.screen.core.active_tab_ids[&CLIENT], destination);

    assert!(scene
        .screen
        .focus_plugin_pane(&run, false, true, false, CLIENT, &mut None)
        .unwrap());
    assert!(scene.screen.core.tabs[&destination].has_pane_with_pid(&pane(1)));
    let roots = &scene.screen.core.tabs[&destination].agent_worktrees;
    assert!(
        roots.contains_key(&pane(1)) && roots.contains_key(&pane(2)),
        "the moved agent is published in its destination: {roots:?}"
    );
    assert!(!scene.screen.core.tabs[&TAB]
        .agent_worktrees
        .contains_key(&pane(1)));
}

#[test]
fn same_worktree_marker_includes_a_hidden_group_terminal_stacked_across_tabs() {
    let repo = repository();
    let mut scene = Scene::new();
    scene.add(3, NewPanePlacement::default());
    scene.report(1, Some(agent(Some(repo.path()))));
    scene.report(3, Some(agent(Some(repo.path()))));
    scene.wait_for_title("👯 [1]");

    let source = TabId::new(1);
    new_tab(&mut scene.screen, 2, source);
    add_pane_to_tab(&mut scene.screen, source, pane(4), false);
    scene.report(2, Some(agent(Some(repo.path()))));
    let plugin = crate::PluginId::new(40);
    // The plugin is displayed; the agent terminal travels hidden inside it.
    let _plugins = scene.group_with_plugin(source, pane(2), plugin, true);

    assert_eq!(
        scene
            .screen
            .stack_panes(vec![pane(1), PaneId::Plugin(plugin)]),
        Some(PaneId::Plugin(plugin))
    );
    assert!(scene.screen.core.tabs[&TAB].has_pane_with_pid(&pane(2)));
    let roots = &scene.screen.core.tabs[&TAB].agent_worktrees;
    assert!(
        roots.contains_key(&pane(2)),
        "the hidden agent is published in its destination before any render: {roots:?}"
    );
    assert!(!scene.screen.core.tabs[&source]
        .agent_worktrees
        .contains_key(&pane(2)));
}
