use super::tests::{frame_params, test_plugin_pane};
use crate::pane::Pane;
use yinz_types::data::InputMode;
use yinz_types::internal_ids::ClientId;
use yinz_types::pane_size::Offset;

#[test]
fn plugin_control_capability_reports_loading_errors_without_terminal_io() {
    let mut plugin = test_plugin_pane(5, 60, Offset::default());
    let pane: &mut dyn Pane = &mut plugin;
    let client = ClientId::new(1);
    pane.add_client(client);
    assert!(pane.terminal_io_mut().is_none());
    let mut loading = crate::ui::loading_indication::LoadingIndication::default();
    loading.indicate_loading_error("capability loading error".into());
    pane.plugin_control_mut()
        .unwrap()
        .update_loading_indication(loading);
    assert!(pane
        .dump_screen(false, Some(client))
        .contains("capability loading error"));
}

#[test]
fn removing_a_client_only_clears_its_own_frame_override() {
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    let mut pane = test_plugin_pane(5, 60, Offset::default());
    pane.add_client(other);
    for owner in [None, Some(other), Some(client)] {
        pane.add_client(client);
        pane.add_highlight_pane_frame_color_override(Some("highlight".into()), owner);
        let previous_override = pane.pane_frame_color_override.clone();
        pane.set_should_render(false);
        pane.remove_client(client);
        assert_eq!(
            pane.pane_frame_color_override,
            if owner == Some(client) {
                None
            } else {
                previous_override
            }
        );
        assert_eq!(pane.should_render(), owner == Some(client));
        assert_eq!(
            pane.should_render.get(&other),
            Some(&(owner == Some(client)))
        );
    }
}

#[test]
fn attaching_a_client_initializes_loading_without_restarting_completed_loads() {
    let mut pane = test_plugin_pane(5, 60, Offset::default());
    // No start timestamp: the loading text is visible without waiting for its grace period.
    pane.loading_indication = Default::default();
    let loading = ClientId::new(1);
    pane.add_client(loading);
    assert!(pane.dump_screen(false, Some(loading)).contains("Loading"));
    pane.loading_indication.ended = true;
    let loaded = ClientId::new(2);
    pane.add_client(loaded);
    assert!(pane.grids.contains_key(&loaded));
    assert!(pane.dump_screen(false, Some(loaded)).trim().is_empty());
}

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

#[test]
fn mixed_kitty_and_text_in_a_plugin_frame_preserve_placement_order() {
    use crate::panes::kitty_graphics::{test_support::apc_b64, KittyHostSupport};
    use yinz_types::pane_size::SizeInPixels;

    let client = ClientId::new(1);
    let mut pane = test_plugin_pane(5, 40, Offset::default());
    *pane.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 8,
        height: 21,
    });
    pane.update_kitty_host_support(KittyHostSupport::Supported);
    let mut frame = b"\x1b[31ma\x1b[m\x1b[2;3H\x1b]10;?\x07".to_vec();
    frame.extend(apc_b64("a=T,f=32,s=1,v=1,i=1,C=1", &[255; 4]));
    frame.extend_from_slice(b"\x1b[32mB\x1b[mC");
    pane.handle_plugin_bytes(client, frame);

    let grid = pane.grids.get_mut(&client).unwrap();
    let placement = &grid.kitty_placements()[0];
    assert_eq!(
        (placement.display_rect.x, placement.display_rect.y),
        (16, 21)
    );
    let screen = grid.dump_screen(false);
    let lines: Vec<_> = screen.lines().collect();
    assert_eq!(lines[0].trim_end(), "a");
    assert_eq!(lines[1].trim_end(), "  BC");
    assert!(grid.pending_messages_to_pty.is_empty());
    assert!(grid.pending_forwarded_queries.is_empty());
}

