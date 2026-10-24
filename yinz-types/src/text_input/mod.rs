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
mod tests {
    use super::*;
    use hegel::generators;

    #[test]
    fn test_new_and_empty() {
        let input = TextInput::new("hello".to_string());
        assert_eq!(input.get_text(), "hello");
        assert_eq!(input.get_cursor_position(), 5);

        let empty = TextInput::empty();
        assert_eq!(empty.get_text(), "");
        assert_eq!(empty.get_cursor_position(), 0);
    }

    #[test]
    fn test_insert_char() {
        let mut input = TextInput::new("helo".to_string());
        input.cursor_position = 3; // Position after "hel"
        input.insert_char('l');
        assert_eq!(input.get_text(), "hello");
        assert_eq!(input.get_cursor_position(), 4);
    }

    #[test]
    fn test_insert_text_at_cursor() {
        // Start, middle and end all splice at the cursor.
        let mut input = TextInput::new("bar".to_string());
        input.cursor_position = 0;
        input.insert_text("foo ");
        assert_eq!(input.get_text(), "foo bar");
        assert_eq!(input.get_cursor_position(), 4);

        input.cursor_position = 4;
        input.insert_text("big ");
        assert_eq!(input.get_text(), "foo big bar");
        assert_eq!(input.get_cursor_position(), 8);

        input.move_to_end();
        input.insert_text("!");
        assert_eq!(input.get_text(), "foo big bar!");
        assert_eq!(input.get_cursor_position(), 12);
    }

    #[test]
    fn test_insert_text_multibyte() {
        // The cursor is a char index and the buffer index is bytes, so a
        // multi-byte prefix is where an off-by-one would land mid-codepoint.
        let mut input = TextInput::new("héllo".to_string());
        input.cursor_position = 2; // after "hé"
        input.insert_text("→ø");
        assert_eq!(input.get_text(), "hé→øllo");
        assert_eq!(input.get_cursor_position(), 4);
    }

    #[test]
    fn test_insert_text_normalizes_newlines() {
        let mut input = TextInput::empty();
        input.insert_text("a\r\nb\rc\nd");
        assert_eq!(input.get_text(), "a\nb\nc\nd");
        assert_eq!(input.get_cursor_position(), 7);
    }

    #[test]
    fn test_insert_text_strips_control_chars() {
        let mut input = TextInput::empty();
        input.insert_text("a\x1b[31mb\x07\tc");
        assert_eq!(input.get_text(), "a[31mb\tc");

        // A paste of nothing but control characters changes nothing, and
        // leaves no undo step behind either.
        let mut empty = TextInput::empty();
        empty.insert_text("\x07\x00");
        assert_eq!(empty.get_text(), "");
        assert!(!empty.can_undo());
    }

    #[test]
    fn test_insert_text_is_one_undo_step() {
        let mut input = TextInput::new("ab".to_string());
        input.insert_char('c');
        input.insert_text("hello world");
        assert_eq!(input.get_text(), "abchello world");

        // The paste goes back whole, and typing after it does not fold in.
        assert!(input.undo());
        assert_eq!(input.get_text(), "abc");
        assert_eq!(input.get_cursor_position(), 3);
    }

    #[test]
    fn test_insert_text_after_paste_does_not_coalesce() {
        let mut input = TextInput::empty();
        input.insert_text("pasted");
        input.insert_char('x');
        assert!(input.undo());
        assert_eq!(input.get_text(), "pasted");
    }

    #[test]
    fn test_backspace() {
        let mut input = TextInput::new("hello".to_string());
        input.backspace();
        assert_eq!(input.get_text(), "hell");
        assert_eq!(input.get_cursor_position(), 4);

        // Backspace at start does nothing
        input.cursor_position = 0;
        input.backspace();
        assert_eq!(input.get_text(), "hell");
        assert_eq!(input.get_cursor_position(), 0);
    }

    #[test]
    fn test_delete() {
        let mut input = TextInput::new("hello".to_string());
        input.cursor_position = 0;
        input.delete();
        assert_eq!(input.get_text(), "ello");
        assert_eq!(input.get_cursor_position(), 0);

        // Delete at end does nothing
        input.move_to_end();
        input.delete();
        assert_eq!(input.get_text(), "ello");
    }

