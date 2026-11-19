use super::super::*;

impl super::super::AgentController {
    /// Whether the pane holds the launcher's verified Muse hook capability, still waiting on
    /// the provider's first turn to bind a conversation. Claimed by the marker alone.
    pub(in crate::screen) fn has_provisional_muse_lifecycle(&self, pane: PaneId) -> bool {
        self.agent_lifecycle
            .get(&pane)
            .is_some_and(|lifecycle| lifecycle.kind == "muse" && lifecycle.conversation.is_empty())
    }
}

pub(in crate::screen::agent_control) fn muse_composer(
    host: &impl AgentHost,
    pane_id: PaneId,
    require_empty: bool,
) -> bool {
    let Some((columns, contents)) = host.viewport(pane_id, false) else {
        return false;
    };
    let Some((x, y)) = contents.cursor.map(|cursor| cursor.into_tuple()) else {
        return false;
    };
    if contents.is_scrolled || y >= contents.viewport.len() {
        return false;
    }
    let separator =
        |line: &String| line.chars().count() == columns && line.chars().all(|c| c == '─');
    if separator(&contents.viewport[y]) {
        return false;
    }
    let Some(top) = contents.viewport[..y].iter().rposition(separator) else {
        return false;
    };
    let Some(bottom) = contents.viewport[y + 1..]
        .iter()
        .position(separator)
        .map(|offset| y + 1 + offset)
    else {
        return false;
    };
    let input = &contents.viewport[top + 1..bottom];
    let Some(text) = input[0].strip_prefix('❯') else {
        return false;
    };
    if (!text.is_empty() && !text.starts_with(' ')) || (y == top + 1 && x < 2) {
        return false;
    }
    !require_empty
        || (y == top + 1
            && x == 2
            && text.trim().is_empty()
            && input[1..].iter().all(|line| line.trim().is_empty()))
}
