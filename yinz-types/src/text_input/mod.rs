//! A single-line text editor: the buffer, the cursor, and the editing keymap.
//!
//! This is the merge of two copies that had been maintained by hand and drifted
//! — `default-plugins/layout-manager/src/text_input.rs` and
//! `default-plugins/agent-proto/src/text_input.rs`. The layout-manager header
//! explained why both existed:
//!
//!   This is a duplicate of the same file in the sequence (and possibly other) plugins
//!   Right now there's no good place to put them for shared logic except the yinz-tile-utils crate
//!   (which I'd rather eliminate), hence the duplication.
//!
//! This module is that place. The behaviour is agent-proto's, which was the
//! superset (shell-style history, `replace_text`); the state-machine tests come
//! from layout-manager, which was the only copy that had them.
//!
//! The buffer is edited in **char** units — `cursor_position` is a char index,
//! not a byte offset. Turning that into a screen column is the renderer's job
//! and is measured in columns rather than chars, so a wide CJK or emoji
//! character does not desync the two; see [`viewport`].
use crate::keys::{BareKey, KeyModifier, KeyWithModifier};

pub mod viewport;

#[cfg(test)]
#[path = "unit/key_tests.rs"]
mod key_tests;

const MAX_UNDO_STACK_SIZE: usize = 100;

/// Action returned by TextInput after handling a key event
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Continue editing
    Continue,
    /// User pressed Enter to submit
    Submit,
    /// User pressed Esc to cancel
    Cancel,
    /// User pressed Tab to request completion
    Complete,
    /// Key was not handled by the input
    NoAction,
}

/// A reusable text input component with cursor support and standard editing keybindings
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    buffer: String,
    cursor_position: usize, // Character position (0-based), NOT byte position
    undo_stack: Vec<(String, usize)>, // (buffer, cursor) snapshots
    redo_stack: Vec<(String, usize)>,
    last_edit_was_insert: bool, // For coalescing consecutive inserts
    history: Vec<String>,
    history_position: Option<usize>,
    history_draft: String,
}

impl TextInput {
    /// Create a new TextInput with the given initial text
    /// Cursor is positioned at the end of the text
    pub fn new(initial_text: String) -> Self {
        let cursor_position = initial_text.chars().count();
        Self {
            buffer: initial_text,
            cursor_position,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_was_insert: false,
            history: Vec::new(),
            history_position: None,
            history_draft: String::new(),
        }
    }

    /// Create an empty TextInput
    pub fn empty() -> Self {
        Self::new(String::new())
    }

    /// Get the current text
    pub fn get_text(&self) -> &str {
        &self.buffer
    }

    /// Get the cursor position (in characters, not bytes)
    pub fn get_cursor_position(&self) -> usize {
        self.cursor_position
    }

    /// Check if the input is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Get a shorthand for cursor_position
    #[allow(unused)]
    pub fn cursor_position(&self) -> usize {
        self.cursor_position
    }

    /// Get mutable access to the underlying buffer for direct manipulation
    #[allow(unused)]
    pub fn get_text_mut(&mut self) -> &mut String {
        &mut self.buffer
    }

    /// Set the text and move cursor to the end
    #[allow(unused)]
    pub fn set_text(&mut self, text: String) {
        self.reset_history_position();
        self.break_coalescing();
        self.save_undo_state();
        self.cursor_position = text.chars().count();
        self.buffer = text;
    }

    /// Set cursor position (clamped to text length)
    #[allow(unused)]
    pub fn set_cursor_position(&mut self, pos: usize) {
        let text_len = self.buffer.chars().count();
        self.cursor_position = pos.min(text_len);
    }

    pub fn replace_text(&mut self, text: String, cursor: usize) {
        self.buffer = text;
        self.cursor_position = cursor.min(self.buffer.chars().count());
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.reset_history_position();
        self.break_coalescing();
    }

    /// Clear all text and reset cursor
    pub fn clear(&mut self) {
        self.reset_history_position();
        self.break_coalescing();
        self.save_undo_state();
        self.buffer.clear();
        self.cursor_position = 0;
    }

    /// Insert a character at the current cursor position
    pub fn insert_char(&mut self, c: char) {
        self.save_undo_state_unless_coalescing();
        // Convert cursor position (char index) to byte index
        let byte_index = self.char_index_to_byte_index(self.cursor_position);
        self.buffer.insert(byte_index, c);
        self.cursor_position += 1;
    }