    #[test]
    fn test_cursor_movement() {
        let mut input = TextInput::new("hello".to_string());
        assert_eq!(input.get_cursor_position(), 5);

        input.move_left();
        assert_eq!(input.get_cursor_position(), 4);

        input.move_right();
        assert_eq!(input.get_cursor_position(), 5);

        input.move_to_start();
        assert_eq!(input.get_cursor_position(), 0);

        input.move_to_end();
        assert_eq!(input.get_cursor_position(), 5);
    }

    #[test]
    fn test_unicode_support() {
        let mut input = TextInput::new("hello 🦀 world".to_string());
        assert_eq!(input.get_cursor_position(), 13); // 13 characters

        input.cursor_position = 6; // After "hello "
        input.insert_char('🐱');
        assert_eq!(input.get_text(), "hello 🐱🦀 world");
    }

    #[test]
    fn test_word_jump_right() {
        let mut input = TextInput::new("hello world foo bar".to_string());
        input.cursor_position = 0;

        // Jump from start to "world"
        input.move_word_right();
        assert_eq!(input.get_cursor_position(), 6); // After "hello "

        // Jump to "foo"
        input.move_word_right();
        assert_eq!(input.get_cursor_position(), 12); // After "world "

        // Jump to "bar"
        input.move_word_right();
        assert_eq!(input.get_cursor_position(), 16); // After "foo "

        // Jump to end
        input.move_word_right();
        assert_eq!(input.get_cursor_position(), 19); // At end
    }

    #[test]
    fn test_word_jump_left() {
        let mut input = TextInput::new("hello world foo bar".to_string());
        input.move_to_end();
        assert_eq!(input.get_cursor_position(), 19);

        // Jump back to "bar"
        input.move_word_left();
        assert_eq!(input.get_cursor_position(), 16); // Start of "bar"

        // Jump back to "foo"
        input.move_word_left();
        assert_eq!(input.get_cursor_position(), 12); // Start of "foo"

        // Jump back to "world"
        input.move_word_left();
        assert_eq!(input.get_cursor_position(), 6); // Start of "world"

        // Jump back to "hello"
        input.move_word_left();
        assert_eq!(input.get_cursor_position(), 0); // Start of "hello"
    }

    #[test]
    fn test_word_jump_with_multiple_spaces() {
        let mut input = TextInput::new("hello   world".to_string());
        input.cursor_position = 0;

        // Jump over multiple spaces
        input.move_word_right();
        assert_eq!(input.get_cursor_position(), 8); // After "hello   ", at start of "world"

        // Jump back should skip spaces
        input.move_word_left();
        assert_eq!(input.get_cursor_position(), 0); // Back to start of "hello"
    }

    #[test]
    fn test_word_jump_boundaries() {
        let mut input = TextInput::new("test".to_string());

        // At start - word left does nothing
        input.cursor_position = 0;
        input.move_word_left();
        assert_eq!(input.get_cursor_position(), 0);

        // At end - word right does nothing
        input.move_to_end();
        let end_pos = input.get_cursor_position();
        input.move_word_right();
        assert_eq!(input.get_cursor_position(), end_pos);
    }

    #[test]
    fn test_up_down_arrows() {
        let mut input = TextInput::new("hello world".to_string());

        // Start in the middle
        input.cursor_position = 5;
        assert_eq!(input.get_cursor_position(), 5);

        // Up arrow should go to start
        input.move_to_start();
        assert_eq!(input.get_cursor_position(), 0);

        // Move back to middle
        input.cursor_position = 5;

        // Down arrow should go to end
        input.move_to_end();
        assert_eq!(input.get_cursor_position(), 11);
    }

    #[test]
    fn test_delete_word_backward() {
        let mut input = TextInput::new("hello world foo".to_string());

        // Delete "foo" from end
        input.move_to_end();
        input.delete_word_backward();
        assert_eq!(input.get_text(), "hello world ");
        assert_eq!(input.get_cursor_position(), 12);

        // Delete "world "
        input.delete_word_backward();
        assert_eq!(input.get_text(), "hello ");
        assert_eq!(input.get_cursor_position(), 6);

        // Delete "hello "
        input.delete_word_backward();
        assert_eq!(input.get_text(), "");
        assert_eq!(input.get_cursor_position(), 0);

        // Delete on empty buffer does nothing
        input.delete_word_backward();
        assert_eq!(input.get_text(), "");
        assert_eq!(input.get_cursor_position(), 0);
    }

