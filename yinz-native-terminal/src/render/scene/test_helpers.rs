use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub clear: Srgb,
    pub rects: Vec<Rect>,
    pub glyphs: Vec<GlyphQuad>,
    pub color_glyphs: Vec<GlyphQuad>,
    pub images: Vec<ImageQuad>,
    pub resident_images: Vec<ImageKey>,
}

pub fn build(state: &TerminalState, cache: &mut GlyphCache) -> Scene {
    build_at(
        state,
        cache,
        BlinkPhase::On,
        &Paints::default(),
        CursorOptions::default(),
        None,
        None,
    )
}

pub fn build_at(
    state: &TerminalState,
    cache: &mut GlyphCache,
    phase: BlinkPhase,
    paints: &Paints,
    cursor: CursorOptions,
    hovered_link: Option<&LinkRun>,
    preedit: Option<&Preedit>,
) -> Scene {
    let scene = build_once(
        SceneInputs {
            state,
            phase,
            paints,
            cursor,
            hovered_link,
            preedit,
        },
        cache,
    );
    if !cache.exhausted() {
        return scene;
    }
    cache.flush();
    build_once(
        SceneInputs {
            state,
            phase,
            paints,
            cursor,
            hovered_link,
            preedit,
        },
        cache,
    )
}

pub(super) fn build_once(inputs: SceneInputs<'_>, cache: &mut GlyphCache) -> Scene {
    let state = inputs.state;
    let context = RowContext::new(inputs, cache);
    let mut scene = Scene {
        width: context.width(),
        height: context.height(),
        clear: context.clear,
        rects: Vec::new(),
        glyphs: Vec::new(),
        color_glyphs: Vec::new(),
        images: images_of(state, context.metrics),
        resident_images: resident_images(state),
    };

    let mut scratch = RowScratch::default();
    let mut row_scene = RowScene::default();
    for row in 0..context.size.rows {
        build_row(&context, cache, row, &mut scratch, &mut row_scene);
        scene.rects.append(&mut row_scene.rects);
        scene.glyphs.append(&mut row_scene.glyphs);
        scene.color_glyphs.append(&mut row_scene.color_glyphs);
    }

    scene
}
