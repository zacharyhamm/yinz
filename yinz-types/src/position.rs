use serde::{Deserialize, Serialize};

#[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, PartialOrd, Deserialize, Serialize)]
pub struct Position {
    pub line: Line,
    pub column: Column,
}

impl Position {
    pub fn new(line: i32, column: u16) -> Self {
        Self {
            line: Line::new(line as isize),
            column: Column::new(column as usize),
        }
    }
    pub fn change_line(&mut self, line: isize) {
        self.line = Line::new(line);
    }

    pub fn change_column(&mut self, column: usize) {
        self.column = Column::new(column);
    }

    pub fn relative_to(&self, line: usize, column: usize) -> Self {
        Self {
            line: Line::new(self.line.get() - line as isize),
            column: Column::new(self.column.get().saturating_sub(column)),
        }
    }

    pub fn line(&self) -> isize {
        self.line.get()
    }

    pub fn column(&self) -> usize {
        self.column.get()
    }
}

impl Default for Position {
    fn default() -> Self {
        Position::new(0, 0)
    }
}

macro_rules! coordinate {
    ($(#[$meta:meta])* $name:ident($scalar:ty)) => {
        $(#[$meta])*
        #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, PartialOrd, Ord)]
        #[repr(transparent)]
        #[serde(transparent)]
        pub struct $name($scalar);
        impl $name {
            pub const fn new(value: $scalar) -> Self { Self(value) }
            pub const fn get(self) -> $scalar { self.0 }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
        }
    };
}
coordinate!(
    /// A signed line coordinate, including scrollback positions.
    /// ```compile_fail
    /// use yinz_types::position::{Line, Column};
    /// let _: Line = Column::new(0);
    /// ```
    Line(isize)
);
coordinate!(
    /// A zero-based cell column.
    Column(usize)
);
coordinate!(
    /// A zero-based visible cell row.
    Row(usize)
);
coordinate!(
    /// A height measured in terminal cell rows.
    /// ```compile_fail
    /// use yinz_types::position::{Row, RowCount};
    /// let _: RowCount = Row::new(0);
    /// ```
    RowCount(usize)
);
coordinate!(
    /// A width measured in terminal cell columns.
    ColumnCount(usize)
);

/// Visible cursor coordinates, serialized as the legacy `[x, y]` array.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "(usize, usize)", into = "(usize, usize)")]
pub struct CellCoordinates {
    pub x: Column,
    pub y: Row,
}
impl CellCoordinates {
    pub const fn new(x: Column, y: Row) -> Self {
        Self { x, y }
    }
    pub const fn into_tuple(self) -> (usize, usize) {
        (self.x.get(), self.y.get())
    }
}
impl From<(usize, usize)> for CellCoordinates {
    fn from((x, y): (usize, usize)) -> Self {
        Self::new(Column::new(x), Row::new(y))
    }
}
impl From<CellCoordinates> for (usize, usize) {
    fn from(value: CellCoordinates) -> Self {
        value.into_tuple()
    }
}

#[cfg(test)]
#[path = "unit/position_tests.rs"]
mod tests;

macro_rules! checked_coordinate_conversions {
    ($name:ident($scalar:ty); from $($from:ty),*; into $($into:ty),*) => {
        $(impl TryFrom<$from> for $name {
            type Error = std::num::TryFromIntError;
            fn try_from(value: $from) -> Result<Self, Self::Error> {
                <$scalar>::try_from(value).map(Self::new)
            }
        })*
        $(impl TryFrom<$name> for $into {
            type Error = std::num::TryFromIntError;
            fn try_from(value: $name) -> Result<Self, Self::Error> {
                Self::try_from(value.get())
            }
        })*
    };
}
checked_coordinate_conversions!(Line(isize); from i64, u64, u32; into i32, i64);
checked_coordinate_conversions!(Column(usize); from i64, u64, u32; into u16, u32, i64);
checked_coordinate_conversions!(Row(usize); from i64, u64, u32; into u16, u32, i64);
checked_coordinate_conversions!(RowCount(usize); from i64, u64, u32; into u16, u32, i64);
checked_coordinate_conversions!(ColumnCount(usize); from i64, u64, u32; into u16, u32, i64);
