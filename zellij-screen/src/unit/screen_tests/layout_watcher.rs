use super::*;
use std::time::Instant;
use zellij_types::data::{LayoutInfo, LayoutWithError};

fn wait_for_layouts(
    screen: &mut Screen,
    receiver: &Receiver<(ScreenInstruction, ErrorContext)>,
    expected: impl Fn(&[LayoutInfo], &[LayoutWithError]) -> bool,
) {
    let deadline = Instant::now() + zellij_test_support::default_timeout();
    loop {
        let (instruction, _) = receiver
            .recv_deadline(deadline)
            .expect("layout watcher did not report the expected layouts before the deadline");
        if let ScreenInstruction::UpdateAvailableLayouts {
            generation,
            layouts,
            errors,
        } = instruction
        {
            let matches =
                generation == screen.layout_watcher_generation && expected(&layouts, &errors);
            screen.update_available_layouts(generation, layouts, errors);
            if matches {
                return;
            }
        }
    }
}

fn layout_updates(
    receiver: &Receiver<(PluginInstruction, ErrorContext)>,
) -> Vec<(Vec<LayoutInfo>, Vec<LayoutWithError>)> {
    receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::LayoutListUpdate(layouts, errors) => Some((layouts, errors)),
            _ => None,
        })
        .collect()
}

fn assert_layouts_published(
    screen: &Screen,
    receiver: &Receiver<(PluginInstruction, ErrorContext)>,
) {
    assert_eq!(
        layout_updates(receiver).last(),
        Some(&(
            screen.cached_layouts.clone(),
            screen.cached_layout_errors.clone()
        ))
    );
}

