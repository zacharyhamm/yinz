pub use super::generated_api::api::pane_frame_style::PaneFrameStyle as ProtobufPaneFrameStyle;
use yinz_types::input::options::PaneFrameStyle;

use std::convert::TryFrom;

impl TryFrom<ProtobufPaneFrameStyle> for PaneFrameStyle {
    type Error = &'static str;
    fn try_from(protobuf_pane_frame_style: ProtobufPaneFrameStyle) -> Result<Self, &'static str> {
        match protobuf_pane_frame_style {
            ProtobufPaneFrameStyle::Full => Ok(PaneFrameStyle::Full),
            ProtobufPaneFrameStyle::Titles => Ok(PaneFrameStyle::Titles),
            ProtobufPaneFrameStyle::None => Ok(PaneFrameStyle::None),
        }
    }
}

impl TryFrom<PaneFrameStyle> for ProtobufPaneFrameStyle {
    type Error = &'static str;
    fn try_from(pane_frame_style: PaneFrameStyle) -> Result<Self, &'static str> {
        Ok(match pane_frame_style {
            PaneFrameStyle::Full => ProtobufPaneFrameStyle::Full,
            PaneFrameStyle::Titles => ProtobufPaneFrameStyle::Titles,
            PaneFrameStyle::None => ProtobufPaneFrameStyle::None,
        })
    }
}

#[cfg(test)]
#[path = "unit/pane_frame_style_tests.rs"]
mod tests;
