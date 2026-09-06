//! Image, placement and deletion semantics, asserted against the specification.
//!
//! Derived from `graphics-protocol.rst` ("Image transmission", "Image display", "Deleting
//! images", "Image persistence and storage") and from kitty's `test_load_images`,
//! `test_gr_operations_with_numbers`, `test_image_put` and `test_gr_delete`.
//!
//! This level works directly on `KittyGrid`, one step below the terminal grid: it can assert on
//! placement geometry in pixels and on image lifetime in the store without also depending on
//! cursor movement or scrollback. The deviations still outstanding are indexed in the module doc
//! of `kitty_graphics/mod.rs`.

use super::super::grid_state::{KittyGrid, KittyVerticalAnchor};
use super::super::parser::{
    KittyAction, KittyCommand, KittyCommandParser, KittyError, KittyErrorCode,
};
use super::super::store::KittyImageStore;
use super::super::test_support::*;
use std::cell::RefCell;
use std::rc::Rc;
use zellij_types::pane_size::SizeInPixels;

const CELL: SizeInPixels = SizeInPixels {
    width: 10,
    height: 20,
};

/// A `KittyGrid` plus the pieces of terminal state its API takes as arguments, so a test can say
/// "run this command" instead of assembling six positional parameters each time.
struct Model {
    kitty: KittyGrid,
    store: Rc<RefCell<KittyImageStore>>,
    parser: KittyCommandParser,
    cursor_px: (usize, usize),
    cursor_cell: (usize, usize),
    viewport_cells: (usize, usize),
    scrollback_rows: usize,
}

impl Model {
    fn new() -> Self {
        let store = Rc::new(RefCell::new(KittyImageStore::default()));
        let kitty = KittyGrid::new(Rc::new(RefCell::new(Some(CELL))), store.clone());
        Model {
            kitty,
            store,
            parser: KittyCommandParser::new(),
            cursor_px: (0, 0),
            cursor_cell: (0, 0),
            viewport_cells: (20, 10),
            scrollback_rows: 0,
        }
    }

    fn parse(&mut self, control: &str, payload: &[u8]) -> Result<KittyCommand, KittyError> {
        self.parser
            .parse(unwrap_apc(&apc_b64(control, payload)))
            .expect("a complete command produces a result")
    }

    /// Run one command the way `Grid::execute_kitty_command` does, returning the image id the
    /// terminal would report back.
    fn run(&mut self, control: &str, payload: &[u8]) -> Result<Option<u32>, KittyError> {
        let command = self.parse(control, payload)?;
        match command.action {
            KittyAction::Transmit => {
                let image = command.image.clone().expect("transmit carries an image");
                self.kitty.transmit(&command, image).map(Some)
            },
            KittyAction::TransmitAndDisplay => {
                let image = command.image.clone().expect("transmit carries an image");
                let id = self.kitty.transmit(&command, image)?;
                let internal = *self
                    .kitty
                    .pane_image_id_map()
                    .get(&id)
                    .expect("a freshly transmitted image is mapped");
                self.place(id, internal, &command)?;
                Ok(Some(id))
            },
            KittyAction::Display => {
                let (id, internal) = self.kitty.resolve_display_target(&command)?;
                self.place(id, internal, &command)?;
                Ok(Some(id))
            },
            KittyAction::Delete => {
                self.kitty.delete(
                    &command,
                    self.cursor_cell,
                    self.viewport_cells,
                    self.scrollback_rows,
                )?;
                Ok(None)
            },
            // A query validates the payload and reports back without storing anything, so there
            // is nothing for `KittyGrid` to do. This arm is a stub: the query rules themselves are
            // asserted against the real command path in `grid_tests::kitty_spec`, not here.
            KittyAction::Query => Ok(command.image_id),
            KittyAction::Frame => {
                let image = command.image.clone().expect("a frame carries its data");
                self.kitty.transmit_frame(&command, image).map(Some)
            },
            KittyAction::Animate => self.kitty.animate(&command).map(|_| command.image_id),
            KittyAction::Compose => self
                .kitty
                .compose_frames(&command)
                .map(|_| command.image_id),
        }
    }

