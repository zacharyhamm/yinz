//! Pixel regressions use Mesa's headless EGL device on Linux.
use super::*;
use crate::render::atlas::GlyphAtlas;
use crate::render::scene::test_helpers::Scene;
use crate::text::font::CellChange;
use glutin::prelude::*;
use std::num::NonZeroU32;

fn with_renderer(test: impl FnOnce(&mut Renderer)) {
    use glutin::api::egl::{device::Device, display::Display};
    let device = Device::query_devices().unwrap().next().expect("EGL device");
    let display = unsafe { Display::with_device(&device, None).unwrap() };
    let template = glutin::config::ConfigTemplateBuilder::new()
        .with_surface_type(glutin::config::ConfigSurfaceTypes::PBUFFER)
        .with_api(glutin::config::Api::GLES3)
        .build();
    let config = unsafe { display.find_configs(template).unwrap().next().unwrap() };
    let attributes = glutin::context::ContextAttributesBuilder::new()
        .with_context_api(glutin::context::ContextApi::Gles(Some(
            glutin::context::Version::new(3, 0),
        )))
        .build(None);
    let context = unsafe { display.create_context(&config, &attributes).unwrap() };
    let attributes =
        glutin::surface::SurfaceAttributesBuilder::<glutin::surface::PbufferSurface>::new()
            .build(NonZeroU32::new(768).unwrap(), NonZeroU32::new(384).unwrap());
    let surface = unsafe {
        display
            .create_pbuffer_surface(&config, &attributes)
            .unwrap()
    };
    let context = context.make_current(&surface).unwrap();
    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            display.get_proc_address(&std::ffi::CString::new(name).unwrap())
        })
    };
    let mut renderer = Renderer::new(gl).unwrap();
    test(&mut renderer);
    drop(renderer);
    drop(context);
}

fn scene(z: i32, color: [u8; 4]) -> Scene {
    Scene {
        width: 2,
        height: 1,
        clear: [0; 3],
        rects: vec![crate::render::scene::Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            color: [0, 255, 0],
        }],
        glyphs: vec![],
        color_glyphs: vec![],
        images: vec![ImageQuad {
            key: ImageKey::Kitty(1),
            revision: 1,
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            source_x: 0,
            source_y: 0,
            z,
            image: std::rc::Rc::new(crate::emulation::kitty::Image {
                width: 2,
                height: 1,
                pixels: color.repeat(2),
            }),
        }],
        resident_images: vec![ImageKey::Kitty(1)],
    }
}