#[test]
fn layout_watcher_retargets_and_rejects_stale_updates() {
    let old_dir = tempfile::tempdir().unwrap();
    let new_dir = tempfile::tempdir().unwrap();
    std::fs::write(old_dir.path().join("alpha.kdl"), "layout { pane; }").unwrap();
    std::fs::write(old_dir.path().join("old-broken.kdl"), "layout {").unwrap();
    std::fs::write(new_dir.path().join("beta.kdl"), "layout { pane; }").unwrap();
    std::fs::write(new_dir.path().join("broken.kdl"), "layout {").unwrap();
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    let (to_screen, screen_rx) = channels::unbounded();
    let (to_plugin, plugin_rx) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    screen.layout_dir = Some(old_dir.path().to_owned());
    screen.default_layout_name = Some("alpha".into());
    screen.restart_layout_watcher();
    assert!(
        screen.layout_watcher.is_none(),
        "a config file enables watching"
    );
    screen.config_file_path = Some(old_dir.path().join("config.kdl"));
    screen.restart_layout_watcher();
    // The initial callback is sent after watch registration, so subsequent edits are observable.
    wait_for_layouts(&mut screen, &screen_rx, |layouts, _| {
        layouts[0].name() == "alpha"
    });
    assert_layouts_published(&screen, &plugin_rx);
    let old_generation = screen.layout_watcher_generation;
    let old_layouts = screen.cached_layouts.clone();
    let old_errors = screen.cached_layout_errors.clone();

    let mut config = test_screen_config();
    config.layout_dir = Some(new_dir.path().to_owned());
    config.default_layout_name = Some("beta".into());
    screen
        .reconfigure(config.clone(), Keybinds::default(), ClientId::new(1))
        .unwrap();
    assert_eq!(screen.cached_layouts[0].name(), "beta");
    assert_eq!(screen.cached_layout_errors[0].layout_name, "broken");
    assert_layouts_published(&screen, &plugin_rx);
    wait_for_layouts(&mut screen, &screen_rx, |layouts, errors| {
        layouts[0].name() == "beta" && errors.len() == 1
    });

    std::fs::write(old_dir.path().join("old-only.kdl"), "layout { pane; }").unwrap();
    std::fs::remove_file(new_dir.path().join("broken.kdl")).unwrap();
    std::fs::write(new_dir.path().join("gamma.kdl"), "layout { pane; }").unwrap();
    wait_for_layouts(&mut screen, &screen_rx, |layouts, errors| {
        layouts.iter().any(|layout| layout.name() == "gamma") && errors.is_empty()
    });
    assert!(!screen
        .cached_layouts
        .iter()
        .any(|layout| layout.name() == "old-only"));
    assert_layouts_published(&screen, &plugin_rx);
    let current_layouts = screen.cached_layouts.clone();
    screen.update_available_layouts(old_generation, old_layouts.clone(), old_errors.clone());
    assert_eq!(screen.cached_layouts, current_layouts);
    assert!(screen.cached_layout_errors.is_empty());
    assert!(layout_updates(&plugin_rx).is_empty());

    // Returning to A must still reject a queued result from A's original watcher.
    config.layout_dir = Some(old_dir.path().to_owned());
    config.default_layout_name = Some("alpha".into());
    screen
        .reconfigure(config.clone(), Keybinds::default(), ClientId::new(1))
        .unwrap();
    assert_layouts_published(&screen, &plugin_rx);
    let current_layouts = screen.cached_layouts.clone();
    screen.update_available_layouts(old_generation, old_layouts, old_errors);
    assert_eq!(screen.cached_layouts, current_layouts);
    assert!(layout_updates(&plugin_rx).is_empty());

    config.default_layout_name = Some("compact".into());
    screen
        .reconfigure(config.clone(), Keybinds::default(), ClientId::new(1))
        .unwrap();
    assert_eq!(screen.cached_layouts[0].name(), "compact");
    assert_layouts_published(&screen, &plugin_rx);
    wait_for_layouts(&mut screen, &screen_rx, |layouts, _| {
        layouts[0].name() == "compact"
    });
    let generation = screen.layout_watcher_generation;
    let task_id = screen.layout_watcher.as_ref().unwrap().0.id();
    screen
        .reconfigure(config, Keybinds::default(), ClientId::new(1))
        .unwrap();
    assert_eq!(screen.layout_watcher_generation, generation);
    assert_eq!(screen.layout_watcher.as_ref().unwrap().0.id(), task_id);

    drop(screen);
    // Every watcher owns a sender. Disconnection proves teardown cancelled all of them,
    // including the replaced watchers, after draining any already queued results.
    let deadline = Instant::now() + zellij_test_support::default_timeout();
    loop {
        match screen_rx.recv_deadline(deadline) {
            Ok(_) => {},
            Err(channels::RecvTimeoutError::Disconnected) => break,
            Err(error) => panic!("layout watchers survived screen teardown: {error}"),
        }
    }
}

#[test]
fn layout_watcher_reports_a_directory_created_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let layouts_dir = dir.path().join("layouts");
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    let (to_screen, screen_rx) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    screen.config_file_path = Some(dir.path().join("config.kdl"));
    let mut config = test_screen_config();
    config.layout_dir = Some(layouts_dir.clone());
    config.default_layout_name = Some("later".into());
    screen
        .reconfigure(config, Keybinds::default(), ClientId::new(1))
        .unwrap();
    assert!(!screen
        .cached_layouts
        .iter()
        .any(|layout| layout.name() == "later"));
    std::fs::create_dir(&layouts_dir).unwrap();
    std::fs::write(layouts_dir.join("later.kdl"), "layout { pane; }").unwrap();
    wait_for_layouts(&mut screen, &screen_rx, |layouts, _| {
        layouts[0].name() == "later"
    });
}

#[test]
fn layout_watcher_reload_uses_the_default_directory_when_unset() {
    let dir = tempfile::tempdir().unwrap();
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    screen.layout_dir = Some(dir.path().to_owned());
    let config = test_screen_config();
    let expected = zellij_config::layout::list_available_layouts(
        zellij_runtime::home::default_layout_dir(),
        &config.default_layout_name,
    );
    screen
        .reconfigure(config, Keybinds::default(), ClientId::new(1))
        .unwrap();
    assert_eq!(
        (screen.cached_layouts, screen.cached_layout_errors),
        expected
    );
}