    fn place(
        &mut self,
        id: u32,
        internal: crate::panes::kitty_graphics::store::InternalImageId,
        command: &KittyCommand,
    ) -> Result<Option<(u16, u16)>, KittyError> {
        self.kitty.place(
            id,
            internal,
            command,
            self.cursor_px,
            CELL,
            KittyVerticalAnchor {
                canonical_line: 0,
                offset_px_from_line_start: self.cursor_px.1 as isize,
            },
        )
    }

    fn ok(&mut self, control: &str, payload: &[u8]) -> Option<u32> {
        self.run(control, payload)
            .unwrap_or_else(|e| panic!("expected {:?} to be accepted, got {:?}", control, e))
    }

    fn err(&mut self, control: &str, payload: &[u8]) -> KittyError {
        self.run(control, payload)
            .err()
            .unwrap_or_else(|| panic!("expected {:?} to be rejected", control))
    }

    fn at_cell(&mut self, column: usize, row: usize) -> &mut Self {
        self.cursor_cell = (column, row);
        self.cursor_px = (column * CELL.width, row * CELL.height);
        self
    }

    fn placement_count(&self) -> usize {
        self.kitty.placement_count()
    }

    fn image_count(&self) -> usize {
        self.store.borrow().image_count()
    }

    /// The (image id, placement id) pairs currently on screen, sorted for stable comparison.
    fn placement_ids(&self) -> Vec<(u32, u32)> {
        let mut ids: Vec<(u32, u32)> = self
            .kitty
            .placements()
            .iter()
            .map(|placement| (placement.image_id, placement.placement_id))
            .collect();
        ids.sort();
        ids
    }

    /// Whether the image is still displayable, which is how a client observes the difference
    /// between a lowercase and an uppercase delete.
    ///
    /// The probe is a real `a=p`, so a successful one **leaves a placement behind**. Call it after
    /// any assertion on placement counts, not before.
    fn can_still_display_by_placing_it(&mut self, image_id: u32) -> bool {
        self.run(&format!("a=p,i={},C=1", image_id), &[]).is_ok()
    }
}

/// `width * height` RGB bytes, the payload for `f=24,s=width,v=height`.
fn rgb(width: usize, height: usize) -> Vec<u8> {
    rgb_block(width, height)
}

// ---- transmission, replacement and anonymous images ----

#[test]
fn a_transmitted_image_can_be_displayed_later_by_id() {
    // Spec: `a=t` stores the image, `a=p` displays a previously transmitted one.
    let mut model = Model::new();
    model.ok("a=t,f=24,s=10,v=20,i=1", &rgb(10, 20));
    assert_eq!(model.placement_count(), 0, "a=t must not display anything");

    model.ok("a=p,i=1", &[]);
    assert_eq!(model.placement_count(), 1);
}

#[test]
fn displaying_an_unknown_image_id_reports_that_it_does_not_exist() {
    let mut model = Model::new();
    assert_eq!(model.err("a=p,i=404", &[]).code, KittyErrorCode::Enoent);
}

#[test]
fn retransmitting_to_an_existing_id_replaces_the_image_and_drops_its_placements() {
    // Spec: "When re-transmitting image data for a specific id, the existing image and all its
    // placements must be deleted."
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    model.at_cell(5, 0).ok("a=p,i=1,p=2", &[]);
    assert_eq!(model.placement_count(), 2);

    model.ok("a=t,f=24,s=20,v=40,i=1", &rgb(20, 40));
    assert_eq!(
        model.placement_count(),
        0,
        "the old image's placements go with it"
    );
    assert_eq!(model.image_count(), 1, "and the old raster is released");
}

// The two positive query rules — that `a=q` stores nothing and does not replace an image with
// the same id — cannot be asserted at this level: `Model::run` answers a query itself rather than
// handing it to `KittyGrid`, so a test here would only be checking the model. They are asserted
// against the real command path in `grid_tests::kitty_spec::a_query_leaves_no_image_behind` and
// `::a_query_does_not_replace_an_image_that_already_has_the_same_id`.
//
// The failing case below *is* meaningful here, because the error comes out of the parser.

#[test]
fn a_failing_query_reports_the_error_without_disturbing_stored_images() {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));

    assert_eq!(
        model.err("a=q,f=24,s=100,v=100,i=1", &rgb(2, 2)).code,
        KittyErrorCode::Enodata
    );
    assert!(model.can_still_display_by_placing_it(1));
}