#[test]
fn snapshots_use_the_requested_client_and_disconnect_forgets_parser_state() {
    let mut pane = test_plugin_pane(5, 60, Offset::default());
    let low = ClientId::new(1);
    let high = ClientId::new(2);
    pane.handle_plugin_bytes(high, b"higher".to_vec());
    pane.handle_plugin_bytes(low, b"\x1b[31mlower".to_vec());
    assert!(pane.pane_contents(None, false, None).viewport.is_empty());
    for ansi in [false, true] {
        let contents = if ansi {
            pane.pane_contents_with_ansi(Some(low), false, None)
        } else {
            pane.pane_contents(Some(low), false, None)
        };
        assert!(contents.viewport.concat().contains("lower"));
    }
    pane.handle_plugin_bytes(low, b"\x1b[".to_vec());
    pane.remove_client(low);
    assert!(!pane.vte_parsers.contains_key(&low));
    assert!(!pane.kitty_interceptors.contains_key(&low));
    assert!(!pane.should_render.contains_key(&low));
    assert!(pane
        .pane_contents(Some(high), false, None)
        .viewport
        .concat()
        .contains("higher"));
    assert!(pane
        .pane_contents_with_ansi(Some(high), false, None)
        .viewport
        .concat()
        .contains("higher"));
    pane.handle_plugin_bytes(low, b"reused".to_vec());
    assert!(pane
        .pane_contents(Some(low), false, None)
        .viewport
        .concat()
        .contains("reused"));
    pane.remove_client(low);
    pane.remove_client(high);
    // Painting a frame must not recreate a disconnected client's empty grid.
    pane.render_frame(low, frame_params(), InputMode::Normal)
        .unwrap();
    assert!(pane.grids.is_empty());
    assert!(pane
        .pane_contents(Some(low), false, None)
        .viewport
        .is_empty());
    assert!(pane
        .pane_contents_with_ansi(Some(low), false, None)
        .viewport
        .is_empty());
}

#[test]
fn runs_and_reconstructed_prefixes_preserve_plugin_output() {
    let client = ClientId::new(1);
    let prefix = b"\x1b_Xignored\x1b\\";
    for split in 0..=2 {
        let mut pane = test_plugin_pane(10, 40, Offset::default());
        pane.handle_plugin_bytes(client, prefix[..split].to_vec());
        let mut frame = prefix[split..].to_vec();
        frame.extend_from_slice(b"head\x1b_Xignored\x1b\\tail\x1b_Gq=2,a=d,d=A\x1b\\end");
        pane.handle_plugin_bytes(client, frame);

        let grid = pane.grids.get(&client).expect("plugin grid");
        assert_eq!(grid.dump_screen(false).trim_end(), "headtailend");
        assert_eq!(grid.kitty_commands_handled(), 1);
        assert!(grid.pending_messages_to_pty.is_empty());
        assert!(grid.pending_forwarded_queries.is_empty());
    }
}

#[test]
fn terminal_palette_does_not_remap_plugin_output() {
    let client = ClientId::new(1);
    let mut pane = test_plugin_pane(5, 60, Offset::default());
    pane.style.terminal = Some(yinz_types::style::TerminalTheme {
        foreground: (1, 2, 3),
        background: (4, 5, 6),
        ansi: [(7, 8, 9); 16],
    });
    pane.handle_plugin_bytes(client, b"\x1b[31mplugin".to_vec());
    let (chunks, _, _, _) = pane
        .grids
        .get_mut(&client)
        .unwrap()
        .render(0, 0, &pane.style)
        .unwrap()
        .unwrap();
    assert!(chunks.iter().all(|chunk| chunk.pane_default_fg.is_none()
        && chunk.pane_default_bg.is_none()
        && chunk.changed_colors.is_none()));
}

#[test]
fn plugin_frames_keep_the_calculated_fade_color() {
    use crate::panes::terminal_character::AnsiCode;
    use yinz_types::data::PaletteColor;

    let mut pane = test_plugin_pane(5, 60, Offset::frame(1));
    pane.flash_pane_frame_color_override(std::time::Instant::now());
    for color in [Some(PaletteColor::Rgb((30, 40, 50))), None] {
        let mut params = frame_params();
        params.color = color;
        let (frame, _) = pane
            .render_frame(ClientId::new(1), params, InputMode::Normal)
            .unwrap()
            .unwrap();
        assert!(!frame.is_empty());
        assert!(frame
            .iter()
            .flat_map(|chunk| &chunk.terminal_characters)
            .all(|character| character.styles.foreground
                == Some(color.map(Into::into).unwrap_or(AnsiCode::Reset))));
    }
}