fn pixels(renderer: &mut Renderer, scene: &Scene) -> Vec<u8> {
    let mask = GlyphAtlas::new(1);
    let color = GlyphAtlas::new(4);
    renderer.draw(
        scene,
        Atlases {
            mask: &mask,
            color: &color,
        },
        (2, 1),
    );
    let mut pixels = vec![0; 8];
    unsafe {
        renderer.gl.read_pixels(
            0,
            0,
            2,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    pixels
}

#[test]
fn kitty_depth_threshold_covers_only_deep_images_with_cell_backgrounds() {
    // https://sw.kovidgoyal.net/kitty/graphics-protocol/#controlling-displayed-image-layout
    // "Negative z-index values below INT32_MIN/2 (-1,073,741,824) will be drawn under cells
    // with non-default background colors."
    with_renderer(|renderer| {
        for z in [i32::MIN, -1073741825, -1073741824, -1073741823, -1, 0, 1] {
            let actual = pixels(renderer, &scene(z, [255, 0, 0, 255]));
            let covered = if z < -1073741824 {
                [0, 255, 0, 255]
            } else {
                [255, 0, 0, 255]
            };
            assert_eq!(&actual[..4], &covered, "z={z}");
            assert_eq!(&actual[4..], &[255, 0, 0, 255], "default background, z={z}");
        }
    });
}

#[test]
fn reconnect_uploads_new_pixels_for_reused_image_identity_and_revision() {
    with_renderer(|renderer| {
        let old = scene(0, [255, 0, 0, 255]);
        assert_eq!(&pixels(renderer, &old)[4..], &[255, 0, 0, 255]);
        renderer.clear_images();
        assert_eq!(renderer.image_textures(), 0);
        assert!(renderer.image_batches.is_empty());
        let new = scene(0, [0, 0, 255, 255]);
        assert_eq!(&pixels(renderer, &new)[4..], &[0, 0, 255, 255]);
    });
}

fn font_state() -> crate::emulation::terminal::TerminalState {
    use crate::testing::painter::Painter;
    use yinz_types::structured_render::{ATTR_BOLD, ATTR_ITALIC};
    Painter::state(5, 24, |painter| {
        for (row, attrs) in [0, ATTR_BOLD, ATTR_ITALIC, ATTR_BOLD | ATTR_ITALIC]
            .into_iter()
            .enumerate()
        {
            painter.styled(row, 0, "Ag -> fi", |cell| cell.attrs |= attrs);
            painter.wide_styled(row, 10, '☀', |cell| cell.attrs |= attrs);
            painter.cluster_styled(row, 14, "e\u{301}", 1, |cell| cell.attrs |= attrs);
        }
        painter.wide(4, 0, '中');
        painter.wide(4, 3, '😀');
    })
}

fn fixture_cache(options: &crate::text::font::FontOptions) -> crate::render::atlas::GlyphCache {
    use crate::text::{discovery::Discovery, font::FontStack};
    let fonts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
    let discovery = Discovery::synthetic(&fonts)
        .expect("required font GPU tests need fixture discovery/libfontconfig");
    crate::render::atlas::GlyphCache::new(FontStack::build_with(options, Some(discovery)).unwrap())
}

fn font_pixels(
    renderer: &mut Renderer,
    retained: &mut RetainedScene,
    state: &crate::emulation::terminal::TerminalState,
    cache: &mut crate::render::atlas::GlyphCache,
) -> Vec<u8> {
    use crate::emulation::color::Paints;
    use crate::render::scene::{BlinkPhase, CursorOptions, SceneInputs};
    retained.refresh(
        SceneInputs {
            state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        cache,
    );
    draw_and_read(renderer, retained, cache.atlases(), (768, 384))
}

fn draw_and_read(
    renderer: &mut Renderer,
    retained: &RetainedScene,
    atlases: Atlases<'_>,
    target: (u32, u32),
) -> Vec<u8> {
    renderer.draw_retained(retained, atlases, target, Frame::default());
    read_framebuffer(renderer)
}

fn read_framebuffer(renderer: &Renderer) -> Vec<u8> {
    let mut pixels = vec![0; 768 * 384 * 4];
    unsafe {
        renderer.gl.read_pixels(
            0,
            0,
            768,
            384,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    pixels
}

#[test]
fn native_switch_notice_draws_and_dismisses_over_source_content() {
    use crate::emulation::selection::Selection;
    use crate::render::scene::{notice_scene, Notice};
    use crate::testing::painter::{residency, Painter, Place};
    use crate::text::font::FontOptions;

    with_renderer(|renderer| {
        let original = [0, 255, 0, 255];
        let state = Painter::state(5, 48, |p| {
            p.text(0, 4, "source content");
            p.graphics(&[
                residency(1, 8, 8, original),
                Place::of(1, 8, 8).depth(1).record(),
            ]);
        });
        let selection = Selection::at(0, 4).extended_to(0, 9);
        let copied = selection.text(state.screen());
        let mut cache = fixture_cache(&FontOptions::default());
        let mut retained = RetainedScene::new();
        retained.set_selection(Some(selection));
        let baseline = font_pixels(renderer, &mut retained, &state, &mut cache);
        assert_eq!(baseline_offset_atlas_retry_pixel(&baseline, 1, 1), original);
        let target = (768, 384);
        let notice = notice_scene(
            Notice::Failure(
                "Could not switch to destination. Esc dismisses.\nFix /work/layout.swap.kdl and retry.",
            ),
            &mut cache,
            target,
            0,
        );
        assert!(!notice.glyphs.is_empty());
        renderer.draw_retained(
            &retained,
            cache.atlases(),
            target,
            Frame {
                notice: Some(&notice),
                ..Default::default()
            },
        );
        let shown = read_framebuffer(renderer);
        assert_eq!(
            baseline_offset_atlas_retry_pixel(&shown, 1, 1),
            [35, 29, 23, 255]
        );
        let panel = notice.rects[0];
        let text_ink = (2..panel.height - 2).any(|y| {
            (2..panel.width - 2).any(|x| {
                let [r, _, _, _] = baseline_offset_atlas_retry_pixel(&shown, x, y);
                r > 100
            })
        });
        assert!(text_ink, "the notice panel must contain visible text ink");
        assert_eq!(selection.text(state.screen()), copied);
        assert_eq!(
            draw_and_read(renderer, &retained, cache.atlases(), target),
            baseline
        );
    });
}

#[test]
fn native_switch_pending_notice_draws_and_clears_over_source_content() {
    use crate::emulation::selection::Selection;
    use crate::render::scene::{notice_scene, Notice};
    use crate::testing::painter::{residency, Painter, Place};
    use crate::text::font::FontOptions;

    with_renderer(|renderer| {
        let original = [0, 255, 0, 255];
        let state = Painter::state(5, 48, |p| {
            p.text(0, 4, "source content");
            p.graphics(&[
                residency(1, 8, 8, original),
                Place::of(1, 8, 8).depth(1).record(),
            ]);
        });
        let selection = Selection::at(0, 4).extended_to(0, 9);
        let copied = selection.text(state.screen());
        let mut cache = fixture_cache(&FontOptions::default());
        let mut retained = RetainedScene::new();
        retained.set_selection(Some(selection));
        let mut baseline = font_pixels(renderer, &mut retained, &state, &mut cache);
        let metrics = cache.metrics();
        let inset = metrics.stroke.max(2);
        let height = 384;
        for (width, expected) in [
            (
                768,
                "Esc cancels\nOpening: 中AB destination\nInput: source room",
            ),
            (
                12 * metrics.width + 2 * inset,
                "Esc cancels\nOpening: 中A\nInput: sourc",
            ),
            (8 * metrics.width + 2 * inset, "Esc\nOpening:\nInput: s"),
        ] {
            let target = (width, height);
            if width != 768 {
                baseline = draw_and_read(renderer, &retained, cache.atlases(), target);
            }
            assert_eq!(baseline_offset_atlas_retry_pixel(&baseline, 1, 1), original);
            let notice = notice_scene(
                Notice::Pending {
                    destination: "中AB\ndestination",
                    source: "source\u{2028}room",
                },
                &mut cache,
                target,
                0,
            );
            assert_eq!(
                notice,
                notice_scene(Notice::Failure(expected), &mut cache, target, 0)
            );
            renderer.draw_retained(
                &retained,
                cache.atlases(),
                target,
                Frame {
                    notice: Some(&notice),
                    ..Default::default()
                },
            );
            let shown = read_framebuffer(renderer);
            assert_eq!(
                baseline_offset_atlas_retry_pixel(&shown, 1, 1),
                [35, 29, 23, 255]
            );
            let panel = notice.rects[0];
            assert_eq!(panel.height, 3 * metrics.height + 2 * inset);
            assert!(panel.width <= width);
            for row in 0..3 {
                let top = inset + row * metrics.height;
                assert!(
                    (top..top + metrics.height).any(|y| {
                        (inset..panel.width - inset)
                            .any(|x| baseline_offset_atlas_retry_pixel(&shown, x, y)[0] > 100)
                    }),
                    "pending line {row} has no visible text at width {width}"
                );
            }
            assert_eq!(selection.text(state.screen()), copied);
            assert_eq!(
                draw_and_read(renderer, &retained, cache.atlases(), target),
                baseline
            );
        }
    });
}

#[test]
fn font_faces_draw_four_styles_weights_cjk_and_color_emoji() {
    use yinz_types::input::native_terminal::TerminalConfig;
    let settings = TerminalConfig {
        font_family: Some("Iosevka Term".into()),
        font_bold: Some("Iosevka Term".into()),
        font_italic: Some("Iosevka Term".into()),
        font_bold_italic: Some("Iosevka Term".into()),
        ..Default::default()
    };
    let options = crate::window::effective_font_options(&settings, 1.0, 1.0);
    let mut cache = fixture_cache(&options);
    let mut retained = RetainedScene::new();
    let state = font_state();
    with_renderer(|renderer| {
        let pixels = font_pixels(renderer, &mut retained, &state, &mut cache);
        let metrics = cache.metrics();
        let row_pixels = |row: usize| -> Vec<u8> {
            // GL readback is bottom-up. Compare the same glyphs in each style.
            let mut ink = Vec::new();
            for y in row * metrics.height as usize..(row + 1) * metrics.height as usize {
                let offset = ((383 - y) * 768) * 4;
                ink.extend_from_slice(&pixels[offset..offset + metrics.width as usize * 8 * 4]);
            }
            assert!(
                ink.as_chunks::<4>().0.iter().any(|p| p[0] != 0),
                "row {row} has no glyph pixels"
            );
            ink
        };
        let mut cjk_ink = false;
        for y in 4 * metrics.height as usize..5 * metrics.height as usize {
            let offset = (383 - y) * 768 * 4;
            cjk_ink |= pixels[offset..offset + metrics.width as usize * 2 * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] != 0);
        }
        assert!(cjk_ink, "CJK fallback has no GPU pixels");
        let rows: Vec<_> = (0..4).map(row_pixels).collect();
        for a in 0..4 {
            for b in a + 1..4 {
                assert_ne!(rows[a], rows[b], "styles {a}/{b}");
            }
        }
        assert!(
            row_pixels(4)
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] != p[1] || p[1] != p[2]),
            "emoji lost its color"
        );
    });
}

#[test]
fn font_faces_live_changes_match_fresh_gpu_draws_and_preserve_selection() {
    use crate::emulation::selection::Selection;
    use yinz_types::input::native_terminal::TerminalConfig;
    let initial = TerminalConfig {
        font_family: Some("Iosevka Term".into()),
        ..Default::default()
    };
    let mut variants = Vec::new();
    for style in 0..4 {
        let mut settings = initial.clone();
        let family = Some(
            if style == 0 {
                "Noto Sans Mono CJK SC"
            } else {
                "Noto Color Emoji"
            }
            .into(),
        );
        match style {
            0 => settings.font_family = family,
            1 => settings.font_bold = family,
            2 => settings.font_italic = family,
            _ => settings.font_bold_italic = family,
        }
        variants.push(crate::window::effective_font_options(&settings, 1.0, 1.0));
    }
    variants.push(crate::window::effective_font_options(
        &TerminalConfig {
            font_weight: Some(700),
            ..initial.clone()
        },
        1.0,
        1.0,
    ));
    variants.push(crate::window::effective_font_options(&initial, 1.5, 1.0));
    variants.push(crate::window::effective_font_options(&initial, 1.0, 2.0));
    let state = font_state();
    let selection = Selection::at(0, 0).extended_to(0, 1);
    assert_eq!(selection.text(state.screen()), "Ag");
    let mut outputs = Vec::new();
    with_renderer(|renderer| {
        let mut active = crate::window::effective_font_options(&initial, 1.0, 1.0);
        let mut cache = fixture_cache(&active);
        let mut retained = RetainedScene::new();
        retained.set_selection(Some(selection));
        let initial_pixels = font_pixels(renderer, &mut retained, &state, &mut cache);
        // A second draw consumes the warmed shaping/glyph/atlas/retained state.
        assert_eq!(
            font_pixels(renderer, &mut retained, &state, &mut cache),
            initial_pixels
        );
        for requested in &variants {
            let generation = cache.generation();
            let old_atlas = cache.atlases().mask.stamp();
            let replacement = fixture_cache(requested);
            crate::window::apply_fonts(
                &mut active,
                &mut cache,
                &mut retained,
                requested.clone(),
                crate::window::FontUpdate::Replacement(Box::new(replacement)),
            );
            assert_ne!(cache.generation(), generation);
            assert_ne!(cache.atlases().mask.stamp(), old_atlas);
            let pixels = font_pixels(renderer, &mut retained, &state, &mut cache);
            assert!(
                pixels != initial_pixels,
                "change did not affect GPU ink: {requested:?}"
            );
            assert_eq!(selection.text(state.screen()), "Ag");
            outputs.push(pixels);
        }
    });
    for (options, expected) in variants.iter().zip(outputs) {
        with_renderer(|renderer| {
            let mut cache = fixture_cache(options);
            let mut retained = RetainedScene::new();
            retained.set_selection(Some(selection));
            assert!(
                font_pixels(renderer, &mut retained, &state, &mut cache) == expected,
                "live font replacement left stale GPU pixels for {options:?}"
            );
        });
    }
}

#[test]
fn font_faces_resize_repaint_reanchors_copy_and_gpu_highlight() {
    resize_repaint_reanchors_copy_and_gpu_highlight(false);
}

#[test]
fn cell_metrics_resize_repaint_reanchors_copy_and_gpu_highlight() {
    resize_repaint_reanchors_copy_and_gpu_highlight(true);
}

fn resize_repaint_reanchors_copy_and_gpu_highlight(cell_metrics: bool) {
    use crate::emulation::selection::{LocalSelection, Selection};
    use crate::testing::painter::Painter;
    use yinz_types::input::native_terminal::TerminalConfig;
    let settings = TerminalConfig::default();
    let initial = crate::window::effective_font_options(&settings, 1.0, 1.0);
    let requested = if cell_metrics {
        crate::window::effective_font_options(
            &TerminalConfig {
                cell_width: Some(4.0 / 3.0),
                line_height: Some(1.25),
                ..settings
            },
            1.0,
            1.0,
        )
    } else {
        crate::window::effective_font_options(&settings, 1.2, 1.0)
    };
    let text = format!("{}distinctive ABCD suffix", "x".repeat(58));
    let old_frame = Painter::frame(5, 80, |p| {
        p.full_repaint();
        p.text(0, 0, &text[..80]);
        p.text(1, 0, &text[80..]);
        p.wide(3, 0, '中');
        p.wide(4, 0, '😀');
    });
    let resized_frame = Painter::frame(5, 60, |p| {
        p.full_repaint();
        p.text(0, 0, &text[..60]);
        p.text(1, 0, &text[60..]);
        p.wide(3, 0, '中');
        p.wide(4, 0, '😀');
    });
    let mut state = crate::emulation::terminal::TerminalState::new(5, 80);
    let mut selection = LocalSelection::default();
    selection.apply_frame(&mut state, &old_frame).unwrap();
    selection.start(0, 70);
    selection.extend(0, 73);
    let expected_range = Selection::at(1, 10).extended_to(1, 13);
    with_renderer(|renderer| {
        let mut active = initial.clone();
        let mut cache = fixture_cache(&active);
        let mut retained = RetainedScene::new();
        retained.set_selection(selection.range());
        let warmed = font_pixels(renderer, &mut retained, &state, &mut cache);
        assert_eq!(
            font_pixels(renderer, &mut retained, &state, &mut cache),
            warmed
        );
        let update = if cell_metrics {
            crate::window::prepare_fonts(&active, &requested).unwrap()
        } else {
            crate::window::FontUpdate::Replacement(Box::new(fixture_cache(&requested)))
        };
        crate::window::apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            update,
        );
        selection.resizing(&state, 5, 60);
        let applied = selection.apply_frame(&mut state, &old_frame).unwrap();
        retained.mark(&applied.damage);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
        let applied = selection.apply_frame(&mut state, &resized_frame).unwrap();
        retained.mark(&applied.damage);
        assert_eq!(selection.range(), Some(expected_range));
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
        retained.set_selection(selection.range());
        let live = font_pixels(renderer, &mut retained, &state, &mut cache);
        let mut fresh_cache = fixture_cache(&requested);
        let mut fresh = RetainedScene::new();
        fresh.set_selection(Some(expected_range));
        assert_eq!(
            font_pixels(renderer, &mut fresh, &state, &mut fresh_cache),
            live
        );
        fresh.set_selection(None);
        let unselected = font_pixels(renderer, &mut fresh, &state, &mut fresh_cache);
        let metrics = cache.metrics();
        let mut highlighted_pixels = 0;
        for y in 0..384 {
            for x in 0..768 {
                let offset = ((383 - y) * 768 + x) * 4;
                if live[offset..offset + 4] != unselected[offset..offset + 4] {
                    // Only the independently asserted relocated range may change.
                    // In particular the old row-0/right-edge highlight is gone.
                    assert!((metrics.height as usize..2 * metrics.height as usize).contains(&y));
                    assert!((10 * metrics.width as usize..14 * metrics.width as usize).contains(&x));
                    highlighted_pixels += 1;
                }
            }
        }
        assert!(
            highlighted_pixels > 0,
            "relocated highlight has no GPU pixels"
        );
        let generation = cache.generation();
        let atlas = cache.atlases().mask.stamp();
        let overwrite = Painter::frame(5, 60, |p| p.text(1, 10, "WXYZ"));
        let applied = selection.apply_frame(&mut state, &overwrite).unwrap();
        retained.mark(&applied.damage);
        assert_eq!(expected_range.text(state.screen()), "WXYZ");
        assert_eq!(selection.range(), None);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
        retained.set_selection(selection.range());
        let updated = font_pixels(renderer, &mut retained, &state, &mut cache);
        let mut unselected_cache = fixture_cache(&requested);
        let mut unselected_scene = RetainedScene::new();
        assert_eq!(
            updated,
            font_pixels(
                renderer,
                &mut unselected_scene,
                &state,
                &mut unselected_cache
            )
        );
        assert_eq!(cache.generation(), generation);
        // New letters append glyphs (and advance revision) in the same atlas.
        assert_eq!(cache.atlases().mask.stamp().identity, atlas.identity);
    });
}

#[test]
fn font_faces_new_selection_during_resize_reanchors_copy_and_gpu_highlight() {
    use crate::emulation::selection::{LocalSelection, Selection};
    use crate::testing::painter::Painter;
    use yinz_types::input::native_terminal::TerminalConfig;
    let initial = crate::window::effective_font_options(&TerminalConfig::default(), 1.0, 1.0);
    let requested = crate::window::effective_font_options(&TerminalConfig::default(), 1.2, 1.0);
    let text = format!(
        "{}ABCD{}WXYZ{}",
        "x".repeat(70),
        "y".repeat(16),
        "z".repeat(46)
    );
    let frame = |cols| {
        Painter::frame(5, cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
            p.wide(3, 0, '中');
            p.wide(4, 0, '😀');
        })
    };
    let old_frame = frame(80);
    let resized_frame = frame(60);
    let mut state = crate::emulation::terminal::TerminalState::new(5, 80);
    let mut selection = LocalSelection::default();
    selection.apply_frame(&mut state, &old_frame).unwrap();
    selection.start(0, 70);
    selection.extend(0, 73);
    let old_range = Selection::at(1, 10).extended_to(1, 13);
    let expected = Selection::at(1, 30).extended_to(1, 33);
    with_renderer(|renderer| {
        let mut active = initial.clone();
        let mut cache = fixture_cache(&active);
        let mut retained = RetainedScene::new();
        retained.set_selection(selection.range());
        let warmed = font_pixels(renderer, &mut retained, &state, &mut cache);
        assert_eq!(
            font_pixels(renderer, &mut retained, &state, &mut cache),
            warmed
        );
        let replacement = fixture_cache(&requested);
        crate::window::apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            crate::window::FontUpdate::Replacement(Box::new(replacement)),
        );
        selection.resizing(&state, 5, 60);
        // This press cancels ABCD while the old grid is still displayed.
        selection.start(1, 10);
        selection.extend(1, 13);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        let applied = selection.apply_frame(&mut state, &old_frame).unwrap();
        retained.mark(&applied.damage);
        retained.set_selection(selection.range());
        font_pixels(renderer, &mut retained, &state, &mut cache);
        assert_eq!(selection.range(), Some(old_range));
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        let applied = selection.apply_frame(&mut state, &resized_frame).unwrap();
        retained.mark(&applied.damage);
        assert_eq!(old_range.text(state.screen()), "ABCD");
        assert_eq!(expected.text(state.screen()), "WXYZ");
        assert_eq!(selection.range(), Some(expected));
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        retained.set_selection(selection.range());
        let live = font_pixels(renderer, &mut retained, &state, &mut cache);
        let mut fresh_cache = fixture_cache(&requested);
        let mut fresh = RetainedScene::new();
        fresh.set_selection(Some(expected));
        assert_eq!(
            live,
            font_pixels(renderer, &mut fresh, &state, &mut fresh_cache)
        );
        fresh.set_selection(None);
        let unselected = font_pixels(renderer, &mut fresh, &state, &mut fresh_cache);
        let metrics = cache.metrics();
        assert!(34 * metrics.width <= 768 && 2 * metrics.height <= 384);
        let mut highlighted = 0;
        for y in 0..384 {
            for x in 0..768 {
                let offset = ((383 - y) * 768 + x) * 4;
                if live[offset..offset + 4] != unselected[offset..offset + 4] {
                    assert!((metrics.height as usize..2 * metrics.height as usize).contains(&y));
                    assert!((30 * metrics.width as usize..34 * metrics.width as usize).contains(&x));
                    highlighted += 1;
                }
            }
        }
        assert!(
            highlighted > 0,
            "new selection must have visible relocated pixels"
        );
        let generation = cache.generation();
        let atlas = cache.atlases().mask.stamp();
        let overwrite = Painter::frame(5, 60, |p| p.text(1, 30, "gone"));
        let applied = selection.apply_frame(&mut state, &overwrite).unwrap();
        retained.mark(&applied.damage);
        assert_eq!(expected.text(state.screen()), "gone");
        assert_eq!(selection.range(), None);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        retained.set_selection(selection.range());
        let updated = font_pixels(renderer, &mut retained, &state, &mut cache);
        let mut unselected_cache = fixture_cache(&requested);
        let mut unselected_scene = RetainedScene::new();
        assert_eq!(
            updated,
            font_pixels(
                renderer,
                &mut unselected_scene,
                &state,
                &mut unselected_cache
            )
        );
        assert_eq!(cache.generation(), generation);
        assert_eq!(cache.atlases().mask.stamp().identity, atlas.identity);
    });
}