#[test]
fn images_transmitted_without_an_id_coexist_instead_of_replacing_each_other() {
    // Spec: an image id of zero means "no id", and "there can exist multiple images with
    // `image id=0, placement id=0`".
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20", &rgb(10, 20));
    model.at_cell(5, 0).ok("a=T,f=24,s=10,v=20", &rgb(10, 20));

    assert_eq!(model.placement_count(), 2);
    assert_eq!(
        model.image_count(),
        2,
        "neither transmission replaced the other"
    );
}

#[test]
fn a_placement_id_is_ignored_for_an_image_with_no_id() {
    // Spec: "If you specify a placement id for an image that does not have an id (i.e. has id=0),
    // it will be ignored, i.e. the placement will not get an id." A placement is only addressable
    // through the pair.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,p=7", &rgb(10, 20));
    assert_eq!(model.placement_ids(), vec![(0, 0)]);
}

// ---- image numbers ----

#[test]
fn each_transmission_with_an_image_number_creates_a_distinct_image() {
    // Spec: image numbers are "not unique" — each transmission makes a new image and the
    // terminal reports the id it assigned.
    let mut model = Model::new();
    let first = model.ok("a=t,f=24,s=10,v=20,I=1", &rgb(10, 20)).unwrap();
    let second = model.ok("a=t,f=24,s=10,v=20,I=1", &rgb(10, 20)).unwrap();

    assert_ne!(first, second, "a second transmission is a second image");
    assert_ne!(first, 0);
    assert_ne!(second, 0);
    assert_eq!(model.image_count(), 2);
}

#[test]
fn displaying_by_image_number_picks_the_most_recent_transmission() {
    // Spec: commands that refer to images by number "will act on only the newest image with that
    // number".
    let mut model = Model::new();
    model.ok("a=t,f=24,s=10,v=20,I=7", &rgb(10, 20));
    let newest = model.ok("a=t,f=24,s=20,v=40,I=7", &rgb(20, 40)).unwrap();

    model.ok("a=p,I=7", &[]);
    assert_eq!(model.placement_ids(), vec![(newest, 0)]);
}

#[test]
fn displaying_an_unknown_image_number_reports_that_it_does_not_exist() {
    let mut model = Model::new();
    assert_eq!(model.err("a=p,I=99", &[]).code, KittyErrorCode::Enoent);
}

#[test]
fn an_image_transmitted_by_number_is_still_addressable_by_its_assigned_id() {
    let mut model = Model::new();
    let assigned = model.ok("a=t,f=24,s=10,v=20,I=4", &rgb(10, 20)).unwrap();
    model.ok(&format!("a=p,i={}", assigned), &[]);
    assert_eq!(model.placement_ids(), vec![(assigned, 0)]);
}

// ---- placement geometry ----

#[test]
fn a_placement_with_no_geometry_keys_covers_the_whole_image() {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=25,v=45,i=1", &rgb(25, 45));

    let placement = &model.kitty.placements()[0];
    assert_eq!(
        (placement.source_rect.width, placement.source_rect.height),
        (25, 45)
    );
    // 25x45 pixels over 10x20 cells needs 3 columns and 3 rows to cover it.
    assert_eq!(placement.dest_cells, (3, 3));
}

#[test]
fn the_source_rectangle_selects_a_region_of_the_image() {
    // Spec: the keys "`x, y, w, h` [...] specify the top-left corner, width and height of the
    // source rectangle", in pixels.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,x=10,y=5,w=20,h=10", &rgb(40, 40));

    let source = model.kitty.placements()[0].source_rect;
    assert_eq!((source.x, source.y), (10, 5));
    assert_eq!((source.width, source.height), (20, 10));
}

#[test]
fn a_source_rectangle_larger_than_the_image_is_clipped_to_it() {
    // Spec: "The displayed area is the intersection of the specified rectangle with the source
    // image rectangle."
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,x=30,y=30,w=999,h=999", &rgb(40, 40));

    let source = model.kitty.placements()[0].source_rect;
    assert_eq!((source.width, source.height), (10, 10));
}

#[test]
fn a_source_rectangle_starting_outside_the_image_is_rejected() {
    let mut model = Model::new();
    model.ok("a=t,f=24,s=40,v=40,i=1", &rgb(40, 40));
    assert_eq!(model.err("a=p,i=1,x=40", &[]).code, KittyErrorCode::Einval);
    assert_eq!(model.err("a=p,i=1,y=40", &[]).code, KittyErrorCode::Einval);
}

