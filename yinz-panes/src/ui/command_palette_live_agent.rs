//! Literal, display-only presentation of a live agent, matching the agents panel.
use super::{append, md, take_columns, MdLine, MdStyle};
use unicode_width::UnicodeWidthStr;
use yinz_types::{agent::AgentState, data::PaneInfo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPresentation {
    pub project: String,
    pub branch: String,
    pub provider: String,
    pub state: AgentState,
    pub host: Option<String>,
    pub pane_id: String,
    pub activity: String,
    pub hook_flash: bool,
}

impl AgentPresentation {
    pub fn from_pane(pane: &PaneInfo) -> Option<Self> {
        let agent = pane.agent.as_ref()?;
        let provider = one_line(&agent.kind.replace('◆', ""));
        let activity = match agent.state {
            AgentState::Waiting => agent.waiting_on.as_deref(),
            AgentState::Busy => agent.detail.as_deref(),
            AgentState::Idle | AgentState::Exited => None,
        }
        .map(one_line)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| one_line(&pane.title));
        Some(Self {
            project: String::new(),
            branch: String::new(),
            provider: if provider.is_empty() {
                "agent".into()
            } else {
                provider
            },
            state: agent.state,
            host: pane.target.display().map(|host| one_line(&host)),
            pane_id: pane.id.legacy_parts().0.to_string(),
            activity,
            hook_flash: false,
        })
    }

    pub(super) fn lines(&self, width: usize, max_lines: usize) -> Vec<MdLine> {
        let (glyph, status, color) = match self.state {
            AgentState::Busy => ("●", "working", Some(2)),
            AgentState::Waiting => ("!", "needs input", Some(1)),
            AgentState::Idle => ("○", "idle", None),
            AgentState::Exited => ("×", "exited", None),
        };
        let mut main = styled(
            glyph,
            MdStyle {
                bold: true,
                color: if self.state == AgentState::Busy && self.hook_flash {
                    Some(0)
                } else {
                    color
                },
                ..Default::default()
            },
        );
        append(
            &mut main,
            &styled(
                &format!(" {}", self.provider),
                MdStyle {
                    bold: true,
                    ..Default::default()
                },
            ),
        );
        append(
            &mut main,
            &styled(
                &format!("  {status}"),
                MdStyle {
                    color,
                    ..Default::default()
                },
            ),
        );
        if let Some(host) = &self.host {
            append(
                &mut main,
                &styled(
                    &format!(" @{host}"),
                    MdStyle {
                        color: Some(3),
                        ..Default::default()
                    },
                ),
            );
        }
        let tag = format!("[{}]", self.pane_id);
        let tag_style = MdStyle {
            color: Some(3),
            ..Default::default()
        };
        let max_lines = max_lines.clamp(1, 5);
        if max_lines == 1 {
            // Reserve the ID before truncating the identity; wrapping could drop it.
            let identity_width = width.saturating_sub(tag.width() + 2);
            if identity_width == 0 {
                main = MdLine::plain("");
            } else if main.text.width() > identity_width {
                ellipsize(&mut main, identity_width);
            }
            let tag = take_columns(&tag, width);
            let gap = width.saturating_sub(main.text.width() + tag.width());
            append(&mut main, &MdLine::plain(" ".repeat(gap)));
            append(&mut main, &styled(&tag, tag_style));
            return vec![main];
        }
        let gap = width.saturating_sub(main.text.width() + tag.width());
        // Keep at least two columns between the status/host and the right-aligned ID.
        append(
            &mut main,
            &MdLine::plain(" ".repeat(if gap >= 2 { gap } else { 2 })),
        );
        append(&mut main, &styled(&tag, tag_style));

        let mut lines = Vec::new();
        let project = self.project.trim().replace(char::is_control, " ");
        let branch = self.branch.trim().replace(char::is_control, " ");
        let branch = if branch == "no branch" { "" } else { &branch };
        // A single row must retain the identity, including the right-aligned pane ID.
        if max_lines > 1 && (!project.is_empty() || !branch.is_empty()) {
            let mut metadata = styled(
                &project,
                MdStyle {
                    bold: true,
                    color: Some(0),
                    ..Default::default()
                },
            );
            let separator = if project.is_empty() || branch.is_empty() {
                ""
            } else {
                " · "
            };
            append(
                &mut metadata,
                &styled(
                    &format!("{separator}{branch}"),
                    MdStyle {
                        color: Some(2),
                        ..Default::default()
                    },
                ),
            );
            lines.push(metadata);
        }
        // One extra row tells us whether the final visible row needs an ellipsis.
        lines.extend(wrap(&main, width, 0, max_lines + 1 - lines.len()));
        if max_lines > 1 && lines.len() <= max_lines && !self.activity.is_empty() {
            let waiting = self.state == AgentState::Waiting;
            let activity = styled(
                &self.activity,
                MdStyle {
                    bold: waiting,
                    color: waiting.then_some(1),
                    ..Default::default()
                },
            );
            lines.extend(wrap(&activity, width, 2, max_lines + 1 - lines.len()));
        }
        let truncated = lines.len() > max_lines;
        lines.truncate(max_lines);
        if truncated && let Some(last) = lines.last_mut() {
            ellipsize(last, width);
        }
        for line in &mut lines {
            if line.text.width() > width {
                ellipsize(line, width);
            }
        }
        lines
    }
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn styled(text: &str, style: MdStyle) -> MdLine {
    MdLine {
        text: text.into(),
        styles: vec![style; text.chars().count()],
    }
}

fn wrap(line: &MdLine, width: usize, first_indent: usize, max_lines: usize) -> Vec<MdLine> {
    let mut block = md::MdBlock {
        chars: line
            .text
            .chars()
            .zip(line.styles.iter().copied())
            .map(|(ch, style)| md::StyledChar { ch, style })
            .collect(),
        ..Default::default()
    };
    let mut lines = Vec::new();
    while !block.chars.is_empty() && lines.len() < max_lines {
        let indent = if lines.is_empty() { first_indent } else { 2 }.min(width.saturating_sub(1));
        let mut next = md::render_block_head(&block, width.saturating_sub(indent), 1).remove(0);
        let consumed = next.text.chars().count();
        block.chars.drain(..consumed);
        let spaces = block.chars.iter().take_while(|ch| ch.ch == ' ').count();
        block.chars.drain(..spaces);
        next = next.indent(indent);
        lines.push(next);
    }
    lines
}

fn ellipsize(line: &mut MdLine, width: usize) {
    line.text = take_columns(&line.text, width.saturating_sub(1));
    line.styles.truncate(line.text.chars().count());
    let style = line.styles.last().copied().unwrap_or_default();
    append(line, &styled("…", style));
}
