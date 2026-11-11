use unicode_width::UnicodeWidthChar;
use winit::event::Ime;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preedit {
    text: String,
    cursor: Option<(usize, usize)>,
}

impl Preedit {
    pub fn new(text: impl Into<String>, cursor: Option<(usize, usize)>) -> Self {
        Self {
            text: text.into(),
            cursor,
        }
    }

    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn columns(&self) -> Vec<Column> {
        let mut columns: Vec<Column> = Vec::new();
        for (cluster, _) in yinz_types::graphemes::clusters(&self.text) {
            let mut first_of_cluster = true;
            for character in cluster.chars() {
                let width = character.width().unwrap_or(0);
                if width == 0 {
                    if let Some(previous) = columns.last_mut() {
                        previous.text.push(character);
                    }
                    continue;
                }
                columns.push(Column {
                    text: character.to_string(),
                    width,
                    continues: !first_of_cluster,
                });
                first_of_cluster = false;
            }
        }
        columns
    }

    pub fn cursor_column(&self) -> Option<usize> {
        let (_, end) = self.cursor?;
        let end = end.min(self.text.len());
        let mut column = 0;
        for (offset, character) in self.text.char_indices() {
            if offset >= end {
                break;
            }
            column += character.width().unwrap_or(0);
        }
        Some(column)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub text: String,
    pub width: usize,
    pub continues: bool,
}

impl Column {
    pub fn character(&self) -> char {
        self.text.chars().next().unwrap_or(' ')
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Open,
    Resolving,
    Composing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    Nothing,
    Redraw,
    Commit(String),
}

#[derive(Debug, Default)]
pub struct Composition {
    preedit: Option<Preedit>,
    dead: bool,
    accent: Option<char>,
}

impl Composition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn gate(&self) -> Gate {
        if self.preedit.is_some() {
            return Gate::Composing;
        }
        if self.dead {
            return Gate::Resolving;
        }
        Gate::Open
    }

    pub fn shown(&self) -> Option<Preedit> {
        if let Some(preedit) = self.preedit.as_ref() {
            return Some(preedit.clone());
        }
        let accent = self.accent?;
        let text = accent.to_string();
        let end = text.len();
        Some(Preedit::new(text, Some((end, end))))
    }

    pub fn ime(&mut self, event: Ime) -> Reaction {
        match event {
            Ime::Enabled => Reaction::Nothing,
            Ime::Preedit(text, cursor) => {
                let preedit = (!text.is_empty()).then(|| Preedit::new(text, cursor));
                if preedit.is_some() {
                    self.forget_dead();
                }
                self.replace(preedit)
            },
            Ime::Commit(text) => {
                self.preedit = None;
                self.forget_dead();
                Reaction::Commit(text)
            },
            Ime::Disabled => self.cancel(),
        }
    }

    pub fn dead_key(&mut self, accent: Option<char>) -> Reaction {
        let shown = self.shown();
        self.dead = true;
        self.accent = accent;
        self.redraw_if(shown)
    }

    pub fn resolve(&mut self) -> Reaction {
        let shown = self.shown();
        self.forget_dead();
        self.redraw_if(shown)
    }

    pub fn cancel(&mut self) -> Reaction {
        let shown = self.shown();
        self.preedit = None;
        self.forget_dead();
        self.redraw_if(shown)
    }

    fn replace(&mut self, preedit: Option<Preedit>) -> Reaction {
        let shown = self.shown();
        self.preedit = preedit;
        self.redraw_if(shown)
    }

    fn forget_dead(&mut self) {
        self.dead = false;
        self.accent = None;
    }

    fn redraw_if(&self, before: Option<Preedit>) -> Reaction {
        if before == self.shown() {
            Reaction::Nothing
        } else {
            Reaction::Redraw
        }
    }
}

#[cfg(test)]
#[path = "unit/composition_tests.rs"]
mod tests;