#[test]
fn scaling_to_a_column_and_row_count_produces_exactly_that_many_cells() {
    // Spec: "`c` is the number of columns and `r` the number of rows. The image will be scaled
    // (enlarged/shrunk) as needed to fit the specified area."
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,c=3,r=2", &rgb(40, 40));

    let placement = &model.kitty.placements()[0];
    assert_eq!(placement.dest_cells, (3, 2));
    assert_eq!(
        (placement.display_rect.width, placement.display_rect.height),
        (3 * CELL.width, 2 * CELL.height)
    );
}

#[test]
fn scaling_by_columns_alone_derives_the_rows_from_the_aspect_ratio() {
    // Spec: "If only one of either `r` or `c` is specified, the other one is computed based on the
    // source image aspect ratio, so that the image is displayed without distortion." A 40x40 image
    // over 3 columns is 30px wide, so it is 30px tall: two 20px rows.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,c=3", &rgb(40, 40));

    let placement = &model.kitty.placements()[0];
    assert_eq!(placement.dest_cells, (3, 2));
    assert_eq!(placement.display_rect.height, 30);
}

#[test]
fn scaling_by_rows_alone_derives_the_columns_from_the_aspect_ratio() {
    // A 40x40 image over 2 rows is 40px tall, so 40px wide: four 10px columns.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,r=2", &rgb(40, 40));

    let placement = &model.kitty.placements()[0];
    assert_eq!(placement.dest_cells, (4, 2));
    assert_eq!(placement.display_rect.width, 40);
}

#[test]
fn scaling_by_columns_ignores_the_horizontal_cell_offset_when_deriving_rows() {
    // Spec: "Note that if you specify a start cell offset via the `X,Y` keys, it is not added to
    // the number of rows/columns." A square 40x40 image scaled to 4 columns is 40px wide, and by
    // aspect ratio 40px tall, which is exactly two 20px rows — a start offset within the first
    // cell does not change how many rows the image needs.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,c=4", &rgb(40, 40));
    assert_eq!(
        model.kitty.placements()[0].dest_cells,
        (4, 2),
        "sanity: without an offset the derived row count is two"
    );

    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,c=4,X=5", &rgb(40, 40));
    assert_eq!(
        model.kitty.placements()[0].dest_cells,
        (4, 2),
        "the X offset must not be added to the derived row count"
    );
}

#[test]
fn scaling_by_rows_ignores_the_vertical_cell_offset_when_deriving_columns() {
    // The mirror image: 40x40 scaled to 2 rows is 40px tall and 40px wide, four 10px columns.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,r=2", &rgb(40, 40));
    assert_eq!(
        model.kitty.placements()[0].dest_cells,
        (4, 2),
        "sanity: without an offset the derived column count is four"
    );

    let mut model = Model::new();
    model.ok("a=T,f=24,s=40,v=40,i=1,r=2,Y=5", &rgb(40, 40));
    assert_eq!(
        model.kitty.placements()[0].dest_cells,
        (4, 2),
        "the Y offset must not be added to the derived column count"
    );
}

#[test]
fn the_cell_offsets_shift_the_image_within_its_first_cell() {
    // Spec: `X` and `Y` are "the x-offset/y-offset within the first cell at which to start
    // displaying the image", in pixels.
    let mut model = Model::new();
    model
        .at_cell(2, 1)
        .ok("a=T,f=24,s=10,v=20,i=1,X=3,Y=4", &rgb(10, 20));

    let placement = &model.kitty.placements()[0];
    assert_eq!(placement.cell_offset, (3, 4));
    assert_eq!(placement.display_rect.x, 2 * CELL.width + 3);
    assert_eq!(placement.display_rect.y, (CELL.height + 4) as isize);
}

#[test]
fn an_unscaled_image_pushed_by_a_cell_offset_covers_an_extra_cell() {
    // A 10x20 image exactly fills one cell; offsetting it by a pixel spills into the next.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1,X=5,Y=5", &rgb(10, 20));
    assert_eq!(model.kitty.placements()[0].dest_cells, (2, 2));
}