#[test]
fn ligatures_live_toggles_match_fresh_pixels_and_preserve_copy_cursor_and_cells() {
    use crate::emulation::screen_buffer::CursorShape;
    use crate::emulation::selection::LocalSelection;
    use crate::render::atlas::GlyphCache;
    use crate::testing::painter::Painter;
    use crate::window::{apply_fonts, effective_font_options, prepare_fonts, FontUpdate};
    use yinz_types::input::native_terminal::TerminalConfig;
    // Two cursor positions exercise interruption and reformation of the run.
    for cursor_col in [4, 10] {
        let mut state = font_state();
        Painter::apply(&mut state, |p| p.cursor(0, cursor_col, CursorShape::Beam));
        let mut selection = LocalSelection::default();
        selection.start(0, 3);
        selection.extend(0, 4);
        let range = selection.range();
        let cursor = state.cursor_position();
        let cells: Vec<_> = (0..5)
            .flat_map(|row| (0..24).map(move |col| (row, col)))
            .map(|(row, col)| state.screen().cell(row, col))
            .collect();
        let mut options = effective_font_options(
            &TerminalConfig {
                system_fonts: Some(false),
                ..Default::default()
            },
            1.0,
            1.0,
        );
        let mut cache = GlyphCache::new(options.stack(1.0).unwrap());
        let mut retained = RetainedScene::new();
        retained.set_selection(range);
        with_renderer(|renderer| {
            let joined = font_pixels(renderer, &mut retained, &state, &mut cache);
            assert_eq!(
                font_pixels(renderer, &mut retained, &state, &mut cache),
                joined
            );
            let generation = cache.generation();
            for (spelling, enabled) in [
                ("ligatures", false),
                ("font_ligatures", false),
                ("font_ligatures", true),
                ("ligatures", true),
                ("ligatures", false),
                ("font_ligatures", true),
            ] {
                let config = yinz_config::config::Config::from_kdl(
                    &format!("terminal {{ system_fonts false; {spelling} {enabled}; }}"),
                    None,
                )
                .unwrap();
                let requested = effective_font_options(&config.terminal, 1.0, 1.0);
                let changed = options.ligatures != enabled;
                let update = prepare_fonts(&options, &requested).unwrap();
                assert_eq!(matches!(update, FontUpdate::ShapingOnly), changed);
                let atlases = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
                let entries = cache.entries();
                let clusters = cache.cluster_entries();
                apply_fonts(
                    &mut options,
                    &mut cache,
                    &mut retained,
                    requested.clone(),
                    update,
                );
                assert_eq!(cache.generation(), generation);
                assert_eq!(
                    (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
                    atlases
                );
                assert_eq!(cache.entries(), entries);
                assert_eq!(cache.cluster_entries(), clusters);
                if changed {
                    assert_eq!(cache.shaped_runs(), 0);
                }
                let live = font_pixels(renderer, &mut retained, &state, &mut cache);
                if changed {
                    assert!(retained.rebuilt_everything());
                }
                if enabled {
                    assert_eq!(live, joined);
                } else {
                    assert_ne!(live, joined, "toggle did not change GPU ink");
                }
                let mut fresh_cache = GlyphCache::new(requested.stack(1.0).unwrap());
                let mut fresh = RetainedScene::new();
                fresh.set_selection(range);
                assert_eq!(
                    font_pixels(renderer, &mut fresh, &state, &mut fresh_cache),
                    live,
                    "stale live ink for {spelling} {enabled}, cursor {cursor_col}"
                );
                assert_eq!(selection.range(), range);
                assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
                assert_eq!(state.cursor_position(), cursor);
                for (index, cell) in cells.iter().enumerate() {
                    assert_eq!(state.screen().cell(index / 24, index % 24), *cell);
                }
            }
        });
    }
}

#[test]
fn font_features_live_reloads_match_fresh_pixels_and_preserve_copy_cursor_and_cells() {
    use crate::emulation::screen_buffer::CursorShape;
    use crate::emulation::selection::LocalSelection;
    use crate::render::atlas::GlyphCache;
    use crate::testing::painter::Painter;
    use crate::window::{apply_fonts, effective_font_options, prepare_fonts, FontUpdate};
    use yinz_types::input::native_terminal::TerminalConfig;
    // Two cursor positions exercise interruption and reformation of the run.
    for cursor_col in [4, 10] {
        let mut state = font_state();
        Painter::apply(&mut state, |p| p.cursor(0, cursor_col, CursorShape::Beam));
        let mut selection = LocalSelection::default();
        selection.start(0, 3);
        selection.extend(0, 4);
        let range = selection.range();
        let cursor = state.cursor_position();
        let cells: Vec<_> = (0..5)
            .flat_map(|row| (0..24).map(move |col| (row, col)))
            .map(|(row, col)| state.screen().cell(row, col))
            .collect();
        let mut options = effective_font_options(
            &TerminalConfig {
                system_fonts: Some(false),
                ..Default::default()
            },
            1.0,
            1.0,
        );
        let mut cache = GlyphCache::new(options.stack(1.0).unwrap());
        let mut retained = RetainedScene::new();
        retained.set_selection(range);
        with_renderer(|renderer| {
            let joined = font_pixels(renderer, &mut retained, &state, &mut cache);
            let generation = cache.generation();
            let mut previous: Option<Vec<u8>> = None;
            // calt drives Iosevka's arrow join: each step toggles the visible run.
            for (settings, is_joined) in [
                ("font_features \"-calt\";", false),
                ("font_features \"zero\";", true),
                ("ligatures false; font_features \"zero\";", false),
                ("ligatures false; font_features \"+calt\";", true),
                ("font_features \"zero\" \"-calt\";", false),
                ("font_features;", true),
            ] {
                let config = yinz_config::config::Config::from_kdl(
                    &format!("terminal {{ system_fonts false; {settings} }}"),
                    None,
                )
                .unwrap();
                let requested = effective_font_options(&config.terminal, 1.0, 1.0);
                let update = prepare_fonts(&options, &requested).unwrap();
                assert!(matches!(update, FontUpdate::ShapingOnly), "{settings}");
                let atlases = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
                let entries = cache.entries();
                let clusters = cache.cluster_entries();
                apply_fonts(
                    &mut options,
                    &mut cache,
                    &mut retained,
                    requested.clone(),
                    update,
                );
                assert_eq!(cache.generation(), generation);
                assert_eq!(
                    (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
                    atlases
                );
                assert_eq!(cache.entries(), entries);
                assert_eq!(cache.cluster_entries(), clusters);
                assert_eq!(cache.shaped_runs(), 0);
                let live = font_pixels(renderer, &mut retained, &state, &mut cache);
                assert!(retained.rebuilt_everything());
                if is_joined {
                    assert_eq!(live, joined, "{settings}");
                } else {
                    assert_ne!(live, joined, "{settings} did not change GPU ink");
                }
                if let Some(previous) = &previous {
                    assert_ne!(&live, previous, "{settings} did not change GPU ink");
                }
                let mut fresh_cache = GlyphCache::new(requested.stack(1.0).unwrap());
                let mut fresh = RetainedScene::new();
                fresh.set_selection(range);
                assert_eq!(
                    font_pixels(renderer, &mut fresh, &state, &mut fresh_cache),
                    live,
                    "stale live ink for {settings}, cursor {cursor_col}"
                );
                assert_eq!(selection.range(), range);
                assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
                assert_eq!(state.cursor_position(), cursor);
                for (index, cell) in cells.iter().enumerate() {
                    assert_eq!(state.screen().cell(index / 24, index % 24), *cell);
                }
                previous = Some(live);
            }
        });
    }
}

#[test]
fn baseline_offset_and_cell_metrics_warmed_spacing_zoom_scale_and_reset_match_fresh_gpu_output() {
    use crate::emulation::selection::Selection;
    use crate::text::font::FaceStyle;
    use yinz_types::input::native_terminal::TerminalConfig;
    let settings = TerminalConfig {
        system_fonts: Some(false),
        ..Default::default()
    };
    let mut active = crate::window::effective_font_options(&settings, 1.0, 1.0);
    let mut cache = crate::render::atlas::GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let mut state = font_state();
    crate::testing::painter::Painter::apply(&mut state, |p| {
        p.cluster(4, 6, "👩‍💻", 2);
        p.text(4, 10, "─┌┐");
    });
    let selection = Selection::at(4, 0).extended_to(4, 7);
    let copied = selection.text(state.screen());
    retained.set_selection(Some(selection));
    with_renderer(|renderer| {
        let original_metrics = cache.metrics();
        let original = font_pixels(renderer, &mut retained, &state, &mut cache);
        assert_eq!(
            font_pixels(renderer, &mut retained, &state, &mut cache),
            original
        );
        for (width, height, zoom, scale, ligatures, offset) in [
            (1.0, 1.0, 1.0, 1.0, true, 2),
            (1.0, 1.0, 1.0, 1.0, true, -2),
            (1.0, 1.0, 1.0, 1.0, true, 64),
            (1.0, 1.0, 1.0, 1.0, true, 63),
            (1.0, 1.0, 1.0, 1.0, true, -64),
            (1.0, 1.0, 1.0, 1.0, true, 0),
            (1.5, 1.0, 1.0, 1.0, true, 2),
            (1.0, 1.25, 1.0, 1.0, true, -2),
            (0.5, 0.5, 1.0, 1.0, false, 2),
            (3.0, 3.0, 1.0, 1.0, true, -2),
            (1.375, 1.275, 1.15, 1.25, true, 2),
            (1.375, 1.275, 1.15, 1.5, true, -2),
            (1.375, 1.275, 1.15, 2.0, true, 2),
            (1.0, 1.0, 1.0, 1.0, true, 0),
            (1.001, 1.001, 1.0, 1.0, false, 0),
        ] {
            let settings = TerminalConfig {
                cell_width: Some(width),
                line_height: Some(height),
                baseline_offset: Some(offset),
                font_features: Some(vec![if offset == 63 { "-calt" } else { "+calt" }.into()]),
                font_ligatures: Some(ligatures),
                ..settings.clone()
            };
            let requested = crate::window::effective_font_options(&settings, zoom, scale);
            let face = cache.primary_of(FaceStyle::Regular);
            let update = crate::window::prepare_fonts(&active, &requested).unwrap();
            crate::window::apply_fonts(
                &mut active,
                &mut cache,
                &mut retained,
                requested.clone(),
                update,
            );
            assert_eq!(cache.primary_of(FaceStyle::Regular), face);
            let live = font_pixels(renderer, &mut retained, &state, &mut cache);
            let metrics = cache.metrics();
            let scene = retained.flatten();
            assert_eq!(
                (scene.width, scene.height),
                (24 * metrics.width, 5 * metrics.height)
            );
            // Highlight rectangles retain logical ownership in the new physical cells.
            for col in 0..8 {
                assert!(scene
                    .rects
                    .iter()
                    .any(|r| r.x == (col * metrics.width) as i32
                        && r.y == (4 * metrics.height) as i32
                        && r.width == metrics.width
                        && r.height == metrics.height));
            }
            assert_eq!(selection.text(state.screen()), copied);
            let mut fresh = crate::render::atlas::GlyphCache::new(requested.stack(1.0).unwrap());
            let mut fresh_retained = RetainedScene::new();
            fresh_retained.set_selection(Some(selection));
            assert_eq!(
                font_pixels(renderer, &mut fresh_retained, &state, &mut fresh),
                live
            );
            // +calt explicitly restores the original ligature behavior, even
            // when ligatures=false. Rounded-equivalent spacing is a no-op too.
            if metrics == original_metrics {
                assert!(
                    live == original,
                    "reset/equivalent pixels: {width} {height} {offset}"
                );
            } else {
                assert!(
                    live != original,
                    "changed pixels: {width} {height} {offset}"
                );
            }
        }
    });
}

#[test]
fn cell_metrics_complete_clusters_keep_distinct_gpu_ink_across_updates() {
    use crate::emulation::selection::Selection;
    use crate::testing::painter::Painter;
    use yinz_types::input::native_terminal::TerminalConfig;
    let settings = TerminalConfig {
        system_fonts: Some(false),
        font_size: Some(crate::text::font::test_helpers::pixels_to_points(16.0)),
        ..Default::default()
    };
    with_renderer(|renderer| {
        for (text, head, budget) in [("e\u{301}", "e", 1), ("👩‍💻", "👩", 2), ("👍🏽", "👍", 2)]
        {
            let state = Painter::state(3, 8, |p| p.cluster(1, 2, text, budget));
            let plain = Painter::state(3, 8, |p| p.cluster(1, 2, head, budget));
            let blank = Painter::state(3, 8, |_| {});
            let selection = Selection::at(1, 2).extended_to(1, 2 + budget);
            assert_eq!(selection.text(state.screen()), text);
            let mut active = crate::window::effective_font_options(&settings, 1.0, 1.0);
            let mut cache = crate::render::atlas::GlyphCache::new(active.stack(1.0).unwrap());
            let mut retained = RetainedScene::new();
            // Keep selection/cursor/background identical in all three fixtures.
            retained.set_selection(Some(selection));
            for (width, height, ligatures, zoom, scale) in [
                (1.0, 1.0, true, 1.0, 1.0),
                (0.75, 1.0, false, 1.0, 1.0),
                (0.5, 1.0, true, 1.0, 1.0),
                (0.5, 1.25, false, 1.0, 1.0),
                (1.5, 1.25, true, 1.0, 1.0),
                (0.75, 1.25, false, 1.15, 1.25),
                (1.0, 1.0, true, 1.0, 1.0),
            ] {
                let configured = TerminalConfig {
                    cell_width: Some(width),
                    line_height: Some(height),
                    font_ligatures: Some(ligatures),
                    ..settings.clone()
                };
                let requested = crate::window::effective_font_options(&configured, zoom, scale);
                let update = crate::window::prepare_fonts(&active, &requested).unwrap();
                crate::window::apply_fonts(
                    &mut active,
                    &mut cache,
                    &mut retained,
                    requested.clone(),
                    update,
                );
                let full = font_pixels(renderer, &mut retained, &state, &mut cache);
                let m = cache.metrics();
                let crop = |pixels: &[u8]| {
                    let mut ink = Vec::new();
                    for y in m.height..2 * m.height {
                        let start = (((383 - y) * 768 + 2 * m.width) * 4) as usize;
                        ink.extend_from_slice(
                            &pixels[start..start + (budget as u32 * m.width * 4) as usize],
                        );
                    }
                    ink
                };
                let mut other = RetainedScene::new();
                other.set_selection(Some(selection));
                let plain_pixels = font_pixels(renderer, &mut other, &plain, &mut cache);
                let mut empty = RetainedScene::new();
                empty.set_selection(Some(selection));
                let blank_pixels = font_pixels(renderer, &mut empty, &blank, &mut cache);
                assert_ne!(crop(&full), crop(&blank_pixels), "blank {text} at {width}");
                assert_ne!(
                    crop(&full),
                    crop(&plain_pixels),
                    "lost components: {text}, width={width}, height={height}"
                );
                let mut fresh =
                    crate::render::atlas::GlyphCache::new(requested.stack(1.0).unwrap());
                let mut fresh_scene = RetainedScene::new();
                fresh_scene.set_selection(Some(selection));
                assert_eq!(
                    full,
                    font_pixels(renderer, &mut fresh_scene, &state, &mut fresh)
                );
                assert_eq!(selection.text(state.screen()), text);
            }
        }
    });
}

#[test]
fn baseline_offset_latin_ink_moves_by_expected_physical_pixels() {
    use crate::testing::painter::Painter;
    use yinz_types::input::native_terminal::TerminalConfig;
    with_renderer(|renderer| {
        for (zoom, scale) in [(1.0, 1.0), (1.15, 1.25), (1.0, 2.0)] {
            let settings = TerminalConfig {
                system_fonts: Some(false),
                ..Default::default()
            };
            let state = Painter::state(3, 8, |p| p.text(1, 2, "A"));
            let blank = Painter::state(3, 8, |_| {});
            let mut active = crate::window::effective_font_options(&settings, zoom, scale);
            let mut cache = crate::render::atlas::GlyphCache::new(active.stack(1.0).unwrap());
            let mut retained = RetainedScene::new();
            let zero = cache.metrics();
            let original = font_pixels(renderer, &mut retained, &state, &mut cache);
            let mut empty = RetainedScene::new();
            let background = font_pixels(renderer, &mut empty, &blank, &mut cache);
            // GL readback is bottom-up; return top-down ink coordinates independent
            // of the baseline implementation. The blank has the identical cursor.
            let ink = |pixels: &[u8]| {
                let mut positions = Vec::new();
                for y in zero.height..2 * zero.height {
                    for x in 2 * zero.width..3 * zero.width {
                        let at = (((383 - y) * 768 + x) * 4) as usize;
                        if pixels[at..at + 4] != background[at..at + 4] {
                            positions.push((x, y));
                        }
                    }
                }
                positions
            };
            let original_ink = ink(&original);
            assert!(!original_ink.is_empty());
            for offset in [1, -1, 0] {
                let requested = crate::window::effective_font_options(
                    &TerminalConfig {
                        baseline_offset: Some(offset),
                        ..settings.clone()
                    },
                    zoom,
                    scale,
                );
                let delta = (offset as f32 * zoom * scale as f32).round() as i32;
                assert_eq!(
                    requested.stack(1.0).unwrap().metrics().baseline as i32,
                    zero.baseline as i32 + delta
                );
                let update = crate::window::prepare_fonts(&active, &requested).unwrap();
                crate::window::apply_fonts(
                    &mut active,
                    &mut cache,
                    &mut retained,
                    requested,
                    update,
                );
                let moved = font_pixels(renderer, &mut retained, &state, &mut cache);
                let expected: Vec<_> = original_ink
                    .iter()
                    .map(|&(x, y)| (x, (y as i32 + delta) as u32))
                    .collect();
                assert_eq!(
                    ink(&moved),
                    expected,
                    "offset={offset}, zoom={zoom}, scale={scale}"
                );
                if offset == 0 {
                    assert_eq!(moved, original);
                }
            }
        }
    });
}

#[test]
fn baseline_offset_distinct_style_families_live_gpu_output_matches_fresh() {
    use crate::emulation::selection::Selection;
    use yinz_types::input::native_terminal::TerminalConfig;
    with_renderer(|renderer| {
        for (zoom, scale) in [(1.0, 1.0), (1.15, 1.25), (1.0, 2.0)] {
            let settings = TerminalConfig {
                font_family: Some("Iosevka Term".into()),
                font_bold: Some("Noto Sans Mono CJK SC".into()),
                font_italic: Some("Noto Sans Mono CJK SC".into()),
                font_bold_italic: Some("Noto Sans Mono CJK SC".into()),
                system_fonts: Some(true),
                ..Default::default()
            };
            let mut active = crate::window::effective_font_options(&settings, zoom, scale);
            let mut cache = fixture_cache(&active);
            let state = font_state();
            let selection = Selection::at(4, 0).extended_to(4, 4);
            let copied = selection.text(state.screen());
            let mut retained = RetainedScene::new();
            retained.set_selection(Some(selection));
            let original = font_pixels(renderer, &mut retained, &state, &mut cache);
            for offset in [2, -2, 64, -64, 0] {
                let requested = crate::window::effective_font_options(
                    &TerminalConfig {
                        baseline_offset: Some(offset),
                        ..settings.clone()
                    },
                    zoom,
                    scale,
                );
                let update = crate::window::prepare_fonts(&active, &requested).unwrap();
                assert!(matches!(update, crate::window::FontUpdate::CellOnly));
                crate::window::apply_fonts(
                    &mut active,
                    &mut cache,
                    &mut retained,
                    requested.clone(),
                    update,
                );
                let live = font_pixels(renderer, &mut retained, &state, &mut cache);
                let mut fresh = fixture_cache(&requested);
                let mut fresh_scene = RetainedScene::new();
                fresh_scene.set_selection(Some(selection));
                assert_eq!(
                    live,
                    font_pixels(renderer, &mut fresh_scene, &state, &mut fresh)
                );
                assert_eq!(selection.text(state.screen()), copied);
                if offset == 0 {
                    assert_eq!(live, original);
                }
            }
        }
    });
}

#[test]
fn baseline_offset_full_apply_equivalent_reloads_keep_rows_and_gpu_uploads() {
    use crate::emulation::{color::Paints, selection::Selection};
    use crate::testing::painter::Painter;
    use crate::window::{apply_fonts, apply_settings, effective_font_options, prepare_fonts};
    use yinz_types::input::native_terminal::TerminalConfig;

    let uploads = |renderer: &Renderer| {
        [
            &renderer.solid_rows,
            &renderer.glyph_rows,
            &renderer.color_rows,
        ]
        .map(|buffer| (buffer.whole_uploads, buffer.sub_uploads))
    };
    with_renderer(|renderer| {
        for distinct_styles in [false, true] {
            for (zoom, scale) in [(1.0, 1.0), (1.15, 1.25), (1.0, 2.0)] {
                let mut settings = TerminalConfig {
                    font_family: Some("Iosevka Term".into()),
                    system_fonts: Some(distinct_styles),
                    ..Default::default()
                };
                if distinct_styles {
                    settings.font_bold = Some("Noto Sans Mono CJK SC".into());
                    settings.font_italic = settings.font_bold.clone();
                    settings.font_bold_italic = settings.font_bold.clone();
                }
                let mut paints = Paints::default();
                let mut active = effective_font_options(&settings, zoom, scale);
                let mut cache = fixture_cache(&active);
                let mut state = font_state();
                Painter::apply(&mut state, |p| p.cluster(4, 6, "👩‍💻", 2));
                let selection = Selection::at(0, 3).extended_to(0, 4);
                let copied = selection.text(state.screen());
                let mut retained = RetainedScene::new();
                retained.set_selection(Some(selection));
                let original = font_pixels(renderer, &mut retained, &state, &mut cache);
                let mut previous = original.clone();
                let dimensions = (cache.metrics().width, cache.metrics().height);
                for (offset, equivalent) in [
                    (Some(0), true),
                    (None, true),
                    (Some(64), false),
                    (Some(63), true),
                    (Some(64), true),
                    (Some(-64), false),
                    (Some(-63), true),
                    (Some(-64), true),
                    (Some(2), false),
                    (Some(-2), false),
                    (None, false),
                ] {
                    let next = TerminalConfig {
                        baseline_offset: offset,
                        ..settings.clone()
                    };
                    let requested = effective_font_options(&next, zoom, scale);
                    // Verify equivalence includes placement and distinct style
                    // fitting, independently of the regular metric comparison.
                    let mut probe = fixture_cache(&active);
                    assert_eq!(
                        probe.update_cell(&requested) == CellChange::Equivalent,
                        equivalent
                    );
                    let before = uploads(renderer);
                    let generation = cache.generation();
                    let stamps = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
                    let update = prepare_fonts(&active, &requested).unwrap();
                    apply_fonts(
                        &mut active,
                        &mut cache,
                        &mut retained,
                        requested.clone(),
                        update,
                    );
                    let next_paints = paints;
                    apply_settings(
                        &mut settings,
                        &mut paints,
                        &mut retained,
                        next.clone(),
                        next_paints,
                    );
                    assert_eq!(settings, next);
                    assert_eq!(active, requested);
                    assert_eq!((cache.metrics().width, cache.metrics().height), dimensions);

                    // Only this synchronous draw is measured. No fresh scene or
                    // atlas can force uploads between the two counter snapshots.
                    let live = font_pixels(renderer, &mut retained, &state, &mut cache);
                    let after = uploads(renderer);
                    if equivalent {
                        assert!(
                            !retained.rebuilt_everything(),
                            "{offset:?}, {zoom}, {scale}, styles={distinct_styles}"
                        );
                        assert!(retained.rebuilt_rows().is_empty());
                        assert_eq!(after, before, "equivalent reload uploaded row buffers");
                        assert_eq!(live, previous);
                        assert_eq!(cache.generation(), generation);
                        assert_eq!(
                            (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
                            stamps
                        );
                    } else {
                        assert!(retained.rebuilt_everything());
                        for (after, before) in after.into_iter().zip(before) {
                            assert!(
                                after.0 > before.0,
                                "effective baseline change must upload each row buffer"
                            );
                        }
                        let mut fresh_cache = fixture_cache(&requested);
                        let mut fresh = RetainedScene::new();
                        fresh.set_selection(Some(selection));
                        assert_eq!(
                            live,
                            font_pixels(renderer, &mut fresh, &state, &mut fresh_cache)
                        );
                        // Restore the live scene/atlases before measuring the
                        // next reload; the comparator intentionally used new ones.
                        assert_eq!(
                            live,
                            font_pixels(renderer, &mut retained, &state, &mut cache)
                        );
                    }
                    if offset.is_none() {
                        assert_eq!(live, original);
                    }
                    assert_eq!(selection.text(state.screen()), copied);
                    previous = live;
                }
            }
        }
    });
}

fn baseline_offset_image_state() -> crate::emulation::terminal::TerminalState {
    baseline_offset_image_state_with([0, 255, 0, 255])
}

fn baseline_offset_image_state_with(color: [u8; 4]) -> crate::emulation::terminal::TerminalState {
    use crate::testing::painter::{residency, sixel_chunk, Painter, Place};
    let mut state = font_state();
    Painter::apply(&mut state, |p| {
        p.cluster(4, 6, "👩‍💻", 2);
        p.graphics(&[
            residency(1, 12, 8, color),
            Place::of(1, 12, 8).at(1, 20).record(),
            sixel_chunk(3, 20, 8, 6, "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\"),
        ]);
    });
    state
}

#[derive(Debug, PartialEq)]
struct InlineImageResources {
    vertices: Vec<f32>,
    batches: Vec<(ImageKey, ImageDepth, i32, i32)>,
    textures: HashMap<ImageKey, (glow::Texture, u64)>,
}

fn baseline_offset_image_resources(renderer: &Renderer) -> InlineImageResources {
    InlineImageResources {
        vertices: renderer.image_vertices.clone(),
        batches: renderer
            .image_batches
            .iter()
            .map(|b| (b.key, b.depth, b.first, b.count))
            .collect(),
        textures: renderer
            .images
            .iter()
            .map(|(&k, v)| (k, (v.texture, v.revision)))
            .collect(),
    }
}

#[test]
fn baseline_offset_full_apply_retains_inline_image_vertices_and_pixels() {
    use crate::emulation::{color::Paints, selection::Selection};
    use crate::window::{apply_fonts, apply_settings, effective_font_options, prepare_fonts};
    use yinz_types::input::native_terminal::TerminalConfig;
    with_renderer(|renderer| {
        for (zoom, scale) in [(1.0, 1.0), (1.15, 1.25), (1.0, 2.0)] {
            let mut settings = TerminalConfig {
                system_fonts: Some(false),
                ..Default::default()
            };
            let mut paints = Paints::default();
            let mut active = effective_font_options(&settings, zoom, scale);
            let mut cache = fixture_cache(&active);
            let state = baseline_offset_image_state();
            let selection = Selection::at(0, 3).extended_to(0, 4);
            let copied = selection.text(state.screen());
            let mut retained = RetainedScene::new();
            retained.set_selection(Some(selection));
            let original = font_pixels(renderer, &mut retained, &state, &mut cache);
            let metrics = cache.metrics();
            assert_eq!(retained.images().len(), 2);
            assert_eq!(renderer.images.len(), 2);
            assert!(!renderer.image_vertices.is_empty());
            let quads = retained.images().to_vec();
            let resident = retained.resident_images().to_vec();
            let resources = baseline_offset_image_resources(renderer);
            // These opaque, nonempty image regions contain no text or cursor.
            let image_pixels = |pixels: &[u8]| {
                quads
                    .iter()
                    .flat_map(|q| {
                        (q.y..q.y + q.height as i32).flat_map(move |y| {
                            (q.x..q.x + q.width as i32).map(move |x| {
                                let at = (((383 - y) * 768 + x) * 4) as usize;
                                <[u8; 4]>::try_from(&pixels[at..at + 4]).unwrap()
                            })
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let original_images = image_pixels(&original);
            assert!(original_images.contains(&[0, 255, 0, 255]));
            assert!(original_images.contains(&[255, 0, 0, 255]));
            for offset in [2, -2, 0] {
                let next = TerminalConfig {
                    baseline_offset: Some(offset),
                    ..settings.clone()
                };
                let requested = effective_font_options(&next, zoom, scale);
                let update = prepare_fonts(&active, &requested).unwrap();
                let before = (renderer.image_syncs, renderer.image_uploads);
                let rows = [
                    &renderer.solid_rows,
                    &renderer.glyph_rows,
                    &renderer.color_rows,
                ]
                .map(|r| r.whole_uploads);
                apply_fonts(
                    &mut active,
                    &mut cache,
                    &mut retained,
                    requested.clone(),
                    update,
                );
                let next_paints = paints;
                apply_settings(
                    &mut settings,
                    &mut paints,
                    &mut retained,
                    next.clone(),
                    next_paints,
                );
                let live = font_pixels(renderer, &mut retained, &state, &mut cache);
                // One synchronous live draw only; comparators/switches are outside
                // this interval, so they cannot manufacture an image upload.
                assert_eq!(
                    (renderer.image_syncs, renderer.image_uploads),
                    before,
                    "offset={offset}, zoom={zoom}, scale={scale}"
                );
                assert!(!retained.replaced_images());
                assert!(retained.rebuilt_everything());
                for (r, before) in [
                    &renderer.solid_rows,
                    &renderer.glyph_rows,
                    &renderer.color_rows,
                ]
                .into_iter()
                .zip(rows)
                {
                    assert!(r.whole_uploads > before);
                }
                assert_eq!(baseline_offset_image_resources(renderer), resources);
                assert_eq!(retained.images(), quads);
                assert_eq!(retained.resident_images(), resident);
                assert_eq!(image_pixels(&live), original_images);
                assert_eq!(
                    (cache.metrics().width, cache.metrics().height),
                    (metrics.width, metrics.height)
                );
                assert_eq!(
                    cache.metrics().baseline as i32,
                    metrics.baseline as i32 + (offset as f32 * zoom * scale as f32).round() as i32
                );
                assert_eq!(selection.text(state.screen()), copied);
                assert_eq!(settings, next);
                if offset == 0 {
                    assert_eq!(live, original);
                } else {
                    assert_ne!(live, original);
                }
                let mut fresh = RetainedScene::new();
                fresh.set_selection(Some(selection));
                let mut fresh_cache = fixture_cache(&requested);
                assert_eq!(
                    font_pixels(renderer, &mut fresh, &state, &mut fresh_cache),
                    live
                );
                assert_eq!(
                    font_pixels(renderer, &mut retained, &state, &mut cache),
                    live
                );
                assert_eq!(baseline_offset_image_resources(renderer), resources);
            }
        }
    });
}

#[test]
fn baseline_offset_inline_image_changes_still_sync_and_match_fresh_pixels() {
    use crate::render::scene::{CellRect, HeldOut};
    use crate::testing::painter::{residency, Painter, Place};
    use crate::window::{apply_fonts, effective_font_options, prepare_fonts};
    use yinz_types::{
        input::native_terminal::TerminalConfig,
        structured_render::{GeometryRecord, PaneRect, PANE_SELECTABLE},
    };
    with_renderer(|renderer| {
        let mut settings = TerminalConfig {
            system_fonts: Some(false),
            ..Default::default()
        };
        let mut active = effective_font_options(&settings, 1.0, 1.0);
        let mut cache = fixture_cache(&active);
        let mut state = baseline_offset_image_state();
        let mut retained = RetainedScene::new();
        let mut held = vec![];
        font_pixels(renderer, &mut retained, &state, &mut cache);
        for change in [
            "move", "revision", "width", "height", "pane", "held", "unheld", "delete",
        ] {
            let old_quads = retained.images().to_vec();
            match change {
                "move" => Painter::apply_marking(&mut state, &mut retained, |p| {
                    p.graphics(&[Place::of(1, 12, 8).at(1, 19).offset(3, 0).record()])
                }),
                "revision" => Painter::apply_marking(&mut state, &mut retained, |p| {
                    p.graphics(&[residency(1, 12, 8, [0, 0, 255, 255])])
                }),
                "width" => settings.cell_width = Some(1.5),
                "height" => settings.line_height = Some(1.5),
                "pane" => Painter::apply_marking(&mut state, &mut retained, |p| {
                    p.geometry(&GeometryRecord {
                        panes: vec![
                            PaneRect {
                                x: 0,
                                y: 0,
                                cols: 19,
                                rows: 5,
                                top: 0,
                                bottom: 0,
                                left: 0,
                                right: 0,
                                flags: PANE_SELECTABLE,
                            },
                            PaneRect {
                                x: 19,
                                y: 0,
                                cols: 1,
                                rows: 5,
                                top: 0,
                                bottom: 0,
                                left: 0,
                                right: 0,
                                flags: PANE_SELECTABLE,
                            },
                        ],
                    })
                }),
                "held" => {
                    held = vec![HeldOut {
                        content: CellRect {
                            x: 19,
                            y: 1,
                            cols: 2,
                            rows: 1,
                        },
                        covers: vec![],
                    }]
                },
                "unheld" => held.clear(),
                "delete" => Painter::apply_marking(&mut state, &mut retained, |p| p.full_repaint()),
                _ => unreachable!(),
            }
            retained.set_held_out(held.clone());
            let requested = effective_font_options(&settings, 1.0, 1.0);
            let update = prepare_fonts(&active, &requested).unwrap();
            apply_fonts(
                &mut active,
                &mut cache,
                &mut retained,
                requested.clone(),
                update,
            );
            let before = (renderer.image_syncs, renderer.image_uploads);
            let live = font_pixels(renderer, &mut retained, &state, &mut cache);
            assert!(retained.replaced_images(), "{change}");
            assert_eq!(renderer.image_syncs, before.0 + 1, "{change}");
            assert_eq!(
                renderer.image_uploads,
                before.1 + usize::from(!retained.images().is_empty()),
                "{change}"
            );
            assert_ne!(retained.images(), old_quads, "{change}");
            if change == "revision" {
                assert_eq!(
                    renderer.images[&ImageKey::Kitty(1)].revision,
                    retained
                        .all_images()
                        .iter()
                        .find(|q| q.key == ImageKey::Kitty(1))
                        .unwrap()
                        .revision
                );
            }
            if change == "delete" {
                assert!(renderer.image_vertices.is_empty());
                assert!(renderer.image_batches.is_empty());
                assert!(renderer.images.is_empty());
                assert!(retained.resident_images().is_empty());
            }
            let mut fresh = RetainedScene::new();
            fresh.set_held_out(held.clone());
            let mut fresh_cache = fixture_cache(&requested);
            assert_eq!(
                font_pixels(renderer, &mut fresh, &state, &mut fresh_cache),
                live,
                "{change}"
            );
            assert_eq!(retained.all_images(), fresh.all_images(), "{change}");
            assert_eq!(retained.images(), fresh.images(), "{change}");
            assert_eq!(
                retained.resident_images(),
                fresh.resident_images(),
                "{change}"
            );
            assert_eq!(
                font_pixels(renderer, &mut retained, &state, &mut cache),
                live
            );
        }
    });
}

#[test]
fn baseline_offset_inline_images_initialize_on_scene_switch_and_clear() {
    use crate::emulation::color::Paints;
    use crate::render::scene::{BlinkPhase, CursorOptions, SceneInputs};
    use crate::testing::painter::{Painter, Place};
    use yinz_types::input::native_terminal::TerminalConfig;
    with_renderer(|renderer| {
        let options = crate::window::effective_font_options(
            &TerminalConfig {
                system_fonts: Some(false),
                ..Default::default()
            },
            1.0,
            1.0,
        );
        let mut cache = fixture_cache(&options);
        let state_a = baseline_offset_image_state();
        let mut state_b = baseline_offset_image_state();
        Painter::apply(&mut state_b, |p| {
            p.graphics(&[Place::of(1, 12, 8).at(1, 19).record()]);
        });
        let mut a = RetainedScene::new();
        let mut b = RetainedScene::new();
        // Pre-refresh both scenes to remove their initial change flags before
        // the renderer ever sees them. Identity must initialize image resources.
        for (scene, state) in [(&mut a, &state_a), (&mut b, &state_b)] {
            for _ in 0..2 {
                scene.refresh(
                    SceneInputs {
                        state,
                        phase: BlinkPhase::On,
                        paints: &Paints::default(),
                        cursor: CursorOptions::default(),
                        hovered_link: None,
                        preedit: None,
                    },
                    &mut cache,
                );
            }
            assert!(!scene.replaced_images());
            assert!(!scene.rebuilt_everything());
        }
        let before = (renderer.image_syncs, renderer.image_uploads);
        let pixels_a = font_pixels(renderer, &mut a, &state_a, &mut cache);
        assert_eq!(
            (renderer.image_syncs, renderer.image_uploads),
            (before.0 + 1, before.1 + 1)
        );
        assert_eq!(renderer.images.len(), 2);
        let before = (renderer.image_syncs, renderer.image_uploads);
        let pixels_b = font_pixels(renderer, &mut b, &state_b, &mut cache);
        assert_eq!(
            (renderer.image_syncs, renderer.image_uploads),
            (before.0 + 1, before.1 + 1)
        );
        assert!(pixels_a != pixels_b, "the moved image must change pixels");
        let before = (renderer.image_syncs, renderer.image_uploads);
        assert_eq!(
            font_pixels(renderer, &mut a, &state_a, &mut cache),
            pixels_a
        );
        assert_eq!(
            (renderer.image_syncs, renderer.image_uploads),
            (before.0 + 1, before.1 + 1)
        );
        // Clearing must also reinitialize an unchanged, already refreshed scene.
        renderer.clear_images();
        assert!(renderer.images.is_empty());
        let before = (renderer.image_syncs, renderer.image_uploads);
        assert_eq!(
            font_pixels(renderer, &mut a, &state_a, &mut cache),
            pixels_a
        );
        assert_eq!(
            (renderer.image_syncs, renderer.image_uploads),
            (before.0 + 1, before.1 + 1)
        );
        assert_eq!(renderer.images.len(), 2);
        let before = (renderer.image_syncs, renderer.image_uploads);
        assert_eq!(
            font_pixels(renderer, &mut a, &state_a, &mut cache),
            pixels_a
        );
        assert_eq!((renderer.image_syncs, renderer.image_uploads), before);
        // A new connection can reuse both ID and revision with different pixels.
        let reconnected = baseline_offset_image_state_with([0, 0, 255, 255]);
        renderer.clear_images();
        let mut fresh = RetainedScene::new();
        let mut expected = pixels_a.clone();
        let mut changed = 0;
        for pixel in expected.as_chunks_mut::<4>().0 {
            if *pixel == [0, 255, 0, 255] {
                pixel.copy_from_slice(&[0, 0, 255, 255]);
                changed += 1;
            }
        }
        assert_eq!(changed, 12 * 8);
        assert!(
            font_pixels(renderer, &mut fresh, &reconnected, &mut cache) == expected,
            "clear must upload new pixels for the reused ID/revision"
        );
        assert_eq!(a.all_images()[1].key, fresh.all_images()[1].key);
        assert_eq!(a.all_images()[1].revision, fresh.all_images()[1].revision);
    });
}

fn baseline_offset_atlas_retry_pressure(
    cache: &mut crate::render::atlas::GlyphCache,
    ordinary: bool,
) {
    use crate::text::font::FaceStyle;
    for budget in 1..=32768 {
        cache.glyph(FaceStyle::Regular, 'A', budget);
        if cache.exhausted() {
            break;
        }
    }
    assert!(cache.exhausted());
    assert!(cache.may_flush(std::time::Instant::now()));
    if ordinary {
        assert_eq!(cache.entries(), 32768);
    } else {
        assert!(cache.entries() < 32768);
    }
}

fn baseline_offset_atlas_retry_pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = (((383 - y) * 768 + x) * 4) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

fn baseline_offset_atlas_retry_images(renderer: &mut Renderer, change: &str, ordinary: bool) {
    use crate::emulation::selection::Selection;
    use crate::render::{
        atlas::GlyphCache,
        scene::{CellRect, HeldOut},
    };
    use crate::testing::painter::{residency, sixel_chunk, Painter, Place};
    use crate::text::font::FontOptions;
    use yinz_types::structured_render::{GeometryRecord, PaneRect, PANE_SELECTABLE};
    let mut options = FontOptions {
        system_fonts: false,
        ..Default::default()
    };
    let make_cache = |options: &FontOptions| {
        let fonts = options.stack(1.0).unwrap();
        if ordinary {
            GlyphCache::new(fonts)
        } else {
            GlyphCache::tiny(fonts)
        }
    };
    let mut cache = make_cache(&options);
    let mut state = Painter::state(4, 8, |p| {
        p.text(0, 0, "A");
        p.text(0, 5, "A");
        p.graphics(&[
            residency(1, 24, 8, [255; 4]),
            Place::of(1, 24, 8).at(1, 1).record(),
            sixel_chunk(3, 4, 8, 6, "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\"),
        ]);
    });
    let held = || {
        vec![HeldOut {
            content: CellRect {
                x: 1,
                y: 1,
                cols: 3,
                rows: 1,
            },
            covers: vec![],
        }]
    };
    let mut retained = RetainedScene::new();
    // Keep selection out of the old image probe when taller cells move that
    // physical point into text row zero.
    let selection = Selection::at(0, 5).extended_to(0, 6);
    let copied = selection.text(state.screen());
    retained.set_selection(Some(selection));
    let mut held_regions = if change == "unheld" { held() } else { vec![] };
    retained.set_held_out(held_regions.clone());
    // Every scenario has its own cache, scene and connection-owned image state.
    renderer.clear_images();
    let original = font_pixels(renderer, &mut retained, &state, &mut cache);
    assert!(!cache.exhausted(), "initial compact scene must fit");
    assert_eq!(retained.all_images().len(), 2);
    assert_eq!(renderer.images.len(), 2);
    let metrics = cache.metrics();
    let old_point = (metrics.width, metrics.height);
    let sixel_point = (4 * metrics.width, 3 * metrics.height);
    let black = [0, 0, 0, 255];
    assert_eq!(
        baseline_offset_atlas_retry_pixel(&original, old_point.0, old_point.1),
        if change == "unheld" { black } else { [255; 4] }
    );
    assert_eq!(
        baseline_offset_atlas_retry_pixel(&original, sixel_point.0, sixel_point.1),
        [255, 0, 0, 255]
    );
    let identity = retained.identity();
    let original_resources = baseline_offset_image_resources(renderer);
    match change {
        "move" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[Place::of(1, 24, 8).at(1, 4).record()]);
        }),
        "revision" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[residency(1, 24, 8, [0, 0, 255, 255])]);
        }),
        "delete" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.full_repaint();
            p.text(0, 0, "A");
            p.text(0, 5, "A");
        }),
        "sixel" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[sixel_chunk(
                3,
                4,
                8,
                6,
                "\x1bP0;1;0q\"1;1;8;6#0;2;0;0;100!8~\x1b\\",
            )]);
        }),
        "pane" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.geometry(&GeometryRecord {
                panes: vec![PaneRect {
                    x: 0,
                    y: 0,
                    cols: 2,
                    rows: 4,
                    top: 0,
                    bottom: 0,
                    left: 0,
                    right: 0,
                    flags: PANE_SELECTABLE,
                }],
            });
        }),
        "held" => held_regions = held(),
        "unheld" => held_regions.clear(),
        "width" => {
            options.cell.cell_width = 1.5;
            assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        },
        "height" => {
            options.cell.line_height = 1.5;
            assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        },
        "baseline" => {
            options.cell.baseline_offset = 2.0;
            assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        },
        "unchanged" => {},
        _ => unreachable!(),
    }
    retained.set_held_out(held_regions.clone());
    // update_cell already flushes. Fill afterward to prove an additional,
    // eligible renewal occurs inside this measured public refresh/draw.
    let generation = cache.generation();
    let flushes = cache.flushes();
    baseline_offset_atlas_retry_pressure(&mut cache, ordinary);
    let before = (renderer.image_syncs, renderer.image_uploads);
    let rows_before = renderer.glyph_rows.whole_uploads;
    // Exactly one synchronous draw, with no comparator/identity switch in this
    // interval: these call deltas cannot depend on scheduling or render counts.
    let live = font_pixels(renderer, &mut retained, &state, &mut cache);
    assert_eq!(cache.flushes(), flushes + 1, "{change}");
    assert_ne!(cache.generation(), generation);
    assert_eq!(retained.identity(), identity);
    assert!(!cache.exhausted(), "post-renewal scene must fit: {change}");
    assert!(retained.rebuilt_everything());
    assert!(renderer.glyph_rows.whole_uploads > rows_before);
    let pixel = |x, y| baseline_offset_atlas_retry_pixel(&live, x, y);
    // Independent color/location oracles precede fresh-scene comparisons and
    // notification assertions, exposing actual stale pixels on the old code.
    match change {
        "move" => {
            assert_eq!(
                pixel(old_point.0, old_point.1),
                black,
                "obsolete image position"
            );
            assert_eq!(pixel(4 * metrics.width, metrics.height), [255; 4]);
        },
        "revision" => assert_eq!(pixel(old_point.0, old_point.1), [0, 0, 255, 255]),
        "delete" => {
            assert_eq!(pixel(old_point.0, old_point.1), black);
            assert_eq!(pixel(sixel_point.0, sixel_point.1), black);
        },
        "sixel" => assert_eq!(pixel(sixel_point.0, sixel_point.1), [0, 0, 255, 255]),
        "held" => assert_eq!(pixel(old_point.0, old_point.1), black),
        "unheld" => assert_eq!(pixel(old_point.0, old_point.1), [255; 4]),
        "pane" => {
            assert_eq!(pixel(old_point.0, old_point.1), [255; 4]);
            assert_eq!(pixel(2 * metrics.width, metrics.height), black);
        },
        "width" | "height" => {
            assert_eq!(pixel(old_point.0, old_point.1), black);
            assert_eq!(
                pixel(cache.metrics().width, cache.metrics().height),
                [255; 4]
            );
        },
        "unchanged" | "baseline" => {
            assert_eq!(pixel(old_point.0, old_point.1), [255; 4]);
            assert_eq!(pixel(sixel_point.0, sixel_point.1), [255, 0, 0, 255]);
            assert_eq!(
                baseline_offset_image_resources(renderer),
                original_resources
            );
        },
        _ => unreachable!(),
    }
    let changed = !matches!(change, "baseline" | "unchanged");
    assert_eq!(retained.replaced_images(), changed, "{change}");
    assert_eq!(
        renderer.image_syncs,
        before.0 + usize::from(changed),
        "{change}"
    );
    assert_eq!(
        renderer.image_uploads,
        before.1 + usize::from(changed && !retained.images().is_empty()),
        "{change}"
    );
    let mut expected_vertices = vec![];
    for image in retained.images() {
        push_image_vertices(&mut expected_vertices, image);
    }
    assert_eq!(renderer.image_vertices, expected_vertices);
    assert_eq!(
        renderer
            .image_batches
            .iter()
            .map(|b| b.count as usize)
            .sum::<usize>(),
        retained.images().len() * 6
    );
    assert_eq!(renderer.images.len(), retained.resident_images().len());
    for image in retained.all_images() {
        assert_eq!(renderer.images[&image.key].revision, image.revision);
    }
    if change == "delete" {
        assert!(renderer.image_batches.is_empty());
        assert!(renderer.images.is_empty());
    }
    assert_eq!(selection.text(state.screen()), copied);
    let mut fresh = RetainedScene::new();
    fresh.set_selection(Some(selection));
    fresh.set_held_out(held_regions);
    let mut fresh_cache = make_cache(&options);
    assert!(
        font_pixels(renderer, &mut fresh, &state, &mut fresh_cache) == live,
        "live/fresh frame mismatch: {change}"
    );
    assert!(!fresh_cache.exhausted());
    assert_eq!(retained.all_images(), fresh.all_images());
    assert_eq!(retained.images(), fresh.images());
    assert_eq!(retained.resident_images(), fresh.resident_images());
    // Restore live resources after the independent comparator before taking
    // counters for another unchanged public refresh/draw.
    assert!(font_pixels(renderer, &mut retained, &state, &mut cache) == live);
    let resources = baseline_offset_image_resources(renderer);
    let before = (renderer.image_syncs, renderer.image_uploads);
    assert!(font_pixels(renderer, &mut retained, &state, &mut cache) == live);
    assert!(!retained.replaced_images());
    assert!(!retained.rebuilt_everything());
    assert!(retained.rebuilt_rows().is_empty());
    assert_eq!((renderer.image_syncs, renderer.image_uploads), before);
    assert_eq!(baseline_offset_image_resources(renderer), resources);
    assert_eq!(cache.flushes(), flushes + 1);
}

