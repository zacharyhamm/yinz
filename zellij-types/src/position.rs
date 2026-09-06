use serde::{Deserialize, Serialize};

#[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Deserialize, Serialize)]
pub struct Position {
    pub line: Line,
    pub column: Column,
}

impl Position {
    pub fn new(line: i32, column: u16) -> Self {
        Self {
            line: Line(line as isize),
            column: Column(column as usize),
        }
    }
    pub fn change_line(&mut self, line: isize) {
        self.line = Line(line);
    }

    pub fn change_column(&mut self, column: usize) {
        self.column = Column(column);
    }

    pub fn relative_to(&self, line: usize, column: usize) -> Self {
        Self {
            line: Line(self.line.0 - line as isize),
            column: Column(self.column.0.saturating_sub(column)),
        }
    }

    pub fn line(&self) -> isize {
        self.line.0
    }

    pub fn column(&self) -> usize {
        self.column.0
    }
}

impl Default for Position {
    fn default() -> Self {
        Position::new(0, 0)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, PartialOrd)]
/// A signed line coordinate, including scrollback positions.
///
/// ```compile_fail
/// use zellij_types::position::{Line, Column};
/// let _: Line = Column(0);
/// ```
#[repr(transparent)]
#[serde(transparent)]
pub struct Line(pub isize);
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, PartialOrd)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Column(pub usize);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_lines_and_unsigned_columns_keep_their_json_values() {
        for value in [isize::MIN, -1, 0, isize::MAX] {
            let json = value.to_string();
            assert_eq!(serde_json::to_string(&Line(value)).unwrap(), json);
            assert_eq!(serde_json::from_str::<Line>(&json).unwrap(), Line(value));
        }
        for value in [0, usize::MAX] {
            let json = value.to_string();
            assert_eq!(serde_json::to_string(&Column(value)).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<Column>(&json).unwrap(),
                Column(value)
            );
        }
        assert!(serde_json::from_str::<Column>("-1").is_err());
    }
}
