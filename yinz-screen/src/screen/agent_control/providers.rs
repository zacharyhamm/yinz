mod agy;
mod claude;
pub(in crate::screen) mod codex;
mod muse;
use super::{AgentHost, Composer, PaneId};
use yinz_types::agent::{
    control::{
        claude_prompt_text, is_command_prompt, needs_paste_lead_in, pi_prompt_text, PASTE_LEAD_IN,
    },
    provider::AgentInput,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::screen) enum Provider {
    Claude,
    Codex,
    Agy,
    Pi,
    Muse,
    Other,
}

impl Provider {
    pub(in crate::screen) fn for_kind(kind: &str) -> Self {
        match kind {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "agy" => Self::Agy,
            "pi" => Self::Pi,
            "muse" => Self::Muse,
            _ => Self::Other,
        }
    }

    pub(in crate::screen) fn turn_key<T>(self, turn: T, prompt: T) -> T {
        match self {
            Self::Claude | Self::Agy => prompt,
            _ => turn,
        }
    }

    pub(super) fn web_command(self, text: &str) -> bool {
        text.trim_start().starts_with('/')
            || match self {
                Self::Claude => claude_prompt_text(text).trim_start().starts_with('/'),
                Self::Pi => pi_prompt_text(text).starts_with('/'),
                _ => false,
            }
    }

    pub(super) fn trimmed(self, text: &str) -> &str {
        match self {
            Self::Pi => pi_prompt_text(text),
            _ => text.trim(),
        }
    }

    pub(super) fn normalize(self, prompt: String) -> String {
        match self {
            Self::Claude => claude_prompt_text(&prompt).into_owned(),
            _ => prompt,
        }
    }

    pub(super) fn default_submit_delay_ms(self) -> u64 {
        match self {
            Self::Claude => 500,
            _ => 100,
        }
    }

    pub(super) fn type_empty_paste(self) -> bool {
        matches!(self, Self::Codex | Self::Pi)
    }
}

pub(super) fn is_command(kind: &str, input: Option<&AgentInput>, prompt: &str) -> bool {
    input.map_or_else(
        || is_command_prompt(kind, prompt),
        |input| input.is_command_prompt(prompt),
    )
}

pub(super) fn encode_prompt(
    kind: &str,
    input: Option<&AgentInput>,
    prompt: String,
) -> (Vec<u8>, Vec<u8>) {
    if let Some(input) = input {
        (
            input.paste_prefix.as_bytes().to_vec(),
            prompt.replace('\n', &input.newline).into_bytes(),
        )
    } else if needs_paste_lead_in(kind, &prompt) {
        (
            format!("{PASTE_LEAD_IN} ").into_bytes(),
            prompt.into_bytes(),
        )
    } else if (Provider::for_kind(kind) == Provider::Claude && prompt.starts_with('/'))
        || yinz_types::agent::control::is_conversation_reset(kind, &prompt)
    {
        let end = prompt.find(char::is_whitespace).unwrap_or(prompt.len());
        (
            prompt.as_bytes()[..end].to_vec(),
            prompt.as_bytes()[end..].to_vec(),
        )
    } else {
        (Vec::new(), prompt.into_bytes())
    }
}

