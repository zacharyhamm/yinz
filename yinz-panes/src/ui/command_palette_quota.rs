//! A palette owns one bounded quota raster per client and releases it with the view.
use super::{QuotaPlacement, Style};
use std::{cell::RefCell, rc::Rc};
use yinz_runtime::errors::prelude::*;
use yinz_terminal::panes::{
    kitty_graphics::{DecodedImage, KittyFormat, KittyImageChunk, KittyImageStore, ScaledImageKey},
    sixel::PixelRect,
};
use yinz_types::pane_size::SizeInPixels;

pub struct QuotaImage {
    key: (u64, Style, SizeInPixels),
    color: yinz_types::data::PaletteColor,
    chunk: KittyImageChunk,
    store: Rc<RefCell<KittyImageStore>>,
}
impl QuotaImage {
    pub fn prepare(
        cache: &mut Option<Self>,
        placement: Option<&QuotaPlacement>,
        style: Style,
        cell: Option<SizeInPixels>,
        store: Rc<RefCell<KittyImageStore>>,
    ) -> Result<Option<KittyImageChunk>> {
        let Some((placement, cell, value)) = placement
            .zip(cell)
            .and_then(|(p, c)| p.display.value.map(|v| (p, c, v)))
        else {
            *cache = None;
            return Ok(None);
        };
        let key = (value, style, cell);
        if cache
            .as_ref()
            .is_none_or(|image| image.key != key || image.color != placement.display.color(&style))
        {
            *cache = None;
            let width = u32::try_from(cell.width.checked_mul(18).context("quota width overflow")?)
                .context("quota width overflow")?;
            let height = u32::try_from(
                cell.height
                    .checked_mul(2)
                    .context("quota height overflow")?,
            )
            .context("quota height overflow")?;
            let color = placement.display.color(&style);
            let bytes =
                agent_proto::quota::render_rgba(value, color, style.terminal, width, height)
                    .context("rendering palette quota")?;
            let mut images = store.borrow_mut();
            let id = images
                .store_image(DecodedImage {
                    bytes,
                    width,
                    height,
                    format: KittyFormat::Rgba32,
                })
                .map_err(|e| anyhow!("storing quota image: {e:?}"))?;
            images.add_placement_ref(id);
            let uid = images.next_placement_uid();
            drop(images);
            *cache = Some(Self {
                key,
                color,
                store,
                chunk: KittyImageChunk {
                    cell_x: placement.rect.x,
                    cell_y: placement.rect.y,
                    internal_image_id: id,
                    source_px_x: 0,
                    source_px_y: 0,
                    source_px_width: width as usize,
                    source_px_height: height as usize,
                    cell_offset_x: 0,
                    cell_offset_y: 0,
                    z_index: 1,
                    dest_cells: (18, 2),
                    variant_key: ScaledImageKey {
                        source: PixelRect {
                            x: 0,
                            y: 0,
                            width: width as usize,
                            height: height as usize,
                        },
                        size: (width as usize, height as usize),
                    },
                    scaled_px: None,
                    placement_uid: uid,
                },
            });
        }
        Ok(cache.as_ref().map(|image| KittyImageChunk {
            cell_x: placement.rect.x,
            cell_y: placement.rect.y,
            ..image.chunk
        }))
    }
}
impl Drop for QuotaImage {
    fn drop(&mut self) {
        self.store.borrow_mut().free(self.chunk.internal_image_id);
    }
}

#[cfg(test)]
#[path = "unit/command_palette_quota_tests.rs"]
mod tests;