    #[test]
    fn test_delete_word_backward_middle() {
        let mut input = TextInput::new("hello world foo".to_string());

        // Position in middle of "world"
        input.cursor_position = 8; // After "hello wo"
        input.delete_word_backward();
        assert_eq!(input.get_text(), "hello rld foo");
        assert_eq!(input.get_cursor_position(), 6); // After "hello "
    }

    #[test]
    fn test_delete_word_forward() {
        let mut input = TextInput::new("hello world foo".to_string());

        // Delete "hello " from start
        input.cursor_position = 0;
        input.delete_word_forward();
        assert_eq!(input.get_text(), "world foo");
        assert_eq!(input.get_cursor_position(), 0);

        // Delete "world "
        input.delete_word_forward();
        assert_eq!(input.get_text(), "foo");
        assert_eq!(input.get_cursor_position(), 0);

        // Delete "foo"
        input.delete_word_forward();
        assert_eq!(input.get_text(), "");
        assert_eq!(input.get_cursor_position(), 0);

        // Delete on empty buffer does nothing
        input.delete_word_forward();
        assert_eq!(input.get_text(), "");
        assert_eq!(input.get_cursor_position(), 0);
    }

    #[test]
    fn test_delete_word_forward_middle() {
        let mut input = TextInput::new("hello world foo".to_string());

        // Position in middle of "world"
        input.cursor_position = 8; // After "hello wo"
        input.delete_word_forward();
        assert_eq!(input.get_text(), "hello wofoo");
        assert_eq!(input.get_cursor_position(), 8); // Same position, text deleted forward
    }

    #[test]
    fn test_delete_word_with_multiple_spaces() {
        let mut input = TextInput::new("hello   world".to_string());

        // Delete forward includes trailing spaces
        input.cursor_position = 0;
        input.delete_word_forward();
        assert_eq!(input.get_text(), "world");
        assert_eq!(input.get_cursor_position(), 0);
    }

    #[test]
    fn test_kill_to_end() {
        let mut input = TextInput::new("hello world".to_string());
        input.cursor_position = 5;
        input.kill_to_end();
        assert_eq!(input.get_text(), "hello");
        assert_eq!(input.get_cursor_position(), 5);

        // At the end: no-op
        input.kill_to_end();
        assert_eq!(input.get_text(), "hello");

        // Multi-line: stops at the newline
        let mut input = TextInput::new("ab\ncd".to_string());
        input.cursor_position = 1;
        input.kill_to_end();
        assert_eq!(input.get_text(), "a\ncd");

        // Sitting on the newline: eats it
        let mut input = TextInput::new("ab\ncd".to_string());
        input.cursor_position = 2;
        input.kill_to_end();
        assert_eq!(input.get_text(), "abcd");
    }

    #[test]
    fn test_kill_to_start() {
        let mut input = TextInput::new("hello world".to_string());
        input.cursor_position = 6;
        input.kill_to_start();
        assert_eq!(input.get_text(), "world");
        assert_eq!(input.get_cursor_position(), 0);

        // At the start: no-op
        input.kill_to_start();
        assert_eq!(input.get_text(), "world");

        // Multi-line: leaves the preceding line alone
        let mut input = TextInput::new("ab\ncd".to_string());
        input.cursor_position = 4;
        input.kill_to_start();
        assert_eq!(input.get_text(), "ab\nd");
        assert_eq!(input.get_cursor_position(), 3);
    }

    #[test]
    fn test_kill_undo() {
        let mut input = TextInput::new("hello world".to_string());
        input.cursor_position = 5;
        input.kill_to_end();
        input.undo();
        assert_eq!(input.get_text(), "hello world");

        input.cursor_position = 6;
        input.kill_to_start();
        input.undo();
        assert_eq!(input.get_text(), "hello world");
    }

