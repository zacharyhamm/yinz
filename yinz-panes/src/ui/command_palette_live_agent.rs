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
    pub last_reply: Option<yinz_bus::command_palette::AgentReplyPreview>,
    pub hook_flash: bool,
    pub focused: bool,
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
            last_reply: None,
            hook_flash: false,
            focused: false,
        })
    }

    pub(super) fn lines(&self, width: usize, max_lines: usize) -> Vec<MdLine> {
        let (glyph, status, color) = crate::ui::agent_title::status(self.state);
        let mut main = styled(
            glyph,
            MdStyle {
                bold: true,
                color: if self.state == AgentState::Busy && self.hook_flash {
                    Some(crate::ui::agent_title::FLASH_COLOR)
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
        let tag_style = MdStyle {
            color: Some(3),
            ..Default::default()
        };
        let mut tag = if self.focused {
            styled(
                "⌖ ",
                MdStyle {
                    bold: true,
                    color: Some(0),
                    ..Default::default()
                },
            )
        } else {
            MdLine::plain("")
        };
        append(&mut tag, &styled(&format!("[{}]", self.pane_id), tag_style));
        let max_lines = max_lines.clamp(1, 5);
        let reply = self.last_reply.as_ref().and_then(|reply| {
            let text = one_line(&reply.text);
            (max_lines > 1 && !text.is_empty()).then(|| {
                let style = MdStyle {
                    dim: true,
                    ..Default::default()
                };
                let mut line = styled(&format!("↳ {text}"), style);
                if reply.truncated || line.text.width() > width {
                    ellipsize(&mut line, width);
                    line.styles.fill(style);
                }
                line
            })
        });
        // Keep the reply below the existing details, reserving its row before
        // activity wraps. At two rows, identity takes priority over metadata.
        let detail_lines = max_lines - usize::from(reply.is_some());
        if detail_lines == 1 || self.focused {
            // Reserve the ID and focus marker before truncating the identity.
            let identity_width = width.saturating_sub(tag.text.width() + 2);
            if identity_width == 0 {
                main = MdLine::plain("");
            } else if main.text.width() > identity_width {
                ellipsize(&mut main, identity_width);
            }
            tag.text = take_columns(&tag.text, width);
            tag.styles.truncate(tag.text.chars().count());
            let gap = width.saturating_sub(main.text.width() + tag.text.width());
            append(&mut main, &MdLine::plain(" ".repeat(gap)));
            append(&mut main, &tag);
            if max_lines == 1 {
                return vec![main];
            }
        } else {
            let gap = width.saturating_sub(main.text.width() + tag.text.width());
            // Keep at least two columns between the status/host and the right-aligned ID.
            append(
                &mut main,
                &MdLine::plain(" ".repeat(if gap >= 2 { gap } else { 2 })),
            );
            append(&mut main, &tag);
        }

        let mut lines = Vec::new();
        let project = self.project.trim().replace(char::is_control, " ");
        let branch = self.branch.trim().replace(char::is_control, " ");
        let branch = if branch == "no branch" { "" } else { &branch };
        // A single row must retain the identity, including the right-aligned pane ID.
        if detail_lines > 1 && (!project.is_empty() || !branch.is_empty()) {
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
        if self.focused || detail_lines == 1 {
            // Already fitted above; preserve the marker and ID as one aligned group.
            lines.push(main);
        } else {
            // One extra row tells us whether the final visible row needs an ellipsis.
            lines.extend(wrap(&main, width, 0, detail_lines + 1 - lines.len()));
        }
        if detail_lines > 1
            && (lines.len() < detail_lines || (!self.focused && lines.len() == detail_lines))
            && !self.activity.is_empty()
        {
            let waiting = self.state == AgentState::Waiting;
            let activity = styled(
                &self.activity,
                MdStyle {
                    bold: waiting,
                    color: waiting.then_some(1),
                    ..Default::default()
                },
            );
            lines.extend(wrap(&activity, width, 2, detail_lines + 1 - lines.len()));
        }
        let truncated = lines.len() > detail_lines;
        lines.truncate(detail_lines);
        if truncated && let Some(last) = lines.last_mut() {
            ellipsize(last, width);
        }
        for line in &mut lines {
            if line.text.width() > width {
                ellipsize(line, width);
            }
        }
        lines.extend(reply);
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
    if width == 0 {
        *line = MdLine::plain("");
        return;
    }
    line.text = take_columns(&line.text, width.saturating_sub(1));
    line.styles.truncate(line.text.chars().count());
    let style = line.styles.last().copied().unwrap_or_default();
    append(line, &styled("…", style));
}
