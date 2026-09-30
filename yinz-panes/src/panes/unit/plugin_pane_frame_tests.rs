use super::tests::{frame_params, test_plugin_pane};
use crate::pane::Pane;
use yinz_types::data::InputMode;
use yinz_types::internal_ids::ClientId;
use yinz_types::pane_size::Offset;

#[test]
fn frames_are_cached_per_client_but_pinned_frames_redraw() {
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    let mut pane = test_plugin_pane(5, 60, Offset::frame(1));
    let render = |pane: &mut super::PluginPane, client| {
        pane.render_frame(client, frame_params(), InputMode::Normal)
            .unwrap()
            .is_some()
    };
    assert!(render(&mut pane, client));
    assert!(!render(&mut pane, client));
    assert!(render(&mut pane, other));
    pane.set_borderless(true);
    pane.update_name("renamed");
    assert!(!render(&mut pane, client));
    pane.set_borderless(false);
    assert!(render(&mut pane, client));
    assert!(!render(&mut pane, client));
    pane.toggle_pinned();
    assert!(render(&mut pane, client));
    assert!(render(&mut pane, client));
}
