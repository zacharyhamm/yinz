//! Display-only agent status shared by the command palette and pane titles.
use yinz_types::{agent::AgentState, data::PaletteColor, style::StyleDeclaration};

pub fn status(state: AgentState) -> (&'static str, &'static str, Option<usize>) {
    match state {
        AgentState::Busy => ("●", "working", Some(2)),
        AgentState::Waiting => ("!", "needs input", Some(1)),
        AgentState::Idle => ("○", "idle", None),
        AgentState::Exited => ("×", "exited", None),
    }
}

pub const FLASH_COLOR: usize = 0;

pub fn color(colors: StyleDeclaration, emphasis: Option<usize>) -> PaletteColor {
    match emphasis {
        Some(0) => colors.emphasis_0,
        Some(1) => colors.emphasis_1,
        Some(2) => colors.emphasis_2,
        Some(_) => colors.emphasis_3,
        None => colors.base,
    }
}

/// Transient presentation; never part of the pane name or serialized pane tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentTitle {
    pub state: Option<AgentState>,
    pub hook_flash: bool,
}

impl AgentTitle {
    pub fn marker(self) -> Option<&'static str> {
        self.state.map(|state| status(state).0)
    }

    pub fn marker_color(self, colors: StyleDeclaration) -> PaletteColor {
        color(
            colors,
            if self.hook_flash {
                Some(FLASH_COLOR)
            } else {
                self.state.and_then(|state| status(state).2)
            },
        )
    }

    pub fn title_color(self, colors: StyleDeclaration) -> Option<PaletteColor> {
        self.hook_flash.then(|| color(colors, Some(FLASH_COLOR)))
    }
}