#[test]
fn re_placing_the_same_image_and_placement_id_moves_it_rather_than_duplicating_it() {
    // Spec: re-sending a placement with the same (image id, placement id) replaces it, which is
    // how clients move an image without it flickering.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1,p=4", &rgb(10, 20));
    model.at_cell(6, 3).ok("a=p,i=1,p=4", &[]);

    assert_eq!(model.placement_count(), 1);
    let placement = &model.kitty.placements()[0];
    assert_eq!(placement.display_rect.x, 6 * CELL.width);
    assert_eq!(placement.display_rect.y, (3 * CELL.height) as isize);
}

#[test]
fn placements_with_different_placement_ids_coexist() {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1,p=1", &rgb(10, 20));
    model.at_cell(4, 0).ok("a=p,i=1,p=2", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (1, 2)]);
}

#[test]
fn the_z_index_is_recorded_on_the_placement() {
    // Negative values draw the image under the text, which the renderer needs to know.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1,z=-5", &rgb(10, 20));
    assert_eq!(model.kitty.placements()[0].z_index, -5);
}

// ---- deletion ----

/// Three placements: image 1 at the origin, image 2 at cell (5,2), image 3 at cell (10,4).
fn model_with_three_placements() -> Model {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1,p=1,z=0", &rgb(10, 20));
    model
        .at_cell(5, 2)
        .ok("a=T,f=24,s=10,v=20,i=2,p=1,z=3", &rgb(10, 20));
    model
        .at_cell(10, 4)
        .ok("a=T,f=24,s=10,v=20,i=3,p=1,z=-3", &rgb(10, 20));
    assert_eq!(model.placement_count(), 3);
    model
}

#[test]
fn deleting_everything_removes_all_visible_placements() {
    // Spec, d=a: "delete all placements visible on screen".
    let mut model = model_with_three_placements();
    model.ok("a=d,d=a", &[]);
    assert_eq!(model.placement_count(), 0);
}

#[test]
fn the_default_delete_specifier_is_all_visible_placements() {
    // Spec: d defaults to 'a'.
    let mut model = model_with_three_placements();
    model.ok("a=d", &[]);
    assert_eq!(model.placement_count(), 0);
}

#[test]
fn deleting_by_image_id_removes_only_that_images_placements() {
    let mut model = model_with_three_placements();
    model.ok("a=d,d=i,i=2", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (3, 1)]);
}

#[test]
fn deleting_by_image_id_and_placement_id_ignores_other_placements_of_that_image() {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1,p=1", &rgb(10, 20));
    model.at_cell(4, 0).ok("a=p,i=1,p=2", &[]);

    model.ok("a=d,d=i,i=1,p=2", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1)]);
}

#[test]
fn deleting_by_image_id_with_a_placement_id_that_matches_nothing_is_a_no_op() {
    let mut model = model_with_three_placements();
    model.ok("a=d,d=i,i=1,p=99", &[]);
    assert_eq!(model.placement_count(), 3);
}

#[test]
fn deleting_by_image_number_targets_the_newest_image_with_that_number() {
    // Spec, d=n: "delete newest image with the specified number".
    let mut model = Model::new();
    let older = model.ok("a=T,f=24,s=10,v=20,I=5", &rgb(10, 20)).unwrap();
    let newer = model
        .at_cell(4, 0)
        .ok("a=T,f=24,s=10,v=20,I=5", &rgb(10, 20))
        .unwrap();

    model.ok("a=d,d=n,I=5", &[]);
    assert_eq!(model.placement_ids(), vec![(older, 0)]);
    assert_ne!(older, newer);
}

#[test]
fn deleting_by_an_unknown_image_number_is_a_silent_no_op() {
    let mut model = model_with_three_placements();
    model.ok("a=d,d=n,I=77", &[]);
    assert_eq!(model.placement_count(), 3);
}

#[test]
fn deleting_at_the_cursor_removes_only_placements_under_it() {
    // Spec, d=c: "delete all placements that intersect with the current cursor position".
    let mut model = model_with_three_placements();
    model.at_cell(5, 2).ok("a=d,d=c", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (3, 1)]);
}

#[test]
fn deleting_at_a_named_cell_uses_one_based_coordinates() {
    // Spec, d=p: "delete all placements that intersect the specified cell", with x and y
    // one-based, matching the cursor-positioning convention.
    let mut model = model_with_three_placements();
    model.ok("a=d,d=p,x=6,y=3", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (3, 1)]);
}

