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
            .build(NonZeroU32::new(2).unwrap(), NonZeroU32::new(1).unwrap());
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