    #[test]
    fn test_ctrl_char_not_inserted() {
        let mut input = TextInput::new("hi".to_string());

        let ctrl_g = KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier();
        assert_eq!(input.handle_key(ctrl_g), InputAction::NoAction);
        assert_eq!(input.get_text(), "hi");

        // Ctrl-Shift-Z still redoes despite the new Ctrl catch-all
        input.backspace();
        input.undo();
        assert_eq!(input.get_text(), "hi");
        let ctrl_shift_z = KeyWithModifier::new(BareKey::Char('Z'))
            .with_ctrl_modifier()
            .with_shift_modifier();
        assert_eq!(input.handle_key(ctrl_shift_z), InputAction::Continue);
        assert_eq!(input.get_text(), "h");
    }

    #[test]
    fn history_keys_select_entries_and_restore_the_draft() {
        let mut input = TextInput::new("draft".to_owned());
        input.set_history(vec!["first".to_owned(), "second".to_owned()]);

        input.handle_key(KeyWithModifier::new(BareKey::Up));
        assert_eq!(input.get_text(), "second");
        assert_eq!(input.get_cursor_position(), 6);

        input.handle_key(KeyWithModifier::new(BareKey::Char('p')).with_ctrl_modifier());
        input.handle_key(KeyWithModifier::new(BareKey::Up));
        assert_eq!(input.get_text(), "first");

        input.handle_key(KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier());
        assert_eq!(input.get_text(), "second");
        input.handle_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(input.get_text(), "draft");

        input.clear();
        input.set_text("  ".to_owned());
        input.drain_text();
        assert_eq!(input.history(), ["first", "second"]);
    }

    #[test]
    fn test_undo_redo_basic() {
        let mut input = TextInput::empty();

        // Type "hello"
        input.insert_char('h');
        input.insert_char('e');
        input.insert_char('l');
        input.insert_char('l');
        input.insert_char('o');
        assert_eq!(input.get_text(), "hello");

        // Undo should remove all characters (coalesced into one undo entry)
        assert!(input.can_undo());
        assert!(input.undo());
        assert_eq!(input.get_text(), "");
        assert_eq!(input.get_cursor_position(), 0);

        // Redo should restore "hello"
        assert!(input.can_redo());
        assert!(input.redo());
        assert_eq!(input.get_text(), "hello");
        assert_eq!(input.get_cursor_position(), 5);
    }

    #[test]
    fn test_undo_coalescing_breaks_on_cursor_move() {
        let mut input = TextInput::empty();

        // Type "he"
        input.insert_char('h');
        input.insert_char('e');

        // Move cursor to start (breaks coalescing)
        input.move_to_start();

        // Type "llo"
        input.insert_char('l');
        input.insert_char('l');
        input.insert_char('o');

        assert_eq!(input.get_text(), "llohe");

        // First undo removes "llo" (second coalesced group)
        input.undo();
        assert_eq!(input.get_text(), "he");

        // Second undo removes "he" (first coalesced group)
        input.undo();
        assert_eq!(input.get_text(), "");
    }

    #[test]
    fn test_undo_backspace() {
        let mut input = TextInput::new("hello".to_string());

        // Backspace once
        input.backspace();
        assert_eq!(input.get_text(), "hell");

        // Undo should restore "hello"
        input.undo();
        assert_eq!(input.get_text(), "hello");
        assert_eq!(input.get_cursor_position(), 5);
    }

    #[test]
    fn test_undo_delete() {
        let mut input = TextInput::new("hello".to_string());
        input.cursor_position = 0;

        // Delete first character
        input.delete();
        assert_eq!(input.get_text(), "ello");

        // Undo should restore "hello"
        input.undo();
        assert_eq!(input.get_text(), "hello");
        assert_eq!(input.get_cursor_position(), 0);
    }

    #[test]
    fn test_undo_word_delete() {
        let mut input = TextInput::new("hello world".to_string());

        // Delete "world" backward
        input.delete_word_backward();
        assert_eq!(input.get_text(), "hello ");

        // Undo should restore "hello world"
        input.undo();
        assert_eq!(input.get_text(), "hello world");
        assert_eq!(input.get_cursor_position(), 11);
    }