pub(super) fn draft_composer(host: &impl AgentHost, pane: PaneId, kind: &str) -> Option<Composer> {
    let provider = Provider::for_kind(kind);
    if !matches!(provider, Provider::Claude | Provider::Codex | Provider::Pi) {
        return None;
    }
    let (columns, contents) = host.viewport(pane, false)?;
    let (x, y) = contents.cursor?.into_tuple();
    if contents.is_scrolled || y >= contents.viewport.len() {
        return None;
    }
    let rows = &contents.viewport;
    let separator = |row: &String| row.chars().count() == columns && row.chars().all(|c| c == '─');
    let (top, bottom, prefix) = match provider {
        Provider::Claude | Provider::Pi => {
            let top = rows[..y].iter().rposition(separator)? + 1;
            let bottom = y + 1 + rows[y + 1..].iter().position(separator)?;
            let prefix = if provider == Provider::Claude { 2 } else { 0 };
            if provider == Provider::Pi {
                // Pi has no prompt prefix. Its cyan borders distinguish the
                // editor from full-width rules typed into a multiline draft.
                let ansi = host.viewport(pane, true)?.1;
                let top_rule = ansi.viewport.get(top - 1)?;
                if !top_rule.starts_with("\x1b[38;5;6m")
                    || ansi.viewport.get(bottom) != Some(top_rule)
                {
                    return None;
                }
            }
            if provider == Provider::Claude
                && !rows[top].starts_with("❯\u{a0}")
                && !rows[top].starts_with("❯ ")
            {
                return None;
            }
            // Extra panels below the composer are not an editor footer.
            if rows[bottom + 1..]
                .iter()
                .filter(|row| !row.trim().is_empty())
                .count()
                > 2
            {
                return None;
            }
            (top, bottom, prefix)
        },
        Provider::Codex => {
            let top = rows[..=y].iter().rposition(|row| row.starts_with("› "))?;
            let is_footer = |row: &String| row.starts_with("  ") && row.contains(" · ");
            let bottom = y + 1 + rows[y + 1..].iter().position(is_footer)?;
            // Shift+Tab can leave the collaboration-mode footer above the
            // normal status row, occupying the editor's trailing blank row.
            let mode_footer = rows[bottom].trim_end().ends_with("Plan mode")
                || rows[bottom].trim_end().ends_with("Default mode");
            let mut footer = rows[bottom + 1..]
                .iter()
                .filter(|row| !row.trim().is_empty());
            let extra_footer = footer.next();
            let two_row_mode = mode_footer && extra_footer.is_some_and(is_footer);
            if top == 0
                || !rows[top - 1].trim().is_empty()
                || (!two_row_mode && (bottom <= y + 1 || !rows[bottom - 1].trim().is_empty()))
                || (extra_footer.is_some() && !two_row_mode)
                || footer.next().is_some()
                || rows[top + 1..bottom]
                    .iter()
                    .any(|row| !row.is_empty() && !row.starts_with("  "))
            {
                return None;
            }
            (top, bottom, 2)
        },
        _ => return None,
    };
    if (y == top && x < prefix) || rows[top..bottom].iter().any(separator) {
        return None;
    }
    // Only the editor and its validated footer belong to this composer.
    // Codex also renders remote image rows immediately above the blank row
    // preceding its prompt. Local images and pastes are inline editor text.
    let remote_image = provider == Provider::Codex
        && top.checked_sub(2).is_some_and(|row| {
            rows[row]
                .trim_end()
                .strip_prefix("  [Image #")
                .and_then(|label| label.strip_suffix(']'))
                .is_some_and(|number| {
                    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                })
        });
    let indicators = remote_image
        || rows[top..].iter().any(|row| {
            row.contains("[Image") || row.contains("[Pasted") || row.contains("[Attached")
        });
    // Captured Codex 0.160.x and Claude 2.1.289 placeholders are dim;
    // identically spelled typed drafts are not. Codex can style padding
    // after the hint, so a reset immediately after it is not required.
    let hint: String = rows[top].chars().skip(prefix).collect();
    let hint = hint.trim_end();
    let known_hint = match provider {
        Provider::Codex => hint == "Ask Codex to do anything",
        Provider::Claude => hint.starts_with("Try \"") && hint.ends_with('"'),
        _ => false,
    };
    let placeholder = known_hint
        && host
            .viewport(pane, true)?
            .1
            .viewport
            .get(top)
            .is_some_and(|row| row.contains(&format!("\x1b[2m\x1b[23m{hint}")));
    let empty = !indicators
        && y == top
        && x == prefix
        && (rows[top].chars().skip(prefix).all(char::is_whitespace) || placeholder)
        && rows[top + 1..bottom]
            .iter()
            .all(|row| row.trim().is_empty());
    let mut before_cursor = String::new();
    for (index, row) in rows.iter().enumerate().take(y + 1).skip(top) {
        let row = if index < y {
            row.trim_end()
        } else {
            row.as_str()
        };
        let mut column = 0;
        for character in row.chars() {
            let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
            if index == y && column + width > x {
                break;
            }
            if column >= prefix {
                before_cursor.push(character);
            }
            column += width;
        }
    }
    Some(Composer {
        empty,
        before_cursor,
    })
}

pub(super) use muse::muse_composer;
