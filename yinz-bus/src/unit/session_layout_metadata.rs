use super::*;
use yinz_types::pane_size::Dimension;
use yinz_types::pane_tabs::*;

#[test]
fn pane_tabs_metadata_roundtrip_omits_terminal_backed_wrappers() {
    for terminal_member in [true, false] {
        for active in 0..2 {
            for floating in [false, true] {
                let tabs = PaneTabsInfo {
                    id: PaneTabsId(1),
                    pane_id: if terminal_member && active == 0 {
                        PaneId::Terminal(TerminalId::new(1))
                    } else {
                        PaneId::Plugin(PluginId::new(active as u32 + 1))
                    },
                    tabs: ["Conversation", "Overview"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| PaneTabInfo {
                            descriptor: PaneTabDescriptor {
                                label: label.into(),
                                location: "yinz:session-manager".into(),
                                configuration: BTreeMap::from([("custom".into(), "value".into())]),
                                initial_cwd: Some("/project/overlay".into()),
                            },
                            plugin_id: (!(terminal_member && index == 0))
                                .then_some(PluginId::new(index as u32 + 1)),
                            terminal_id: (terminal_member && index == 0)
                                .then_some(TerminalId::new(1)),
                            state: PaneTabLoadState::Ready,
                        })
                        .collect(),
                    active,
                    bar_focused: vec![],
                };
                let run = if terminal_member && active == 0 {
                    Run::Command(RunCommand {
                        command: "claude".into(),
                        args: vec!["--resume".into()],
                        cwd: Some("/project/overlay".into()),
                        hold_on_start: true,
                        hold_on_close: true,
                        ..Default::default()
                    })
                } else {
                    Run::Plugin(
                        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                            "yinz:session-manager",
                            &None,
                            None,
                            None,
                        )
                        .unwrap(),
                    )
                };
                let pane = PaneLayoutMetadata::new(
                    tabs.pane_id,
                    PaneGeom {
                        rows: Dimension::fixed(24),
                        cols: Dimension::fixed(80),
                        ..Default::default()
                    },
                    false,
                    Some(run.clone()),
                    None,
                    true,
                    None,
                    vec![],
                    None,
                    None,
                )
                .with_pane_tabs(Some(&tabs));
                let mut metadata = SessionLayoutMetadata::default();
                let (tiled, floats) = if floating {
                    (vec![], vec![pane])
                } else {
                    (vec![pane], vec![])
                };
                metadata.add_tab("overlay".into(), true, false, false, tiled, floats);
                let (source, _) =
                    yinz_session::session_serialization::serialize_session_layout(metadata.into())
                        .unwrap();
                let layout = yinz_config::kdl::layout::from_kdl(&source, None, None, None)
                    .unwrap_or_else(|error| panic!("{error:?}\n{source}"));
                let (_, tiled, floats) = &layout.tabs[0];
                let (restored_tabs, restored_run) = if floating {
                    (&floats[0].pane_tabs, &floats[0].run)
                } else {
                    (&tiled.children[0].pane_tabs, &tiled.children[0].run)
                };
                if terminal_member {
                    assert_eq!(restored_tabs, &None, "{source}");
                    assert_eq!(restored_run, &Some(run), "{source}");
                } else {
                    assert_eq!(
                        restored_tabs,
                        &Some(PaneTabsLayout {
                            active,
                            tabs: tabs.tabs.into_iter().map(|slot| slot.descriptor).collect(),
                        }),
                        "{source}"
                    );
                }
            }
        }
    }
}