    #[test]
    fn test_undo_clear() {
        let mut input = TextInput::new("hello world".to_string());

        // Clear the buffer
        input.clear();
        assert_eq!(input.get_text(), "");

        // Undo should restore the text
        input.undo();
        assert_eq!(input.get_text(), "hello world");
    }

    #[test]
    fn test_undo_set_text() {
        let mut input = TextInput::new("hello".to_string());

        // Replace with new text
        input.set_text("goodbye".to_string());
        assert_eq!(input.get_text(), "goodbye");

        // Undo should restore "hello"
        input.undo();
        assert_eq!(input.get_text(), "hello");
    }

    #[test]
    fn test_redo_clears_on_new_edit() {
        let mut input = TextInput::empty();

        // Type "hello"
        input.insert_char('h');
        input.insert_char('e');
        input.insert_char('l');
        input.insert_char('l');
        input.insert_char('o');

        // Undo
        input.undo();
        assert_eq!(input.get_text(), "");
        assert!(input.can_redo());

        // Make a new edit (should clear redo stack)
        input.insert_char('x');
        assert!(!input.can_redo());
    }

    #[test]
    fn test_multiple_undo_redo() {
        let mut input = TextInput::empty();

        // First edit: type "hello"
        for c in "hello".chars() {
            input.insert_char(c);
        }

        // Break coalescing
        input.move_left();

        // Second edit: type "world"
        for c in "world".chars() {
            input.insert_char(c);
        }

        assert_eq!(input.get_text(), "hellworldo");

        // Undo "world"
        input.undo();
        assert_eq!(input.get_text(), "hello");

        // Undo "hello"
        input.undo();
        assert_eq!(input.get_text(), "");

        // Redo "hello"
        input.redo();
        assert_eq!(input.get_text(), "hello");

        // Redo "world"
        input.redo();
        assert_eq!(input.get_text(), "hellworldo");
    }

    #[test]
    fn test_undo_stack_limit() {
        let mut input = TextInput::empty();

        // Perform 102 separate edits (breaking coalescing each time)
        for _i in 0..102 {
            input.backspace(); // Break coalescing
            input.insert_char('x');
        }

        // Should have at most 100 undo entries
        let mut undo_count = 0;
        while input.undo() {
            undo_count += 1;
        }

        // We should have 100 undo entries (the stack limit)
        // Plus the final state change from the last coalescing break
        assert!(
            undo_count <= 100,
            "Undo count should be at most 100, got {}",
            undo_count
        );
    }

    #[test]
    fn test_undo_redo_empty_stack() {
        let mut input = TextInput::empty();

        // Undo on empty stack should return false
        assert!(!input.can_undo());
        assert!(!input.undo());

        // Redo on empty stack should return false
        assert!(!input.can_redo());
        assert!(!input.redo());
    }

    #[test]
    fn test_undo_restores_cursor_position() {
        let mut input = TextInput::new("hello world".to_string());

        // Move cursor to position 5 (before "world")
        input.cursor_position = 5;

        // Insert a space
        input.insert_char(' ');
        assert_eq!(input.get_text(), "hello  world");
        assert_eq!(input.get_cursor_position(), 6);

        // Undo should restore both text and cursor position
        input.undo();
        assert_eq!(input.get_text(), "hello world");
        assert_eq!(input.get_cursor_position(), 5);
    }

    #[test]
    fn test_coalescing_consecutive_inserts() {
        let mut input = TextInput::empty();

        // Type several characters
        input.insert_char('a');
        input.insert_char('b');
        input.insert_char('c');

        assert_eq!(input.get_text(), "abc");

        // Single undo should remove all three (they were coalesced)
        input.undo();
        assert_eq!(input.get_text(), "");

        // No more undo available
        assert!(!input.can_undo());
    }

    #[test]
    fn test_backspace_breaks_coalescing() {
        let mut input = TextInput::empty();

        // Type "ab"
        input.insert_char('a');
        input.insert_char('b');

        // Backspace
        input.backspace();
        assert_eq!(input.get_text(), "a");

        // Type "c"
        input.insert_char('c');
        assert_eq!(input.get_text(), "ac");

        // Undo should remove just "c"
        input.undo();
        assert_eq!(input.get_text(), "a");

        // Undo should remove backspace operation
        input.undo();
        assert_eq!(input.get_text(), "ab");

        // Undo should remove "ab"
        input.undo();
        assert_eq!(input.get_text(), "");
    }
    /// Any single `char`, drawn from hegel's full-Unicode text generator (combining marks,
    /// astral plane, exotic whitespace like U+00A0 / U+2028 -- not just ASCII).
    #[hegel::composite]
    fn any_char(tc: &hegel::TestCase) -> char {
        let s = tc.draw(generators::text().min_size(1));
        s.chars().next().unwrap()
    }

