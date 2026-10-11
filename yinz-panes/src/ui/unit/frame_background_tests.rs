use super::*;

#[test]
fn every_frame_shape_uses_ui_defaults_and_preserves_indicator_backgrounds() {
    for (background, foreground) in [
        ((245, 240, 230), (40, 45, 50)),
        ((20, 25, 30), (220, 225, 230)),
    ] {
        for shape in 0..4 {
            let mut frame = PaneFrame {
                geom: Viewport {
                    x: 0,
                    y: 0,
                    cols: 60,
                    rows: 6,
                },
                title: "pane".into(),
                should_draw_pane_frames: true,
                is_main_client: true,
                other_cursors_exist_in_session: true,
                other_focused_clients: vec![ClientId::new(2)],
                ..Default::default()
            };
            frame.style.colors.text_unselected.background = PaletteColor::Rgb(background);
            frame.style.colors.text_unselected.base = PaletteColor::Rgb(foreground);
            frame.style.colors.multiplayer_user_colors.player_2 = PaletteColor::Rgb((90, 130, 200));
            match shape {
                1 => frame.pane_is_floating = true,
                2 => {
                    frame.geom.rows = 1;
                    frame.should_draw_pane_frames = false;
                },
                3 => {
                    frame.stack_list_entry = Some(StackListEntry {
                        width: 20,
                        label: "stack entry".into(),
                        is_selected: true,
                        is_emphasized: false,
                        stack_is_focused: true,
                        border_color: None,
                    })
                },
                _ => {},
            }
            let (rendered, _) = frame.render().unwrap();
            assert!(!rendered.is_empty());
            for chunk in &rendered {
                assert_eq!(
                    chunk.pane_default_bg,
                    Some(PaletteColor::Rgb(background).into())
                );
                assert_eq!(
                    chunk.pane_default_fg,
                    Some(PaletteColor::Rgb(foreground).into())
                );
            }
            let client = ClientId::new(1);
            let mut output = crate::output::Output::default();
            output.add_clients(&[client].into_iter().collect(), Default::default(), None);
            output
                .add_character_chunks_to_client(client, rendered, None)
                .unwrap();
            let serialized = output.serialize().unwrap().remove(&client).unwrap();
            let (r, g, b) = background;
            assert!(serialized.contains(&format!("48;2;{r};{g};{b}")));
            let (r, g, b) = foreground;
            assert!(serialized.contains(&format!("38;2;{r};{g};{b}")));
            assert!(serialized.contains("48;2;90;130;200"));
        }
    }
}
