//! The server's half of unicode placeholders: what a placeholder cell's marks mean as they are
//! parsed off the wire.
//!
//! The character and the diacritic table themselves live in
//! [`yinz_types::kitty_placeholders`], because plugins emit placeholder cells too and a second
//! copy of 297 codepoints is a second copy to keep right. They are re-exported here so the rest of
//! this module reads as it always did.

pub use yinz_types::kitty_placeholders::{diacritic_value, PLACEHOLDER, ROWCOLUMN_DIACRITICS};

/// The row, column and image-id byte a single placeholder cell spells out, in the order the marks
/// arrive. A field left `None` is inherited from the cell to the left when the placements are
/// resolved, which is what lets a client emit a long run of cells cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaceholderMark {
    /// The screen column of the placeholder cell these marks belong to. Kept in the same units as
    /// `Osc133Marker::column`, the row's other side-band metadata, so the two are maintained
    /// together by the same mutations.
    pub column: usize,
    pub row_in_image: Option<u16>,
    pub column_in_image: Option<u16>,
    pub id_most_significant_byte: Option<u16>,
}

impl PlaceholderMark {
    pub fn new(column: usize) -> Self {
        PlaceholderMark {
            column,
            ..Default::default()
        }
    }
    /// Record the next mark to arrive: first the row, then the column, then the high byte of the
    /// image id. A fourth is not part of the encoding and is ignored.
    pub fn push_value(&mut self, value: u16) {
        if self.row_in_image.is_none() {
            self.row_in_image = Some(value);
        } else if self.column_in_image.is_none() {
            self.column_in_image = Some(value);
        } else if self.id_most_significant_byte.is_none() {
            self.id_most_significant_byte = Some(value);
        }
    }
}

#[cfg(test)]
#[path = "./unit/placeholders_tests.rs"]
mod placeholders_tests;
