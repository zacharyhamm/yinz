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
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .set_pane_frames(PaneFrameStyle::Full);
        let (to_screen, instructions) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
        let (to_server, renders) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(to_server));
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
        self.screen
            .tabs
            .get_mut(&TAB)
            .unwrap()
            .get_pane_with_id_mut(pane(id))
            .unwrap()
            .set_agent_info(agent);
    }

    fn focus(&mut self, id: u32, client: ClientId) {
        self.screen
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
                .unwrap_or_else(|error| panic!("worktree title {title:?} did not render: {error:?}; roots={:?}; focus={:?}; output={}", self.screen.tabs[&TAB].agent_worktrees, self.screen.tabs[&TAB].get_active_pane_id(CLIENT), self.text(CLIENT)));
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

    let waiting = AgentInfo {
        state: AgentState::Waiting,
        wait_needs_a_person: true,
        ..agent(Some(repo.path()))
    };
    scene.report(1, Some(waiting));
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
        .terminal_hosts
        .insert(TerminalId::new(3), "remote".into());
    scene.render();
    assert!(!scene.text(CLIENT).contains('👯'));
    assert!(!scene.text(OTHER).contains("👯 [3]"));
    scene.screen.terminal_hosts.clear();
    scene.report(1, Some(agent(Some(repo.path()))));
    scene
        .screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .get_pane_with_id_mut(pane(1))
        .unwrap()
        .hold(Some(0), false, RunCommand::default());
    scene.render();
    assert!(!scene.text(CLIENT).contains("👯 [1]"));
    scene
        .screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .close_pane(pane(3), false, None);
    scene.render();
    assert!(!scene.screen.tabs[&TAB]
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
            *scene.screen.stacked_pane_list.borrow_mut() = stacked;
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
