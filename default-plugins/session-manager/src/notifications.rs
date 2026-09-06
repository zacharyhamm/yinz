use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::*;

#[derive(Debug, Default)]
pub struct Notifications {
    rows: Vec<NotificationInfo>,
    selected_index: Option<usize>,
    detail_scroll: Option<usize>,
    page_size: usize,
}

impl Notifications {
    pub fn refresh(&mut self) -> bool {
        self.apply_refresh(list_notifications())
    }

    fn apply_refresh(&mut self, result: Result<Vec<NotificationInfo>, String>) -> bool {
        result.is_ok_and(|rows| self.replace_rows(rows))
    }

    fn replace_rows(&mut self, mut rows: Vec<NotificationInfo>) -> bool {
        let selected_id = self
            .selected_index
            .and_then(|index| self.rows.get(index))
            .map(|notification| notification.id);
        rows.reverse();
        if rows == self.rows {
            return false;
        }
        self.rows = rows;
        self.selected_index = selected_id
            .and_then(|id| {
                self.rows
                    .iter()
                    .position(|notification| notification.id == id)
            })
            .or((!self.rows.is_empty()).then_some(0));
        true
    }

    pub fn move_down(&mut self) {
        self.move_by(1);
    }

    pub fn move_up(&mut self) {
        self.move_by(-1);
    }

    pub fn page_down(&mut self) {
        if let Some(scroll) = self.detail_scroll.as_mut() {
            *scroll = scroll.saturating_add(self.page_size.max(1));
        } else {
            self.move_by(self.page_size.max(1) as isize);
        }
    }

    pub fn page_up(&mut self) {
        if let Some(scroll) = self.detail_scroll.as_mut() {
            *scroll = scroll.saturating_sub(self.page_size.max(1));
        } else {
            self.move_by(-(self.page_size.max(1) as isize));
        }
    }

    pub fn open_detail(&mut self) {
        if self.selected_index.is_some() && self.detail_scroll.is_none() {
            self.detail_scroll = Some(0);
        }
    }

    pub fn close_detail(&mut self) -> bool {
        self.detail_scroll.take().is_some()
    }

    pub fn in_detail(&self) -> bool {
        self.detail_scroll.is_some()
    }

    pub fn render(&mut self, rows: usize, columns: usize, x: usize, y: usize) {
        if self.in_detail() {
            self.render_detail(rows, columns, x, y);
        } else {
            self.render_list(rows, columns, x, y);
        }
    }

    fn move_by(&mut self, amount: isize) {
        if let Some(scroll) = self.detail_scroll.as_mut() {
            *scroll = if amount < 0 {
                scroll.saturating_sub(amount.unsigned_abs())
            } else {
                scroll.saturating_add(amount as usize)
            };
            return;
        }
        if self.rows.is_empty() {
            return;
        }
        let current = self.selected_index.unwrap_or(0) as isize;
        self.selected_index = Some(
            current
                .saturating_add(amount)
                .clamp(0, self.rows.len().saturating_sub(1) as isize) as usize,
        );
    }

    fn render_list(&mut self, rows: usize, columns: usize, x: usize, y: usize) {
        let available = rows.saturating_sub(3);
        self.page_size = available.saturating_sub(1);
        if self.rows.is_empty() {
            print_text_with_coordinates(
                Text::new("No notifications received in this session."),
                x,
                y + 2,
                None,
                None,
            );
            return;
        }
        let (first, last) = visible_range(self.rows.len(), self.selected_index, self.page_size);
        let mut table = Table::new().add_row(vec!["Received", "Notification"]);
        let preview_width = columns.saturating_sub(18);
        for index in first..last {
            let notification = &self.rows[index];
            let mut cells = vec![
                Text::new(relative_age(notification.received_at_unix_ms)).color_range(2, ..),
                Text::new(truncate(&preview(&notification.message), preview_width)),
            ];
            if Some(index) == self.selected_index {
                cells = cells.into_iter().map(Text::selected).collect();
            }
            table = table.add_styled_row(cells);
        }
        print_table_with_coordinates(table, x, y + 1, Some(columns), Some(available));
    }