    /// `TextInput` checked against the obvious model: a `Vec<char>` plus a char cursor.
    struct TextInputModel {
        real: TextInput,
        buf: Vec<char>,
        cursor: usize,
    }

    impl TextInputModel {
        fn model_text(&self) -> String {
            self.buf.iter().collect()
        }

        /// Mirror of `TextInput::move_word_left`.
        fn model_word_left(&self) -> usize {
            let mut pos = self.cursor;
            while pos > 0 && self.buf[pos - 1].is_whitespace() {
                pos -= 1;
            }
            while pos > 0 && !self.buf[pos - 1].is_whitespace() {
                pos -= 1;
            }
            pos
        }

        /// Mirror of `TextInput::move_word_right`.
        fn model_word_right(&self) -> usize {
            let len = self.buf.len();
            let mut pos = self.cursor;
            while pos < len && !self.buf[pos].is_whitespace() {
                pos += 1;
            }
            while pos < len && self.buf[pos].is_whitespace() {
                pos += 1;
            }
            pos
        }
    }

    #[hegel::state_machine]
    impl TextInputModel {
        #[rule]
        fn insert_char(&mut self, tc: hegel::TestCase) {
            let c = tc.draw(any_char());
            self.real.insert_char(c);
            self.buf.insert(self.cursor, c);
            self.cursor += 1;
        }

        #[rule]
        fn backspace(&mut self, _: hegel::TestCase) {
            self.real.backspace();
            if self.cursor > 0 {
                self.cursor -= 1;
                self.buf.remove(self.cursor);
            }
        }

        #[rule]
        fn delete(&mut self, _: hegel::TestCase) {
            self.real.delete();
            if self.cursor < self.buf.len() {
                self.buf.remove(self.cursor);
            }
        }

        #[rule]
        fn delete_word_backward(&mut self, _: hegel::TestCase) {
            self.real.delete_word_backward();
            if self.cursor > 0 {
                let new_position = self.model_word_left();
                self.buf.drain(new_position..self.cursor);
                self.cursor = new_position;
            }
        }

        #[rule]
        fn delete_word_forward(&mut self, _: hegel::TestCase) {
            self.real.delete_word_forward();
            if self.cursor < self.buf.len() {
                let end = self.model_word_right();
                self.buf.drain(self.cursor..end);
            }
        }

        #[rule]
        fn move_left(&mut self, _: hegel::TestCase) {
            self.real.move_left();
            self.cursor = self.cursor.saturating_sub(1);
        }

        #[rule]
        fn move_right(&mut self, _: hegel::TestCase) {
            self.real.move_right();
            if self.cursor < self.buf.len() {
                self.cursor += 1;
            }
        }

        #[rule]
        fn move_to_start(&mut self, _: hegel::TestCase) {
            self.real.move_to_start();
            self.cursor = 0;
        }

        #[rule]
        fn move_to_end(&mut self, _: hegel::TestCase) {
            self.real.move_to_end();
            self.cursor = self.buf.len();
        }

        #[rule]
        fn move_word_left(&mut self, _: hegel::TestCase) {
            self.real.move_word_left();
            self.cursor = self.model_word_left();
        }

        #[rule]
        fn move_word_right(&mut self, _: hegel::TestCase) {
            self.real.move_word_right();
            self.cursor = self.model_word_right();
        }

        #[rule]
        fn clear(&mut self, _: hegel::TestCase) {
            self.real.clear();
            self.buf.clear();
            self.cursor = 0;
        }

        #[rule]
        fn set_text(&mut self, tc: hegel::TestCase) {
            let text = tc.draw(generators::text());
            self.real.set_text(text.clone());
            self.buf = text.chars().collect();
            self.cursor = self.buf.len();
        }