    /// Insert a whole string at the cursor, as one edit.
    ///
    /// This is the paste path, so it deliberately does not go through
    /// insert_char: coalescing would make the undo granularity depend on
    /// whatever was typed just before, and a pasted block should be one
    /// undo step no matter what preceded it.
    pub fn insert_text(&mut self, text: &str) {
        let text = Self::sanitize_pasted(text);
        if text.is_empty() {
            return;
        }
        self.reset_history_position();
        self.break_coalescing();
        self.save_undo_state();
        let byte_index = self.char_index_to_byte_index(self.cursor_position);
        self.buffer.insert_str(byte_index, &text);
        self.cursor_position += text.chars().count();
        // The next typed character starts its own coalescing run rather than
        // folding into the paste.
        self.break_coalescing();
    }

    /// A paste arrives as whatever the terminal had on the clipboard. Enter
    /// inside it is a CR, and anything else in C0 is not something the input
    /// box or the transcript can render meaningfully.
    fn sanitize_pasted(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push('\n');
                },
                '\n' | '\t' => out.push(c),
                c if c.is_control() => {},
                c => out.push(c),
            }
        }
        out
    }

    /// Delete the character before the cursor (backspace)
    pub fn backspace(&mut self) {
        if self.cursor_position > 0 {
            self.break_coalescing();
            self.save_undo_state();
            self.cursor_position -= 1;
            let byte_index = self.char_index_to_byte_index(self.cursor_position);
            self.buffer.remove(byte_index);
        }
    }

    /// Delete the character at the cursor position (delete key)
    pub fn delete(&mut self) {
        let len = self.buffer.chars().count();
        if self.cursor_position < len {
            self.break_coalescing();
            self.save_undo_state();
            let byte_index = self.char_index_to_byte_index(self.cursor_position);
            self.buffer.remove(byte_index);
        }
    }

    /// Delete the word before the cursor (Ctrl/Alt + Backspace)
    pub fn delete_word_backward(&mut self) {
        if self.cursor_position == 0 {
            return;
        }

        self.break_coalescing();
        self.save_undo_state();

        let old_position = self.cursor_position;
        self.move_word_left();
        let new_position = self.cursor_position;

        // Delete from new position to old position
        let start_byte = self.char_index_to_byte_index(new_position);
        let end_byte = self.char_index_to_byte_index(old_position);
        self.buffer.drain(start_byte..end_byte);
    }

    /// Delete the word after the cursor (Ctrl/Alt + Delete)
    pub fn delete_word_forward(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let len = chars.len();

        if self.cursor_position >= len {
            return;
        }

        self.break_coalescing();
        self.save_undo_state();

        let start_position = self.cursor_position;
        let mut end_position = start_position;

        // Skip the current word
        while end_position < len && !chars[end_position].is_whitespace() {
            end_position += 1;
        }

        // Skip any whitespace after the word
        while end_position < len && chars[end_position].is_whitespace() {
            end_position += 1;
        }

        // Delete from start to end position
        let start_byte = self.char_index_to_byte_index(start_position);
        let end_byte = self.char_index_to_byte_index(end_position);
        self.buffer.drain(start_byte..end_byte);
    }

    /// Delete from the cursor to the end of the line (Ctrl-K)
    /// The buffer can hold newlines (Alt-Enter), so this stops at the next one.
    /// Sitting directly on a newline eats the newline itself, as in emacs.
    pub fn kill_to_end(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut end = self.cursor_position;

        while end < chars.len() && chars[end] != '\n' {
            end += 1;
        }
        if end == self.cursor_position && end < chars.len() {
            end += 1;
        }
        if end == self.cursor_position {
            return;
        }

        self.break_coalescing();
        self.save_undo_state();

        let start_byte = self.char_index_to_byte_index(self.cursor_position);
        let end_byte = self.char_index_to_byte_index(end);
        self.buffer.drain(start_byte..end_byte);
    }

    /// Delete from the start of the line to the cursor (Ctrl-U)
    pub fn kill_to_start(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut start = self.cursor_position;

        while start > 0 && chars[start - 1] != '\n' {
            start -= 1;
        }
        if start == self.cursor_position {
            return;
        }

        self.break_coalescing();
        self.save_undo_state();

        let start_byte = self.char_index_to_byte_index(start);
        let end_byte = self.char_index_to_byte_index(self.cursor_position);
        self.buffer.drain(start_byte..end_byte);
        self.cursor_position = start;
    }

    /// Move cursor one position to the left
    pub fn move_left(&mut self) {
        if self.cursor_position > 0 {
            self.break_coalescing();
            self.cursor_position -= 1;
        }
    }

    /// Move cursor one position to the right
    pub fn move_right(&mut self) {
        let len = self.buffer.chars().count();
        if self.cursor_position < len {
            self.break_coalescing();
            self.cursor_position += 1;
        }
    }

    /// Move cursor to the start of the text (Ctrl-A / Home)
    pub fn move_to_start(&mut self) {
        self.break_coalescing();
        self.cursor_position = 0;
    }

    /// Move cursor to the end of the text (Ctrl-E / End)
    pub fn move_to_end(&mut self) {
        self.break_coalescing();
        self.cursor_position = self.buffer.chars().count();
    }

    /// Move cursor to the start of the previous word (Ctrl/Alt + Left)
    pub fn move_word_left(&mut self) {
        if self.cursor_position == 0 {
            return;
        }

        self.break_coalescing();

        let chars: Vec<char> = self.buffer.chars().collect();
        let mut pos = self.cursor_position;

        // Skip any whitespace immediately to the left
        while pos > 0 && chars[pos - 1].is_whitespace() {
            pos -= 1;
        }

        // Skip the word characters
        while pos > 0 && !chars[pos - 1].is_whitespace() {
            pos -= 1;
        }

        self.cursor_position = pos;
    }

    /// Move cursor to the start of the next word (Ctrl/Alt + Right)
    pub fn move_word_right(&mut self) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let len = chars.len();

        if self.cursor_position >= len {
            return;
        }

        self.break_coalescing();

        let mut pos = self.cursor_position;

        // Skip the current word
        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }

        // Skip any whitespace
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }

        self.cursor_position = pos;
    }

    pub fn set_history(&mut self, history: Vec<String>) {
        self.history = history;
        self.reset_history_position();
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    fn previous_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let position = match self.history_position {
            Some(position) => position.saturating_sub(1),
            None => {
                self.history_draft = self.buffer.clone();
                self.history.len() - 1
            },
        };
        self.history_position = Some(position);
        self.replace_buffer(self.history[position].clone());
    }

    fn next_history(&mut self) {
        let Some(position) = self.history_position else {
            return;
        };
        if position + 1 < self.history.len() {
            let position = position + 1;
            self.history_position = Some(position);
            self.replace_buffer(self.history[position].clone());
        } else {
            self.history_position = None;
            self.replace_buffer(self.history_draft.clone());
        }
    }

    fn replace_buffer(&mut self, text: String) {
        self.cursor_position = text.chars().count();
        self.buffer = text;
        self.break_coalescing();
    }

    fn reset_history_position(&mut self) {
        self.history_position = None;
        self.history_draft.clear();
    }

    /// Handle a key event and return the appropriate action
    /// This is the main entry point for key handling
    pub fn handle_key(&mut self, key: KeyWithModifier) -> InputAction {
        // Ctrl+Shift is checked first: has_modifiers is a subset test, so
        // Ctrl-Shift-Z also satisfies the plain-Ctrl block below, which now
        // swallows every Char it doesn't recognize.
        if key.has_modifiers(&[KeyModifier::Ctrl, KeyModifier::Shift])
            && let BareKey::Char('Z') = key.bare_key
        {
            // Ctrl-Shift-Z: Redo (alternative)
            self.redo();
            return InputAction::Continue;
        }

        // Check for Ctrl modifiers
        if key.has_modifiers(&[KeyModifier::Ctrl]) {
            match key.bare_key {
                BareKey::Char('a') => {
                    self.move_to_start();
                    return InputAction::Continue;
                },
                BareKey::Char('e') => {
                    self.move_to_end();
                    return InputAction::Continue;
                },
                BareKey::Char('c') => {
                    // Ctrl-C clears the prompt
                    return InputAction::Cancel;
                },
                BareKey::Char('z') => {
                    // Ctrl-Z: Undo
                    self.undo();
                    return InputAction::Continue;
                },
                BareKey::Char('y') => {
                    // Ctrl-Y: Redo
                    self.redo();
                    return InputAction::Continue;
                },
                BareKey::Char('k') => {
                    self.kill_to_end();
                    return InputAction::Continue;
                },
                BareKey::Char('u') => {
                    self.kill_to_start();
                    return InputAction::Continue;
                },
                BareKey::Char('w') => {
                    self.delete_word_backward();
                    return InputAction::Continue;
                },
                BareKey::Char('d') => {
                    self.delete();
                    return InputAction::Continue;
                },
                BareKey::Char('b') => {
                    self.move_left();
                    return InputAction::Continue;
                },
                BareKey::Char('f') => {
                    self.move_right();
                    return InputAction::Continue;
                },
                BareKey::Char('p') => {
                    self.previous_history();
                    return InputAction::Continue;
                },
                BareKey::Char('n') => {
                    self.next_history();
                    return InputAction::Continue;
                },
                BareKey::Left => {
                    self.move_word_left();
                    return InputAction::Continue;
                },
                BareKey::Right => {
                    self.move_word_right();
                    return InputAction::Continue;
                },
                BareKey::Backspace => {
                    self.delete_word_backward();
                    return InputAction::Continue;
                },
                BareKey::Delete => {
                    self.delete_word_forward();
                    return InputAction::Continue;
                },
                BareKey::Char(_) => return InputAction::NoAction,
                _ => {},
            }
        }

        // Check for Alt modifiers
        if key.has_modifiers(&[KeyModifier::Alt]) {
            match key.bare_key {
                BareKey::Left => {
                    self.move_word_left();
                    return InputAction::Continue;
                },
                BareKey::Right => {
                    self.move_word_right();
                    return InputAction::Continue;
                },
                BareKey::Backspace => {
                    self.delete_word_backward();
                    return InputAction::Continue;
                },
                BareKey::Delete => {
                    self.delete_word_forward();
                    return InputAction::Continue;
                },
                _ => {},
            }
        }

        match key.bare_key {
            BareKey::Enter => InputAction::Submit,
            BareKey::Esc => InputAction::Cancel,
            BareKey::Tab => InputAction::Complete,
            BareKey::Backspace => {
                self.backspace();
                InputAction::Continue
            },
            BareKey::Delete => {
                self.delete();
                InputAction::Continue
            },
            BareKey::Left => {
                self.move_left();
                InputAction::Continue
            },
            BareKey::Right => {
                self.move_right();
                InputAction::Continue
            },
            BareKey::Home => {
                self.move_to_start();
                InputAction::Continue
            },
            BareKey::End => {
                self.move_to_end();
                InputAction::Continue
            },
            BareKey::Up => {
                self.previous_history();
                InputAction::Continue
            },
            BareKey::Down => {
                self.next_history();
                InputAction::Continue
            },
            BareKey::Char(c)
                if key.has_no_modifiers() || key.has_only_modifiers(&[KeyModifier::Shift]) =>
            {
                self.insert_char(c);
                InputAction::Continue
            },
            _ => InputAction::NoAction,
        }
    }

    /// Helper: Convert character index to byte index
    fn char_index_to_byte_index(&self, char_index: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_index)
            .map(|(byte_idx, _)| byte_idx)
            .unwrap_or(self.buffer.len())
    }

    /// Save current state to undo stack before making changes
    fn save_undo_state(&mut self) {
        if self.undo_stack.len() >= MAX_UNDO_STACK_SIZE {
            self.undo_stack.remove(0);
        }
        self.undo_stack
            .push((self.buffer.clone(), self.cursor_position));
        self.redo_stack.clear();
    }

    /// Save state only if not coalescing with previous insert
    fn save_undo_state_unless_coalescing(&mut self) {
        if !self.last_edit_was_insert {
            self.save_undo_state();
        }
        self.last_edit_was_insert = true;
    }

    /// Mark that a non-insert edit occurred (breaks coalescing)
    fn break_coalescing(&mut self) {
        self.last_edit_was_insert = false;
    }

    /// Undo last change
    pub fn undo(&mut self) -> bool {
        if let Some((buffer, cursor)) = self.undo_stack.pop() {
            self.redo_stack
                .push((self.buffer.clone(), self.cursor_position));
            self.buffer = buffer;
            self.cursor_position = cursor;
            self.break_coalescing();
            true
        } else {
            false
        }
    }

    /// Redo last undone change
    pub fn redo(&mut self) -> bool {
        if let Some((buffer, cursor)) = self.redo_stack.pop() {
            self.undo_stack
                .push((self.buffer.clone(), self.cursor_position));
            self.buffer = buffer;
            self.cursor_position = cursor;
            self.break_coalescing();
            true
        } else {
            false
        }
    }

    /// Check if undo is available
    #[allow(unused)]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Check if redo is available
    #[allow(unused)]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    #[allow(unused)]
    pub fn drain_text(&mut self) -> String {
        let text = self.buffer.clone();
        if !text.trim().is_empty() {
            self.history.push(text);
        }
        self.reset_history_position();
        self.cursor_position = 0;
        std::mem::take(&mut self.buffer)
    }
}

#[cfg(test)]
#[path = "unit/text_input_tests.rs"]
mod tests;