    fn render_detail(&mut self, rows: usize, columns: usize, x: usize, y: usize) {
        let Some(notification) = self.selected_index.and_then(|index| self.rows.get(index)) else {
            self.detail_scroll = None;
            return;
        };
        print_text_with_coordinates(
            Text::new(format!(
                "Received {}",
                relative_age(notification.received_at_unix_ms)
            ))
            .color_range(2, 9..),
            x,
            y + 1,
            None,
            None,
        );
        let available = rows.saturating_sub(4);
        self.page_size = available;
        let lines = wrap(&sanitize(&notification.message, true), columns.max(1));
        let max_scroll = lines.len().saturating_sub(available);
        let scroll = self.detail_scroll.unwrap_or(0).min(max_scroll);
        self.detail_scroll = Some(scroll);
        for (line_index, line) in lines.iter().skip(scroll).take(available).enumerate() {
            print_text_with_coordinates(Text::new(line), x, y + 3 + line_index, None, None);
        }
    }
}

fn visible_range(len: usize, selected: Option<usize>, page_size: usize) -> (usize, usize) {
    let page_size = page_size.max(1).min(len);
    let selected = selected.unwrap_or(0).min(len.saturating_sub(1));
    let first = selected
        .saturating_sub(page_size / 2)
        .min(len.saturating_sub(page_size));
    (first, first + page_size)
}

fn relative_age(received_at_unix_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let seconds = now.saturating_sub(received_at_unix_ms) / 1000;
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn preview(message: &str) -> String {
    sanitize(message, false)
}

fn sanitize(message: &str, preserve_newlines: bool) -> String {
    message
        .chars()
        .filter_map(|character| match character {
            '\n' if preserve_newlines => Some('\n'),
            '\n' => Some(' '),
            '\t' => Some(' '),
            character if character.is_control() => None,
            character => Some(character),
        })
        .collect()
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut truncated = text
        .chars()
        .take_while(|character| {
            let character_width = character.width().unwrap_or(0);
            let fits = used + character_width < width;
            used += character_width;
            fits
        })
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn wrap(message: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    for source_line in message.split('\n') {
        let mut line = String::new();
        let mut line_width = 0;
        for character in source_line.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if line_width > 0 && line_width + character_width > width {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            line.push(character);
            line_width += character_width;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u64, message: &str) -> NotificationInfo {
        NotificationInfo {
            id: zellij_tile::prelude::NotificationId::new(id),
            message: message.to_owned(),
            received_at_unix_ms: id,
        }
    }

    #[test]
    fn selection_survives_a_new_notification() {
        let mut notifications = Notifications {
            rows: vec![notification(2, "two"), notification(1, "one")],
            selected_index: Some(1),
            ..Default::default()
        };
        notifications.replace_rows(vec![
            notification(1, "one"),
            notification(2, "two"),
            notification(3, "three"),
        ]);
        assert_eq!(notifications.selected_index, Some(2));
    }

    #[test]
    fn failed_refresh_keeps_cached_rows() {
        let mut notifications = Notifications {
            rows: vec![notification(1, "one")],
            selected_index: Some(0),
            ..Default::default()
        };
        assert!(!notifications.apply_refresh(Err("timeout".to_owned())));
        assert_eq!(notifications.rows, vec![notification(1, "one")]);
        assert!(notifications.apply_refresh(Ok(vec![])));
        assert!(notifications.rows.is_empty());
    }

    #[test]
    fn detail_text_is_safe_and_wraps_unicode() {
        assert_eq!(
            sanitize("safe\u{1b}[2J\ttext\nnext", true),
            "safe[2J text\nnext"
        );
        assert_eq!(wrap("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn preview_truncates_by_display_width() {
        assert_eq!(truncate("日本語", 5), "日本…");
        assert_eq!(truncate("😀abc", 4), "😀a…");
        assert_eq!(truncate("日本", 4), "日本");
        assert_eq!(truncate("日", 0), "");
    }

    #[test]
    fn viewport_keeps_the_selection_visible() {
        assert_eq!(visible_range(10, Some(8), 4), (6, 10));
        assert_eq!(visible_range(10, Some(1), 4), (0, 4));
    }
}
