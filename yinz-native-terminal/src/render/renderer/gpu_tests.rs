//! Pixel regressions use Mesa's headless EGL device on Linux.
use super::*;
use crate::render::atlas::GlyphAtlas;
use crate::render::scene::test_helpers::Scene;
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
    renderer.draw_retained(retained, cache.atlases(), (768, 384), Frame::default());
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
                ink.chunks_exact(4).any(|p| p[0] != 0),
                "row {row} has no glyph pixels"
            );
            ink
        };
        let mut cjk_ink = false;
        for y in 4 * metrics.height as usize..5 * metrics.height as usize {
            let offset = (383 - y) * 768 * 4;
            cjk_ink |= pixels[offset..offset + metrics.width as usize * 2 * 4]
                .chunks_exact(4)
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
                .chunks_exact(4)
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
                Some(replacement),
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