#[test]
fn baseline_offset_atlas_retry_moved_image_matches_independent_pixels() {
    with_renderer(|renderer| baseline_offset_atlas_retry_images(renderer, "move", false));
}

#[test]
fn baseline_offset_atlas_retry_ordinary_entry_limit_moves_image_pixels() {
    with_renderer(|renderer| baseline_offset_atlas_retry_images(renderer, "move", true));
}

#[test]
fn baseline_offset_atlas_retry_image_updates_and_clipping_match_fresh_pixels() {
    with_renderer(|renderer| {
        for change in [
            "revision", "delete", "sixel", "held", "unheld", "pane", "width", "height",
        ] {
            baseline_offset_atlas_retry_images(renderer, change, false);
        }
    });
}

#[test]
fn baseline_offset_atlas_retry_text_only_renewal_retains_image_resources() {
    with_renderer(|renderer| {
        for change in ["unchanged", "baseline"] {
            baseline_offset_atlas_retry_images(renderer, change, false);
        }
    });
}

#[test]
fn underline_metrics_saved_reload_updates_only_solids_and_survives_skipped_draws() {
    use crate::emulation::{color::Paints, selection::Selection, terminal::TerminalState};
    use crate::render::scene::{BlinkPhase, CursorOptions, SceneInputs};
    use crate::testing::painter::Painter;
    use crate::window::{effective_font_options, refresh_drawable, PreparedReload};
    use yinz_cli::args::CliArgs;
    use yinz_types::structured_render::{WireColor, UNDERLINE_SHIFT, UNDERLINE_STRAIGHT};

    with_renderer(|renderer| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let write = |settings: &str| {
            std::fs::write(
                &path,
                format!("terminal {{ system_fonts false; {settings} }}"),
            )
            .unwrap();
        };
        write("");
        let opts = CliArgs {
            config: Some(path.clone()),
            config_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let mut settings = yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal;
        let mut paints = Paints::default();
        let (zoom, scale) = (1.15, 1.25);
        let mut active = effective_font_options(&settings, zoom, scale);
        let mut cache = fixture_cache(&active);
        let mut state = baseline_offset_image_state();
        let red = [245, 37, 81, 255];
        Painter::apply(&mut state, |p| {
            p.styled(0, 8, " ", |cell| {
                cell.attrs |= UNDERLINE_STRAIGHT << UNDERLINE_SHIFT;
                cell.underline_color = WireColor::Rgb(red[0], red[1], red[2]).pack();
            });
        });
        let selection = Selection::at(0, 3).extended_to(0, 4);
        let copied = selection.text(state.screen());
        let mut retained = RetainedScene::new();
        retained.set_selection(Some(selection));
        let target = (768, 384);
        let refresh = |size,
                       scene: &mut RetainedScene,
                       state: &TerminalState,
                       paints: &Paints,
                       cache: &mut crate::render::atlas::GlyphCache| {
            refresh_drawable(
                size,
                scene,
                SceneInputs {
                    state,
                    phase: BlinkPhase::On,
                    paints,
                    cursor: CursorOptions::default(),
                    hovered_link: None,
                    preedit: None,
                },
                cache,
            )
        };
        let uploads = |renderer: &Renderer| {
            [
                &renderer.solid_rows,
                &renderer.glyph_rows,
                &renderer.color_rows,
            ]
            .map(|rows| (rows.whole_uploads, rows.sub_uploads))
        };
        let atlas_stamps = |cache: &crate::render::atlas::GlyphCache| {
            (cache.atlases().mask.stamp(), cache.atlases().color.stamp())
        };
        let image_resources = baseline_offset_image_resources;

        assert_eq!(
            refresh(Some(target), &mut retained, &state, &paints, &mut cache),
            Some(target)
        );
        let original = draw_and_read(renderer, &retained, cache.atlases(), target);
        let metrics = cache.metrics();
        let x = 8 * metrics.width + metrics.width / 2;
        assert_eq!(
            baseline_offset_atlas_retry_pixel(&original, x, metrics.underline_top),
            red
        );
        let original_uploads = uploads(renderer);
        let generation = cache.generation();
        let font = cache.primary_of(crate::text::font::FaceStyle::Regular);
        let entries = cache.entries();
        let runs = cache.shaped_runs();
        let stamps = atlas_stamps(&cache);
        let images = image_resources(renderer);
        assert!(!images.vertices.is_empty());
        let row = retained.rows()[0].glyphs.clone();
        for (fragment, offset) in [
            ("underline_offset 0; underline_thickness 0;", 0),
            ("underline_offset -4; underline_thickness 1;", -4),
        ] {
            write(fragment);
            PreparedReload::load(&opts, &active, zoom, scale)
                .unwrap()
                .apply(
                    &mut active,
                    &mut cache,
                    &mut retained,
                    &mut settings,
                    &mut paints,
                );
            refresh(Some(target), &mut retained, &state, &paints, &mut cache).unwrap();
            let live = draw_and_read(renderer, &retained, cache.atlases(), target);
            let uploaded = uploads(renderer);
            assert!(!retained.rebuilt_everything());
            assert!(retained.rebuilt_rows().is_empty());
            assert_eq!(cache.generation(), generation);
            assert_eq!(
                cache.primary_of(crate::text::font::FaceStyle::Regular),
                font
            );
            assert_eq!((cache.entries(), cache.shaped_runs()), (entries, runs));
            assert_eq!(atlas_stamps(&cache), stamps);
            assert_eq!(image_resources(renderer), images);
            assert_eq!(uploaded[1..], original_uploads[1..]);
            assert_eq!(retained.rows()[0].glyphs, row);
            if offset == 0 {
                assert_eq!(uploaded[0], original_uploads[0]);
                assert!(retained.rebuilt_solid_rows().is_empty());
                assert_eq!(live, original);
            } else {
                assert_ne!(uploaded[0], original_uploads[0]);
                assert_eq!(retained.rebuilt_solid_rows(), &[0]);
                let top = metrics.underline_top - 6;
                assert_eq!(cache.metrics().underline_top, top);
                assert_eq!(baseline_offset_atlas_retry_pixel(&live, x, top), red);
                assert_ne!(baseline_offset_atlas_retry_pixel(&original, x, top), red);
            }
        }

        let prior = draw_and_read(renderer, &retained, cache.atlases(), target);
        let prior_uploads = uploads(renderer);
        let prior_row = retained.rows()[0].rects.clone();
        let dirty_color = [17, 232, 177, 255];
        let damage = Painter::applied(&mut state, |p| {
            p.styled(2, 7, " ", |cell| {
                cell.bg = WireColor::Rgb(dirty_color[0], dirty_color[1], dirty_color[2]).pack()
            });
        })
        .damage;
        retained.mark(&damage);
        write("underline_offset -7; underline_thickness 2;");
        PreparedReload::load(&opts, &active, zoom, scale)
            .unwrap()
            .apply(
                &mut active,
                &mut cache,
                &mut retained,
                &mut settings,
                &mut paints,
            );
        for skipped in [None, Some((0, target.1)), Some((target.0, 0))] {
            assert_eq!(
                refresh(skipped, &mut retained, &state, &paints, &mut cache),
                None
            );
            assert_eq!(retained.rows()[0].rects, prior_row);
            assert_eq!(uploads(renderer), prior_uploads);
        }
        assert_eq!(
            refresh(Some(target), &mut retained, &state, &paints, &mut cache),
            Some(target)
        );
        assert!(!retained.rebuilt_everything());
        assert_eq!(retained.rebuilt_solid_rows(), &[0, 2]);
        assert_eq!(retained.rebuilt_rows(), &[2]);
        let live = draw_and_read(renderer, &retained, cache.atlases(), target);
        let spot = (7 * metrics.width + 1, 2 * metrics.height + 1);
        let top = metrics.underline_top - 10;
        assert_eq!(cache.metrics().underline_top, top);
        for (point, color) in [((x, top), red), (spot, dirty_color)] {
            assert_ne!(
                baseline_offset_atlas_retry_pixel(&prior, point.0, point.1),
                color
            );
            assert_eq!(
                baseline_offset_atlas_retry_pixel(&live, point.0, point.1),
                color
            );
        }
        assert_eq!(atlas_stamps(&cache), stamps);
        assert_eq!(image_resources(renderer), images);
        assert_eq!(selection.text(state.screen()), copied);

        let mut fresh = RetainedScene::new();
        fresh.set_selection(Some(selection));
        let mut fresh_cache = fixture_cache(&active);
        refresh(Some(target), &mut fresh, &state, &paints, &mut fresh_cache).unwrap();
        assert_eq!(
            live,
            draw_and_read(renderer, &fresh, fresh_cache.atlases(), target)
        );
    });
}

