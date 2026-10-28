use super::*;
use yinz_types::{
    agent::AgentState, internal_ids::TerminalId, pane_id::PaneId, pane_tabs::*, style::Style,
    tab_bar::TabTarget,
};

#[test]
fn pane_tabs_agent_titles_preserve_markers_colors_geometry_and_descriptors() {
    let info = PaneTabsInfo {
        overlay_group: false,
        id: PaneTabsId(1),
        pane_id: PaneId::Terminal(TerminalId::new(7)),
        active: 0,
        bar_focused: vec![],
        tabs: ["Conversation 界", "Notes"]
            .into_iter()
            .map(|label| PaneTabInfo {
                descriptor: PaneTabDescriptor {
                    label: label.into(),
                    location: String::new(),
                    configuration: Default::default(),
                    initial_cwd: None,
                },
                plugin_id: None,
                terminal_id: Some(TerminalId::new(7)),
                state: PaneTabLoadState::Ready,
            })
            .collect(),
    };
    let original = info.clone();
    let style = Style::default();
    for state in [
        AgentState::Busy,
        AgentState::Idle,
        AgentState::Waiting,
        AgentState::Exited,
    ] {
        for flash in [false, true] {
            for focused in [false, true] {
                let titles = [
                    AgentTitle {
                        state: Some(state),
                        hook_flash: flash,
                    },
                    AgentTitle::default(),
                ];
                let colors = if focused {
                    style.colors.text_selected
                } else {
                    style.colors.text_unselected
                };
                for width in 1..50 {
                    let cells =
                        bar_cells_with_titles(&info, Default::default(), focused, width, &titles);
                    let active = cells
                        .iter()
                        .find(|cell| cell.target == TabTarget::Tab(0))
                        .unwrap();
                    let chars =
                        render_bar(&info, Default::default(), focused, width, &style, &titles);
                    assert_eq!(chars.iter().map(|ch| ch.width()).sum::<usize>(), width);
                    assert!(active.text.starts_with(titles[0].marker().unwrap()));
                    assert_eq!(
                        chars[active.x].styles.foreground,
                        Some(titles[0].marker_color(colors).into())
                    );
                    if let Some(character) = chars.iter().find(|ch| ch.character == 'C')
                        && flash
                    {
                        assert_eq!(character.styles.foreground, Some(colors.emphasis_0.into()));
                    }
                }
            }
        }
    }
    assert_eq!(info, original);
}
