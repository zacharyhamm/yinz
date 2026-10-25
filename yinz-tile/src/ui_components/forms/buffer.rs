//! Component output without host calls. Native callers deliver it themselves.
use crate::ui_components::{serialize_text_with_coordinates, Text};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RenderBuffer {
    pub output: String,
    /// None leaves the cursor alone; Some(None) hides it.
    pub cursor: Option<Option<(usize, usize)>>,
}

impl RenderBuffer {
    pub(super) fn text(
        &mut self,
        text: Text,
        x: usize,
        y: usize,
        width: Option<usize>,
        height: Option<usize>,
    ) {
        self.output
            .push_str(&serialize_text_with_coordinates(&text, x, y, width, height));
    }
}

#[cfg(test)]
#[path = "unit/buffer.rs"]
mod tests;
