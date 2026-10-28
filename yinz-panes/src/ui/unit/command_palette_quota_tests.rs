use super::*;
use crate::ui::damage::CellRect;
use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};

#[test]
fn client_quota_caches_reuse_and_release_rasters_on_theme_geometry_value_and_fallback() {
    let store = Rc::new(RefCell::new(KittyImageStore::default()));
    let mut first = None;
    let mut second = None;
    let mut style = Style::default();
    let mut cell = Some(SizeInPixels {
        width: 10,
        height: 20,
    });
    let mut placement = QuotaPlacement {
        rect: CellRect {
            x: 4,
            y: 5,
            cols: 18,
            rows: 2,
        },
        line: 1,
        display: agent_proto::quota::Display::new(
            &ProviderUsage {
                provider: "codex".into(),
                status: ProviderUsageStatus::Ok,
                observed_at_ms: None,
                plan: None,
                message: None,
                windows: vec![ProviderUsageWindow {
                    label: "7d".into(),
                    used_percent: 75.0,
                    resets_at_ms: None,
                }],
            },
            0,
        ),
    };
    let one = QuotaImage::prepare(&mut first, Some(&placement), style, cell, store.clone())
        .unwrap()
        .unwrap();
    let two = QuotaImage::prepare(&mut second, Some(&placement), style, cell, store.clone())
        .unwrap()
        .unwrap();
    assert_ne!(one.internal_image_id, two.internal_image_id);
    assert_eq!(store.borrow().image_count(), 2);
    let again = QuotaImage::prepare(&mut first, Some(&placement), style, cell, store.clone())
        .unwrap()
        .unwrap();
    assert_eq!(one, again);
    for change in 0..4 {
        let old = first.as_ref().unwrap().chunk.internal_image_id;
        match change {
            0 => placement.display.value = Some(10),
            1 => {
                style.colors.text_unselected.emphasis_1 =
                    yinz_types::data::PaletteColor::Rgb((3, 4, 5))
            },
            2 => {
                cell = Some(SizeInPixels {
                    width: 12,
                    height: 24,
                })
            },
            _ => placement.display.stale = true,
        }
        let updated = QuotaImage::prepare(&mut first, Some(&placement), style, cell, store.clone())
            .unwrap()
            .unwrap();
        assert_ne!(old, updated.internal_image_id);
        assert!(store.borrow().get(old).is_none());
        assert!(store.borrow().get(two.internal_image_id).is_some());
        assert_eq!(store.borrow().image_count(), 2);
    }
    // Missing cell geometry or graphics support takes the same explicit fallback path.
    assert!(
        QuotaImage::prepare(&mut first, Some(&placement), style, None, store.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(store.borrow().image_count(), 1);
    assert!(
        QuotaImage::prepare(&mut second, None, style, cell, store.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(store.borrow().image_count(), 0);
    QuotaImage::prepare(&mut first, Some(&placement), style, cell, store.clone()).unwrap();
    drop(first); // Closing the palette releases its raster without waiting for another render.
    assert_eq!(store.borrow().image_count(), 0);
}