        #[rule]
        fn set_cursor_position(&mut self, tc: hegel::TestCase) {
            let pos = tc.draw(generators::integers::<usize>());
            self.real.set_cursor_position(pos);
            self.cursor = pos.min(self.buf.len());
        }

        #[rule]
        fn drain_text(&mut self, _: hegel::TestCase) {
            let drained = self.real.drain_text();
            assert_eq!(
                drained,
                self.model_text(),
                "drain_text returned {:?}, model held {:?}",
                drained,
                self.model_text()
            );
            self.buf.clear();
            self.cursor = 0;
        }

        #[invariant]
        fn matches_the_model(&self, _: hegel::TestCase) {
            assert_eq!(
                self.real.get_text(),
                self.model_text(),
                "buffer {:?} diverged from model {:?} (cursor {})",
                self.real.get_text(),
                self.model_text(),
                self.cursor
            );
            assert_eq!(
                self.real.get_cursor_position(),
                self.cursor,
                "cursor {} diverged from model cursor {} (buffer {:?})",
                self.real.get_cursor_position(),
                self.cursor,
                self.real.get_text()
            );
        }
    }

    #[hegel::test]
    fn text_input_matches_a_naive_char_buffer(tc: hegel::TestCase) {
        let machine = TextInputModel {
            real: TextInput::empty(),
            buf: Vec::new(),
            cursor: 0,
        };
        hegel::stateful::run(machine, tc);
    }

    /// Mutating operations used to drive the undo/redo property.
    #[derive(Debug, Clone)]
    enum Op {
        Insert(char),
        Backspace,
        Delete,
        DeleteWordBackward,
        DeleteWordForward,
        MoveLeft,
        MoveRight,
        MoveToStart,
        MoveToEnd,
        Clear,
        SetText(String),
    }

    impl Op {
        fn apply(&self, input: &mut TextInput) {
            match self {
                Op::Insert(c) => input.insert_char(*c),
                Op::Backspace => input.backspace(),
                Op::Delete => input.delete(),
                Op::DeleteWordBackward => input.delete_word_backward(),
                Op::DeleteWordForward => input.delete_word_forward(),
                Op::MoveLeft => input.move_left(),
                Op::MoveRight => input.move_right(),
                Op::MoveToStart => input.move_to_start(),
                Op::MoveToEnd => input.move_to_end(),
                Op::Clear => input.clear(),
                Op::SetText(s) => input.set_text(s.clone()),
            }
        }
    }

    #[hegel::composite]
    fn any_op(tc: &hegel::TestCase) -> Op {
        let which = tc.draw(generators::integers::<u8>().max_value(10));
        match which {
            0 => Op::Insert(tc.draw(any_char())),
            1 => Op::Backspace,
            2 => Op::Delete,
            3 => Op::DeleteWordBackward,
            4 => Op::DeleteWordForward,
            5 => Op::MoveLeft,
            6 => Op::MoveRight,
            7 => Op::MoveToStart,
            8 => Op::MoveToEnd,
            9 => Op::Clear,
            _ => Op::SetText(tc.draw(generators::text())),
        }
    }

    /// After any sequence of edits, an `undo` that succeeds must be exactly undone by a `redo`.
    #[hegel::test]
    fn undo_then_redo_restores_the_state(tc: hegel::TestCase) {
        let initial = tc.draw(generators::text());
        let ops = tc.draw(generators::vecs(any_op()));
        let mut input = TextInput::new(initial.clone());
        for op in &ops {
            op.apply(&mut input);
        }
        let text_before = input.get_text().to_string();
        let cursor_before = input.get_cursor_position();
        if !input.undo() {
            return;
        }
        assert!(
            input.redo(),
            "undo succeeded but redo did not, after {:?} on {:?}",
            ops,
            initial
        );
        assert_eq!(
            input.get_text(),
            text_before,
            "redo gave {:?} instead of {:?}, after {:?} on {:?}",
            input.get_text(),
            text_before,
            ops,
            initial
        );
        assert_eq!(
            input.get_cursor_position(),
            cursor_before,
            "redo gave cursor {} instead of {}, after {:?} on {:?}",
            input.get_cursor_position(),
            cursor_before,
            ops,
            initial
        );
    }
}