#[test]
fn deleting_at_a_named_cell_with_a_z_index_requires_both_to_match() {
    // Spec, d=q: the cell and the z-index.
    let mut model = model_with_three_placements();
    model.ok("a=d,d=q,x=6,y=3,z=999", &[]);
    assert_eq!(model.placement_count(), 3, "the z-index did not match");

    model.ok("a=d,d=q,x=6,y=3,z=3", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (3, 1)]);
}

#[test]
fn deleting_a_column_removes_every_placement_crossing_it() {
    // Spec, d=x: "delete all placements that intersect the specified column".
    let mut model = model_with_three_placements();
    model.ok("a=d,d=x,x=6", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (3, 1)]);
}

#[test]
fn deleting_a_row_removes_every_placement_crossing_it() {
    // Spec, d=y.
    let mut model = model_with_three_placements();
    model.ok("a=d,d=y,y=3", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (3, 1)]);
}

#[test]
fn deleting_by_z_index_removes_every_placement_at_that_depth() {
    // Spec, d=z.
    let mut model = model_with_three_placements();
    model.ok("a=d,d=z,z=-3", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1), (2, 1)]);
}

#[test]
fn deleting_an_image_id_range_covers_both_endpoints() {
    // Spec, d=r: "delete all images with ids in the range x to y, inclusive".
    let mut model = model_with_three_placements();
    model.ok("a=d,d=r,x=2,y=3", &[]);
    assert_eq!(model.placement_ids(), vec![(1, 1)]);
}

#[test]
fn an_image_id_range_with_no_lower_bound_starts_at_the_first_image() {
    let mut model = model_with_three_placements();
    model.ok("a=d,d=r,y=2", &[]);
    assert_eq!(model.placement_ids(), vec![(3, 1)]);
}

// ---- the lowercase/uppercase data lifetime rule ----

#[test]
fn a_lowercase_delete_keeps_the_image_so_it_can_be_displayed_again() {
    // Spec: "The lowercase variant only deletes the images without necessarily freeing up the
    // stored image data, so that the images can be re-displayed without needing to resend the
    // data. The uppercase variants will delete the image data as well." This is what lets a client
    // hide and re-show an image without re-uploading it.
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    model.ok("a=d,d=i,i=1", &[]);

    assert_eq!(model.placement_count(), 0);
    assert!(
        model.can_still_display_by_placing_it(1),
        "the image data must survive"
    );
}

#[test]
fn an_uppercase_delete_frees_the_image_so_it_can_no_longer_be_displayed() {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    model.ok("a=d,d=I,i=1", &[]);

    assert_eq!(
        model.err("a=p,i=1", &[]).code,
        KittyErrorCode::Enoent,
        "the image data is gone"
    );
    assert_eq!(model.image_count(), 0);
}

#[test]
fn the_lowercase_and_uppercase_rule_holds_for_image_numbers() {
    let mut model = Model::new();
    let id = model.ok("a=T,f=24,s=10,v=20,I=5", &rgb(10, 20)).unwrap();
    model.ok("a=d,d=n,I=5", &[]);
    assert!(model.can_still_display_by_placing_it(id));

    model.ok("a=d,d=N,I=5", &[]);
    assert_eq!(
        model.err(&format!("a=p,i={}", id), &[]).code,
        KittyErrorCode::Enoent
    );
}

#[test]
fn the_lowercase_and_uppercase_rule_holds_for_the_delete_everything_specifier() {
    let mut model = Model::new();
    model.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    model.ok("a=d,d=a", &[]);
    assert!(model.can_still_display_by_placing_it(1));

    model.ok("a=d,d=A", &[]);
    assert_eq!(model.err("a=p,i=1", &[]).code, KittyErrorCode::Enoent);
}

#[test]
fn the_lowercase_and_uppercase_rule_holds_for_id_ranges() {
    let mut model = model_with_three_placements();
    model.ok("a=d,d=r,x=1,y=3", &[]);
    assert!(model.can_still_display_by_placing_it(2));

    model.ok("a=d,d=a", &[]);
    model.ok("a=d,d=R,x=1,y=3", &[]);
    assert_eq!(model.err("a=p,i=2", &[]).code, KittyErrorCode::Enoent);
}

#[test]
fn deleting_placements_of_one_image_leaves_other_images_alone() {
    let mut model = model_with_three_placements();
    model.ok("a=d,d=I,i=2", &[]);

    assert!(model.can_still_display_by_placing_it(1));
    assert!(model.can_still_display_by_placing_it(3));
}
