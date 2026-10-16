use super::*;

#[test]
fn every_frame_shape_uses_terminal_defaults_and_preserves_indicator_backgrounds() {
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
                other_focused_clients: vec![ColorSlot::new(2)],
                ..Default::default()
            };
            frame.style.terminal = Some(yinz_types::style::TerminalTheme {
                foreground,
                background,
                ansi: [(0, 0, 0); 16],
            });
            frame.style.colors.multiplayer_user_colors.player_2 = PaletteColor::Rgb((90, 130, 200));
            match shape {
                1 => frame.is_floating = true,
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

#[test]
fn unthemed_and_ansi_frames_preserve_host_defaults_and_explicit_outlines() {
    let themes = yinz_config::theme::Themes::from_string(
        include_str!("../../../../yinz-config/assets/themes/ansi.kdl"),
        false,
    )
    .unwrap();
    let ansi = themes.get_theme("ansi").unwrap();
    for style in [
        Style::default(),
        Style {
            colors: ansi.palette,
            terminal: ansi.terminal,
            ..Style::default()
        },
    ] {
        assert_eq!(style.chrome_colors(), None);
        for color in [None, Some(style.colors.frame_selected.base)] {
            let frame = PaneFrame {
                geom: Viewport {
                    x: 0,
                    y: 0,
                    cols: 20,
                    rows: 4,
                },
                should_draw_pane_frames: true,
                style,
                color,
                ..Default::default()
            };
            let (chunks, _) = frame.render().unwrap();
            assert!(!chunks.is_empty());
            for chunk in chunks {
                assert_eq!(chunk.pane_default_fg, None);
                assert_eq!(chunk.pane_default_bg, None);
                for character in chunk.terminal_characters {
                    assert_eq!(character.styles.background, Some(AnsiCode::Reset));
                    assert_eq!(
                        character.styles.foreground,
                        Some(color.map(Into::into).unwrap_or(AnsiCode::Reset))
                    );
                }
            }
        }
    }
}