#[test]
fn underline_metrics_repeated_updates_preserve_order_over_missing_glyph_boxes() {
    use crate::testing::painter::Painter;
    use crate::window::{apply_fonts, effective_font_options, prepare_fonts};
    use yinz_config::config::Config;
    use yinz_types::structured_render::{
        WireColor, UNDERLINE_DOTTED, UNDERLINE_SHIFT, UNDERLINE_STRAIGHT,
    };
    with_renderer(|renderer| {
        let settings = |suffix: &str| {
            Config::from_kdl(
                &format!("terminal {{ system_fonts false; {suffix} }}"),
                None,
            )
            .unwrap()
            .terminal
        };
        let mut active = effective_font_options(&settings(""), 1.0, 1.0);
        let mut cache = fixture_cache(&active);
        let mut retained = RetainedScene::new();
        let line_color = [158, 28, 233, 255];
        let box_color = [242, 151, 77, 255];
        let dotted_color = [6, 248, 113];
        let missing_style = |cell: &mut yinz_types::structured_render::WireCell| {
            cell.attrs |= UNDERLINE_STRAIGHT << UNDERLINE_SHIFT;
            cell.fg = WireColor::Rgb(box_color[0], box_color[1], box_color[2]).pack();
            cell.underline_color =
                WireColor::Rgb(line_color[0], line_color[1], line_color[2]).pack();
        };
        let state = Painter::state(4, 12, |p| {
            p.styled(0, 0, "\u{10fffd}", missing_style);
            p.styled(0, 1, " ", |cell| {
                cell.attrs |= UNDERLINE_DOTTED << UNDERLINE_SHIFT;
                cell.underline_color =
                    WireColor::Rgb(dotted_color[0], dotted_color[1], dotted_color[2]).pack();
            });
            p.group_styled(0, 4, &[("a", 1), ("\u{10fffd}", 1)], missing_style);
        });
        let mut previous = font_pixels(renderer, &mut retained, &state, &mut cache);
        let metrics = cache.metrics();
        let middle = metrics.height / 2;
        let box_top = metrics.height / 8 + metrics.stroke;
        let dotted_count = |retained: &RetainedScene| {
            retained.rows()[0]
                .rects
                .iter()
                .filter(|rect| rect.color == dotted_color)
                .count()
        };
        let mut counts = vec![dotted_count(&retained)];
        let mut expected_frames = Vec::new();
        for (y, thickness) in [(middle, 2), (middle - 2, -3), (middle + 2, 3)] {
            let offset = y as i32 - metrics.underline_top as i32;
            let requested = effective_font_options(
                &settings(&format!(
                    "underline_offset {offset}; underline_thickness {thickness};"
                )),
                1.0,
                1.0,
            );
            let update = prepare_fonts(&active, &requested).unwrap();
            apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
            let live = font_pixels(renderer, &mut retained, &state, &mut cache);
            assert!(retained.rebuilt_rows().is_empty());
            assert_eq!(retained.rebuilt_solid_rows(), &[0]);
            counts.push(dotted_count(&retained));
            for column in [0, 5] {
                let x = column * metrics.width + metrics.stroke;
                assert_eq!(
                    baseline_offset_atlas_retry_pixel(&live, x, box_top),
                    box_color
                );
                assert_eq!(baseline_offset_atlas_retry_pixel(&live, x, y), line_color);
            }
            assert_ne!(live, previous);
            previous = live.clone();
            expected_frames.push((active.clone(), live));
        }
        assert!(
            counts.windows(2).any(|pair| pair[0] != pair[1]),
            "{counts:?}"
        );
        for (options, warm) in expected_frames {
            assert_eq!(
                warm,
                font_pixels(
                    renderer,
                    &mut RetainedScene::new(),
                    &state,
                    &mut fixture_cache(&options)
                )
            );
        }
    });
}
