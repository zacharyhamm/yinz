//! Pane tabs as the pane layer sees them: the tab bar's cells, which a plugin pane draws and
//! `Tab` hit-tests, and the opaque carrier that holds a pane's pane tabs while the pane is out of
//! a `Tab`.

use yinz_types::pane_tabs::PaneTabsInfo;

/// A pane's pane tabs and their hidden children, set on the pane when a `Tab` extracts it and
/// taken back by the `Tab` that next adds or parks it, which after a float or embed toggle is the
/// same one. `Tab` decides the contents; the pane layer only holds them, so it never names `Tab`'s
/// bookkeeping.
pub struct PaneTabsTransfer(Box<dyn std::any::Any>);

impl PaneTabsTransfer {
    pub fn new<T: 'static>(contents: T) -> Self {
        PaneTabsTransfer(Box::new(contents))
    }

    /// `None` when the transfer holds something other than a `T`.
    pub fn into_contents<T: 'static>(self) -> Option<T> {
        self.0.downcast().ok().map(|contents| *contents)
    }
}

/// Host-owned view state, separate from plugin events and persisted layouts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneTabsView {
    pub highlight: usize,
    pub reveal: Option<usize>,
}

pub fn bar_cells(
    info: &PaneTabsInfo,
    view: PaneTabsView,
    focused: bool,
    width: usize,
) -> Vec<yinz_types::tab_bar::TabCell<yinz_types::tab_bar::TabTarget>> {
    let labels: Vec<_> = info
        .tabs
        .iter()
        .map(|slot| {
            let label =
                yinz_types::text::clean_string_from_control_and_linebreak(&slot.descriptor.label);
            format!(" {label} │")
        })
        .collect();
    let focus = focused.then_some(view.highlight);
    let keep = focus.or(view.reveal).unwrap_or(info.active);
    yinz_types::tab_bar::overflow(&labels, width, info.active, focus, keep)
}
