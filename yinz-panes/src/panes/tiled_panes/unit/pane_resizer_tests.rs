use super::pane_resizer_test_mock::{
    dim_fixed, dim_pct, run_layout, spec, spec_stacked, with_override, Outcome, PaneSnapshot,
    PaneSpec, Run,
};
use crate::panes::PaneId;
use yinz_types::geometry::SplitDirection;
use yinz_types::pane_size::{Constraint, PaneGeom};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    A,
    B,
    Artefact,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
    Cols,
    Rows,
}

struct Fixture {
    name: &'static str,
    specs: Vec<PaneSpec>,
    direction: SplitDirection,
    space: usize,
    class: Class,
    bands: Vec<Vec<PaneId>>,
}

impl Fixture {
    fn axis(&self) -> Axis {
        match self.direction {
            SplitDirection::Horizontal => Axis::Cols,
            SplitDirection::Vertical => Axis::Rows,
        }
    }
    fn run(&self) -> Run {
        run_layout(&self.specs, self.direction, self.space)
    }
    fn has_stacked(&self) -> bool {
        self.specs.iter().any(|s| s.stacked.is_some())
    }
}

fn tid(id: u32) -> PaneId {
    PaneId::Terminal(crate::TerminalId::new(id))
}

fn fixtures() -> Vec<Fixture> {
    let h = SplitDirection::Horizontal;
    let v = SplitDirection::Vertical;
    vec![
        Fixture {
            name: "single_pane_grows_to_space",
            specs: vec![spec(1, 0, 0, dim_pct(100.0, 40), dim_pct(100.0, 10))],
            direction: h,
            space: 80,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "empty_pane_map_reports_unchanged",
            specs: vec![],
            direction: h,
            space: 80,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "two_panes_split_available_space",
            specs: vec![
                spec(1, 0, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
                spec(2, 40, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "identical_sizes_report_unchanged",
            specs: vec![
                spec(1, 0, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
                spec(2, 40, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 80,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "unchanged_sizes_still_reposition",
            specs: vec![
                spec(1, 0, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
                spec(2, 45, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 80,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "vertical_split_resizes_rows",
            specs: vec![
                spec(1, 0, 0, dim_pct(100.0, 80), dim_pct(50.0, 10)),
                spec(2, 0, 10, dim_pct(100.0, 80), dim_pct(50.0, 10)),
            ],
            direction: v,
            space: 30,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "three_panes_round_up_first",
            specs: vec![
                spec(1, 0, 0, dim_pct(33.3, 26), dim_pct(100.0, 10)),
                spec(2, 26, 0, dim_pct(33.3, 27), dim_pct(100.0, 10)),
                spec(3, 53, 0, dim_pct(33.4, 27), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "over_rounded_band_shrinks_last",
            specs: vec![
                spec(1, 0, 0, dim_pct(33.4999, 33), dim_pct(100.0, 10)),
                spec(2, 33, 0, dim_pct(33.4999, 33), dim_pct(100.0, 10)),
                spec(3, 66, 0, dim_pct(33.0002, 34), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "fixed_pane_locks_and_percent_absorbs",
            specs: vec![
                spec(1, 0, 0, dim_fixed(20), dim_pct(100.0, 10)),
                spec(2, 20, 0, dim_pct(100.0, 60), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "fixed_pane_excluded_from_rounding",
            specs: vec![
                spec(1, 0, 0, dim_fixed(21), dim_pct(100.0, 10)),
                spec(2, 21, 0, dim_pct(50.0, 30), dim_pct(100.0, 10)),
                spec(3, 51, 0, dim_pct(50.0, 29), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "pane_spanning_two_bands_adjusted_once",
            specs: vec![
                spec(1, 0, 0, dim_pct(33.3, 26), dim_pct(50.0, 10)),
                spec(3, 26, 0, dim_pct(66.7, 54), dim_pct(100.0, 20)),
                spec(4, 0, 10, dim_pct(16.65, 13), dim_pct(50.0, 10)),
                spec(5, 13, 10, dim_pct(16.65, 13), dim_pct(50.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "stack_main_pane_carries_whole_stack",
            specs: vec![
                spec_stacked(1, 0, 0, dim_pct(50.0, 40), dim_fixed(1), 0),
                spec_stacked(2, 0, 1, dim_pct(50.0, 40), dim_pct(50.0, 8), 0),
                spec_stacked(3, 0, 9, dim_pct(50.0, 40), dim_fixed(1), 0),
                spec(4, 40, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
            ],
            direction: v,
            space: 12,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "short_stack_is_rejected_without_mutation",
            specs: vec![
                spec_stacked(1, 0, 0, dim_pct(100.0, 80), dim_fixed(1), 0),
                spec_stacked(2, 0, 1, dim_pct(100.0, 80), dim_pct(100.0, 8), 0),
                spec_stacked(3, 0, 9, dim_pct(100.0, 80), dim_fixed(1), 0),
            ],
            direction: v,
            space: 2,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "override_pane_updates_override_only",
            specs: {
                let one = spec(1, 0, 0, dim_pct(50.0, 40), dim_pct(100.0, 10));
                let two = spec(2, 40, 0, dim_pct(50.0, 40), dim_pct(100.0, 10));
                vec![one, with_override(two, two.geom())]
            },
            direction: h,
            space: 100,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "no_room_for_all_spans",
            specs: vec![
                spec(1, 0, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
                spec(2, 40, 0, dim_pct(50.0, 40), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 1,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "conflicting_required_constraints_fail",
            specs: vec![spec(1, 0, 0, dim_fixed(10), dim_pct(100.0, 10))],
            direction: h,
            space: 20,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "stack_without_flexible_pane_is_ignored",
            specs: vec![
                spec_stacked(1, 0, 0, dim_pct(100.0, 80), dim_fixed(1), 0),
                spec_stacked(2, 0, 1, dim_pct(100.0, 80), dim_fixed(1), 0),
            ],
            direction: v,
            space: 20,
            class: Class::A,
            bands: vec![],
        },
        Fixture {
            name: "under_specified_percents",
            specs: vec![
                spec(1, 0, 0, dim_pct(30.0, 40), dim_pct(100.0, 10)),
                spec(2, 40, 0, dim_pct(30.0, 40), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::B,
            bands: vec![vec![tid(1), tid(2)]],
        },
        Fixture {
            name: "over_specified_percents",
            specs: vec![
                spec(1, 0, 0, dim_pct(60.0, 40), dim_pct(100.0, 10)),
                spec(2, 40, 0, dim_pct(60.0, 40), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::B,
            bands: vec![vec![tid(1), tid(2)]],
        },
        Fixture {
            name: "degenerate_across_two_bands",
            specs: vec![
                spec(1, 0, 0, dim_pct(60.0, 26), dim_pct(50.0, 10)),
                spec(3, 26, 0, dim_pct(60.0, 54), dim_pct(100.0, 20)),
                spec(4, 0, 10, dim_pct(30.0, 13), dim_pct(50.0, 10)),
                spec(5, 13, 10, dim_pct(30.0, 13), dim_pct(50.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::B,
            bands: vec![vec![tid(1), tid(3)], vec![tid(4), tid(5), tid(3)]],
        },
        Fixture {
            name: "fixed_and_degenerate_percents",
            specs: vec![
                spec(1, 0, 0, dim_fixed(20), dim_pct(100.0, 10)),
                spec(2, 20, 0, dim_pct(60.0, 30), dim_pct(100.0, 10)),
                spec(3, 50, 0, dim_pct(60.0, 30), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 100,
            class: Class::B,
            bands: vec![vec![tid(1), tid(2), tid(3)]],
        },
        Fixture {
            name: "zero_flex_space_division",
            specs: vec![
                spec(1, 0, 0, dim_fixed(20), dim_pct(100.0, 10)),
                spec(2, 20, 0, dim_pct(50.0, 20), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 20,
            class: Class::Artefact,
            bands: vec![],
        },
        Fixture {
            name: "saturated_flex_space_division",
            specs: vec![
                spec(1, 0, 0, dim_fixed(30), dim_pct(100.0, 10)),
                spec(2, 30, 0, dim_pct(50.0, 20), dim_pct(100.0, 10)),
            ],
            direction: h,
            space: 20,
            class: Class::Artefact,
            bands: vec![],
        },
        Fixture {
            name: "stacked_pane_with_override",
            specs: {
                let two = spec_stacked(2, 0, 1, dim_pct(100.0, 80), dim_pct(100.0, 8), 0);
                vec![
                    spec_stacked(1, 0, 0, dim_pct(100.0, 80), dim_fixed(1), 0),
                    with_override(two, two.geom()),
                    spec_stacked(3, 0, 9, dim_pct(100.0, 80), dim_fixed(1), 0),
                ]
            },
            direction: SplitDirection::Vertical,
            space: 20,
            class: Class::Artefact,
            bands: vec![],
        },
    ]
}

fn fixture(name: &str) -> Fixture {
    fixtures()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("unknown fixture `{}`", name))
}

fn pane(run: &Run, id: u32) -> PaneSnapshot {
    *run.panes
        .iter()
        .find(|p| p.id == PaneId::Terminal(crate::TerminalId::new(id)))
        .unwrap_or_else(|| panic!("no snapshot for pane {}", id))
}

fn input_spec(specs: &[PaneSpec], id: PaneId) -> PaneSpec {
    *specs
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no input spec for {:?}", id))
}

fn assert_ok(run: &Run, name: &str) {
    assert_eq!(run.outcome, Outcome::Ok, "fixture `{}`", name);
}

fn assert_geom_field_eq(actual: &PaneGeom, expected: &PaneGeom, ctx: &str) {
    assert_eq!(actual.x, expected.x, "{}: x", ctx);
    assert_eq!(actual.y, expected.y, "{}: y", ctx);
    assert_eq!(
        actual.cols.constraint, expected.cols.constraint,
        "{}: cols.constraint",
        ctx
    );
    assert_eq!(
        actual.cols.as_usize(),
        expected.cols.as_usize(),
        "{}: cols.inner",
        ctx
    );
    assert_eq!(
        actual.rows.constraint, expected.rows.constraint,
        "{}: rows.constraint",
        ctx
    );
    assert_eq!(
        actual.rows.as_usize(),
        expected.rows.as_usize(),
        "{}: rows.inner",
        ctx
    );
    assert_eq!(actual.stacked, expected.stacked, "{}: stacked", ctx);
    assert_eq!(actual.is_pinned, expected.is_pinned, "{}: is_pinned", ctx);
    assert_eq!(
        actual.logical_position, expected.logical_position,
        "{}: logical_position",
        ctx
    );
}

fn axis_dim(geom: &PaneGeom, axis: Axis) -> (usize, usize, Constraint) {
    match axis {
        Axis::Cols => (geom.x, geom.cols.as_usize(), geom.cols.constraint),
        Axis::Rows => (geom.y, geom.rows.as_usize(), geom.rows.constraint),
    }
}

fn assert_result_is_benign(run: &Run, name: &str) {
    match &run.outcome {
        Outcome::Ok | Outcome::PaneSizeUnchanged => {},
        Outcome::LayoutRejected => panic!("fixture `{}` was rejected", name),
        Outcome::OtherErr(msg) => panic!("fixture `{}` produced an error: {}", name, msg),
    }
}

fn assert_constraints_preserved(run: &Run, input: &[PaneSpec], axis: Axis, name: &str) {
    for snapshot in &run.panes {
        let expected = input_spec(input, snapshot.id).geom();
        let (_, _, actual_constraint) = axis_dim(&snapshot.current, axis);
        let (_, _, expected_constraint) = axis_dim(&expected, axis);
        assert_eq!(
            actual_constraint, expected_constraint,
            "fixture `{}`: {:?} constraint on {:?}",
            name, snapshot.id, axis
        );
    }
}

fn assert_cross_axis_untouched(run: &Run, input: &[PaneSpec], axis: Axis, name: &str) {
    for snapshot in &run.panes {
        let expected = input_spec(input, snapshot.id).geom();
        match axis {
            Axis::Cols => {
                assert_eq!(
                    snapshot.current.y, expected.y,
                    "fixture `{}`: {:?} y",
                    name, snapshot.id
                );
                assert_eq!(
                    snapshot.current.rows.constraint, expected.rows.constraint,
                    "fixture `{}`: {:?} rows.constraint",
                    name, snapshot.id
                );
                assert_eq!(
                    snapshot.current.rows.as_usize(),
                    expected.rows.as_usize(),
                    "fixture `{}`: {:?} rows.inner",
                    name,
                    snapshot.id
                );
            },
            Axis::Rows => {
                assert_eq!(
                    snapshot.current.x, expected.x,
                    "fixture `{}`: {:?} x",
                    name, snapshot.id
                );
                assert_eq!(
                    snapshot.current.cols.constraint, expected.cols.constraint,
                    "fixture `{}`: {:?} cols.constraint",
                    name, snapshot.id
                );
                assert_eq!(
                    snapshot.current.cols.as_usize(),
                    expected.cols.as_usize(),
                    "fixture `{}`: {:?} cols.inner",
                    name,
                    snapshot.id
                );
            },
        }
    }
}

fn assert_metadata_preserved(run: &Run, input: &[PaneSpec], name: &str) {
    for snapshot in &run.panes {
        let expected = input_spec(input, snapshot.id).geom();
        assert_eq!(
            snapshot.current.stacked, expected.stacked,
            "fixture `{}`: {:?} stacked",
            name, snapshot.id
        );
        assert_eq!(
            snapshot.current.logical_position, expected.logical_position,
            "fixture `{}`: {:?} logical_position",
            name, snapshot.id
        );
    }
}

fn assert_min_size(run: &Run, axis: Axis, name: &str) {
    for snapshot in &run.panes {
        let (_, size, _) = axis_dim(&snapshot.current, axis);
        assert!(
            size >= 1,
            "fixture `{}`: {:?} has {:?} size {}",
            name,
            snapshot.id,
            axis,
            size
        );
    }
}

fn assert_fixed_panes_locked(run: &Run, input: &[PaneSpec], axis: Axis, name: &str) {
    for snapshot in &run.panes {
        let expected = input_spec(input, snapshot.id).geom();
        if let (_, _, Constraint::Fixed(n)) = axis_dim(&expected, axis) {
            let (_, size, _) = axis_dim(&snapshot.current, axis);
            assert_eq!(
                size, n,
                "fixture `{}`: {:?} fixed pane size drifted",
                name, snapshot.id
            );
        }
    }
}

fn assert_band_partitions_space(run: &Run, band: &[PaneId], axis: Axis, space: usize, name: &str) {
    let mut members: Vec<(usize, usize)> = band
        .iter()
        .map(|id| {
            let snapshot = run
                .panes
                .iter()
                .find(|p| p.id == *id)
                .unwrap_or_else(|| panic!("fixture `{}`: no snapshot for {:?}", name, id));
            let (pos, size, _) = axis_dim(&snapshot.current, axis);
            (pos, size)
        })
        .collect();
    members.sort_by_key(|(pos, _)| *pos);
    assert!(
        !members.is_empty(),
        "fixture `{}`: empty band supplied",
        name
    );
    assert_eq!(
        members[0].0, 0,
        "fixture `{}`: band {:?} does not start at 0",
        name, band
    );
    for window in members.windows(2) {
        assert_eq!(
            window[1].0,
            window[0].0 + window[0].1,
            "fixture `{}`: band {:?} has a gap or overlap",
            name,
            band
        );
    }
    let last = members[members.len() - 1];
    assert_eq!(
        last.0 + last.1,
        space,
        "fixture `{}`: band {:?} does not fill {}",
        name,
        band,
        space
    );
}

fn assert_no_mutation(run: &Run, input: &[PaneSpec], name: &str) {
    for snapshot in &run.panes {
        let spec = input_spec(input, snapshot.id);
        assert_geom_field_eq(
            &snapshot.geom,
            &spec.geom(),
            &format!("fixture `{}`: {:?} geom", name, snapshot.id),
        );
        let expected_current = spec.geom_override.unwrap_or_else(|| spec.geom());
        assert_geom_field_eq(
            &snapshot.current,
            &expected_current,
            &format!("fixture `{}`: {:?} current", name, snapshot.id),
        );
        match (snapshot.geom_override, spec.geom_override) {
            (None, None) => {},
            (Some(actual), Some(expected)) => assert_geom_field_eq(
                &actual,
                &expected,
                &format!("fixture `{}`: {:?} geom_override", name, snapshot.id),
            ),
            (actual, expected) => panic!(
                "fixture `{}`: {:?} geom_override {:?} != {:?}",
                name, snapshot.id, actual, expected
            ),
        }
    }
}

fn assert_class_b_invariants(fixture: &Fixture, run: &Run) {
    assert_eq!(
        fixture.class,
        Class::B,
        "{}: only class-B fixtures are held to the class-B invariants",
        fixture.name
    );
    let axis = fixture.axis();
    assert_result_is_benign(run, fixture.name);
    assert_constraints_preserved(run, &fixture.specs, axis, fixture.name);
    if !fixture.has_stacked() {
        assert_cross_axis_untouched(run, &fixture.specs, axis, fixture.name);
    }
    assert_metadata_preserved(run, &fixture.specs, fixture.name);
    assert_min_size(run, axis, fixture.name);
    assert_fixed_panes_locked(run, &fixture.specs, axis, fixture.name);
    for band in &fixture.bands {
        assert_band_partitions_space(run, band, axis, fixture.space, fixture.name);
    }
}

fn canonical(run: &Run) -> String {
    let outcome = match &run.outcome {
        Outcome::Ok => "ok".to_string(),
        Outcome::PaneSizeUnchanged => "unchanged".to_string(),
        Outcome::LayoutRejected => "rejected".to_string(),
        Outcome::OtherErr(msg) => format!("err({})", msg),
    };
    let mut out = outcome;
    for snapshot in &run.panes {
        out.push_str(&format!(
            " | {:?} x={} y={} cols={:?}:{} rows={:?}:{} stacked={:?} logical={:?} override={}",
            snapshot.id,
            snapshot.geom.x,
            snapshot.geom.y,
            snapshot.geom.cols.constraint,
            snapshot.geom.cols.as_usize(),
            snapshot.geom.rows.constraint,
            snapshot.geom.rows.as_usize(),
            snapshot.geom.stacked,
            snapshot.geom.logical_position,
            match &snapshot.geom_override {
                None => "none".to_string(),
                Some(g) => format!(
                    "x={} y={} cols={:?}:{} rows={:?}:{} stacked={:?} logical={:?}",
                    g.x,
                    g.y,
                    g.cols.constraint,
                    g.cols.as_usize(),
                    g.rows.constraint,
                    g.rows.as_usize(),
                    g.stacked,
                    g.logical_position
                ),
            }
        ));
    }
    out
}

#[test]
fn single_pane_grows_to_space() {
    let f = fixture("single_pane_grows_to_space");
    let run = f.run();
    assert_ok(&run, f.name);
    let p = pane(&run, 1);
    assert_eq!(p.geom.x, 0);
    assert_eq!(p.geom.cols.constraint, Constraint::Percent(100.0));
    assert_eq!(p.geom.cols.as_usize(), 80);
    assert_eq!(p.geom.y, 0);
    assert_eq!(p.geom.rows.constraint, Constraint::Percent(100.0));
    assert_eq!(p.geom.rows.as_usize(), 10);
}

#[test]
fn empty_pane_map_reports_unchanged() {
    let f = fixture("empty_pane_map_reports_unchanged");
    let run = f.run();
    assert_eq!(run.outcome, Outcome::PaneSizeUnchanged);
    assert!(run.panes.is_empty());
}

#[test]
fn two_panes_split_available_space() {
    let f = fixture("two_panes_split_available_space");
    let run = f.run();
    assert_ok(&run, f.name);
    let one = pane(&run, 1);
    let two = pane(&run, 2);
    assert_eq!(one.geom.x, 0);
    assert_eq!(one.geom.cols.as_usize(), 50);
    assert_eq!(two.geom.x, 50);
    assert_eq!(two.geom.cols.as_usize(), 50);
}

#[test]
fn identical_sizes_report_unchanged() {
    let f = fixture("identical_sizes_report_unchanged");
    let run = f.run();
    assert_eq!(run.outcome, Outcome::PaneSizeUnchanged);
    assert_no_mutation(&run, &f.specs, f.name);
}

#[test]
fn unchanged_sizes_still_reposition() {
    let f = fixture("unchanged_sizes_still_reposition");
    let run = f.run();
    assert_eq!(run.outcome, Outcome::PaneSizeUnchanged);
    assert_eq!(pane(&run, 2).geom.x, 40);
}

#[test]
fn vertical_split_resizes_rows() {
    let f = fixture("vertical_split_resizes_rows");
    let run = f.run();
    assert_ok(&run, f.name);
    let one = pane(&run, 1);
    let two = pane(&run, 2);
    assert_eq!(one.geom.y, 0);
    assert_eq!(one.geom.rows.as_usize(), 15);
    assert_eq!(two.geom.y, 15);
    assert_eq!(two.geom.rows.as_usize(), 15);
    assert_eq!(one.geom.x, 0);
    assert_eq!(one.geom.cols.as_usize(), 80);
    assert_eq!(two.geom.x, 0);
    assert_eq!(two.geom.cols.as_usize(), 80);
}

#[test]
fn three_panes_round_up_first() {
    let f = fixture("three_panes_round_up_first");
    let run = f.run();
    assert_ok(&run, f.name);
    let sizes: Vec<usize> = [1, 2, 3]
        .iter()
        .map(|id| pane(&run, *id).geom.cols.as_usize())
        .collect();
    let positions: Vec<usize> = [1, 2, 3].iter().map(|id| pane(&run, *id).geom.x).collect();
    assert_eq!(sizes, vec![34, 33, 33]);
    assert_eq!(positions, vec![0, 34, 67]);
}

#[test]
fn over_rounded_band_shrinks_last() {
    let f = fixture("over_rounded_band_shrinks_last");
    let run = f.run();
    assert_ok(&run, f.name);
    let sizes: Vec<usize> = [1, 2, 3]
        .iter()
        .map(|id| pane(&run, *id).geom.cols.as_usize())
        .collect();
    let positions: Vec<usize> = [1, 2, 3].iter().map(|id| pane(&run, *id).geom.x).collect();
    assert_eq!(sizes, vec![34, 33, 33]);
    assert_eq!(positions, vec![0, 34, 67]);
    for id in [1u32, 2, 3] {
        assert_eq!(
            pane(&run, id).geom.cols.constraint,
            input_spec(&f.specs, tid(id)).cols.constraint
        );
    }
}

#[test]
fn fixed_pane_locks_and_percent_absorbs() {
    let f = fixture("fixed_pane_locks_and_percent_absorbs");
    let run = f.run();
    assert_ok(&run, f.name);
    let one = pane(&run, 1);
    let two = pane(&run, 2);
    assert_eq!(one.geom.x, 0);
    assert_eq!(one.geom.cols.constraint, Constraint::Fixed(20));
    assert_eq!(one.geom.cols.as_usize(), 20);
    assert_eq!(two.geom.x, 20);
    assert_eq!(two.geom.cols.as_usize(), 80);
}

#[test]
fn fixed_pane_excluded_from_rounding() {
    let f = fixture("fixed_pane_excluded_from_rounding");
    let run = f.run();
    assert_ok(&run, f.name);
    let sizes: Vec<usize> = [1, 2, 3]
        .iter()
        .map(|id| pane(&run, *id).geom.cols.as_usize())
        .collect();
    let positions: Vec<usize> = [1, 2, 3].iter().map(|id| pane(&run, *id).geom.x).collect();
    assert_eq!(sizes, vec![21, 40, 39]);
    assert_eq!(positions, vec![0, 21, 61]);
    assert_eq!(pane(&run, 1).geom.cols.constraint, Constraint::Fixed(21));
}

#[test]
fn pane_spanning_two_bands_adjusted_once() {
    let f = fixture("pane_spanning_two_bands_adjusted_once");
    let run = f.run();
    assert_ok(&run, f.name);
    let expected = [
        (1u32, 0usize, 33usize),
        (3, 33, 67),
        (4, 0, 17),
        (5, 17, 16),
    ];
    for (id, x, cols) in expected {
        let p = pane(&run, id);
        assert_eq!(p.geom.x, x, "pane {} x", id);
        assert_eq!(p.geom.cols.as_usize(), cols, "pane {} cols", id);
    }
}

#[test]
fn stack_main_pane_carries_whole_stack() {
    let f = fixture("stack_main_pane_carries_whole_stack");
    let run = f.run();
    assert_ok(&run, f.name);
    let expected = [(1u32, 0usize, 1usize), (2, 1, 10), (3, 11, 1), (4, 0, 12)];
    for (id, y, rows) in expected {
        let p = pane(&run, id);
        assert_eq!(p.geom.y, y, "pane {} y", id);
        assert_eq!(p.geom.rows.as_usize(), rows, "pane {} rows", id);
    }
}

#[test]
fn short_stack_is_rejected_without_mutation() {
    let f = fixture("short_stack_is_rejected_without_mutation");
    let run = f.run();
    assert_eq!(
        run.outcome,
        Outcome::LayoutRejected,
        "a stack shorter than its member count must be refused, not reported as applied"
    );
    assert_no_mutation(&run, &f.specs, f.name);
}

#[test]
fn override_pane_updates_override_only() {
    let f = fixture("override_pane_updates_override_only");
    let run = f.run();
    assert_ok(&run, f.name);
    let one = pane(&run, 1);
    assert_eq!(one.geom.x, 0);
    assert_eq!(one.geom.cols.as_usize(), 50);
    assert_eq!(one.geom_override, None);

    let two = pane(&run, 2);
    assert_geom_field_eq(
        &two.geom,
        &input_spec(&f.specs, tid(2)).geom(),
        "pane 2 geom",
    );
    let override_geom = two.geom_override.expect("pane 2 lost its geom_override");
    assert_eq!(override_geom.x, 50);
    assert_eq!(override_geom.cols.as_usize(), 50);
    assert_geom_field_eq(&two.current, &override_geom, "pane 2 current");
}

#[test]
fn no_room_for_all_spans() {
    let f = fixture("no_room_for_all_spans");
    let run = f.run();
    match &run.outcome {
        Outcome::OtherErr(msg) => assert!(
            msg.contains("Ran out of room for spans"),
            "unexpected error: {}",
            msg
        ),
        other => panic!("expected OtherErr, got {:?}", other),
    }
    assert_no_mutation(&run, &f.specs, f.name);
}

#[test]
fn conflicting_required_constraints_fail() {
    let f = fixture("conflicting_required_constraints_fail");
    let run = f.run();
    assert!(
        matches!(run.outcome, Outcome::OtherErr(_)),
        "expected OtherErr, got {:?}",
        run.outcome
    );
    assert_ne!(run.outcome, Outcome::PaneSizeUnchanged);
    if let Outcome::OtherErr(msg) = &run.outcome {
        assert!(
            !msg.contains("Ran out of room"),
            "unexpected error: {}",
            msg
        );
    }
    assert_no_mutation(&run, &f.specs, f.name);
}

#[test]
fn stack_without_flexible_pane_is_ignored() {
    let f = fixture("stack_without_flexible_pane_is_ignored");
    let run = f.run();
    assert_eq!(run.outcome, Outcome::PaneSizeUnchanged);
    assert_no_mutation(&run, &f.specs, f.name);
}

#[test]
fn under_specified_percents() {
    let f = fixture("under_specified_percents");
    assert_class_b_invariants(&f, &f.run());
}

#[test]
fn over_specified_percents() {
    let f = fixture("over_specified_percents");
    assert_class_b_invariants(&f, &f.run());
}

#[test]
fn degenerate_across_two_bands() {
    let f = fixture("degenerate_across_two_bands");
    assert_class_b_invariants(&f, &f.run());
}

#[test]
fn fixed_and_degenerate_percents() {
    let f = fixture("fixed_and_degenerate_percents");
    assert_class_b_invariants(&f, &f.run());
}

#[test]
fn zero_flex_space_division() {
    let mut f = fixture("zero_flex_space_division");
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        f.direction = direction;
        if direction == SplitDirection::Vertical {
            for pane in &mut f.specs {
                std::mem::swap(&mut pane.x, &mut pane.y);
                std::mem::swap(&mut pane.cols, &mut pane.rows);
            }
        }
        let run = f.run();
        assert!(
            matches!(&run.outcome, Outcome::OtherErr(error) if error.contains("Ran out of room")),
            "{:?}",
            run.outcome
        );
        assert_no_mutation(&run, &f.specs, f.name);
    }
}

#[test]
fn saturated_flex_space_division() {
    let mut f = fixture("saturated_flex_space_division");
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        f.direction = direction;
        if direction == SplitDirection::Vertical {
            for pane in &mut f.specs {
                std::mem::swap(&mut pane.x, &mut pane.y);
                std::mem::swap(&mut pane.cols, &mut pane.rows);
            }
        }
        let run = f.run();
        assert!(
            matches!(&run.outcome, Outcome::OtherErr(error) if error.contains("Ran out of room")),
            "{:?}",
            run.outcome
        );
        assert_no_mutation(&run, &f.specs, f.name);
    }
}

#[test]
fn fixed_panes_that_exactly_fill_the_space_are_accepted() {
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        let mut panes = vec![
            spec(1, 0, 0, dim_fixed(20), dim_pct(100.0, 10)),
            spec(2, 20, 0, dim_fixed(30), dim_pct(100.0, 10)),
        ];
        if direction == SplitDirection::Vertical {
            for pane in &mut panes {
                std::mem::swap(&mut pane.x, &mut pane.y);
                std::mem::swap(&mut pane.cols, &mut pane.rows);
            }
        }
        let run = run_layout(&panes, direction, 50);
        assert_eq!(run.outcome, Outcome::PaneSizeUnchanged);
        assert_no_mutation(&run, &panes, "exact fixed fit");
    }
}

#[test]
fn stacked_pane_with_override() {
    let f = fixture("stacked_pane_with_override");
    let run = f.run();
    println!("{} = {}", f.name, canonical(&run));
}

// ---- property-based tests (hegel) ----

use hegel::generators;

#[hegel::test]
fn valid_layouts_still_partition_the_screen_after_resize(tc: hegel::TestCase) {
    let a = tc.draw(generators::integers::<usize>().min_value(1).max_value(4)) * 10;
    let b = tc.draw(generators::integers::<usize>().min_value(1).max_value(4)) * 10;
    let c = tc.draw(generators::integers::<usize>().min_value(1).max_value(4)) * 10;
    let space = tc.draw(
        generators::integers::<usize>()
            .min_value(100)
            .max_value(300),
    );

    // Two bands share the rightmost pane; the lower band splits the left pane again.
    // Unlike arbitrary rectangles, these inputs are feasible and must tile the entire axis.
    // Exercise both axes and fixed/percentage spans for every generated set of dimensions.
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        for fixed in [false, true] {
            let flex = if fixed { a + b } else { a + b + c };
            let dimension = |size| dim_pct(100.0 * size as f64 / flex as f64, size);
            let mut specs = vec![
                spec(1, 0, 0, dimension(a + b), dim_pct(50.0, 10)),
                spec(
                    2,
                    a + b,
                    0,
                    if fixed { dim_fixed(c) } else { dimension(c) },
                    dim_pct(100.0, 20),
                ),
                spec(3, 0, 10, dimension(a), dim_pct(50.0, 10)),
                spec(4, a, 10, dimension(b), dim_pct(50.0, 10)),
            ];
            if direction == SplitDirection::Vertical {
                for spec in &mut specs {
                    std::mem::swap(&mut spec.x, &mut spec.y);
                    std::mem::swap(&mut spec.cols, &mut spec.rows);
                }
            }
            let fixture = Fixture {
                name: "valid generated partition",
                specs,
                direction,
                space,
                class: Class::B,
                bands: vec![vec![tid(1), tid(2)], vec![tid(3), tid(4), tid(2)]],
            };
            let run = fixture.run();
            assert_eq!(run.panes.len(), fixture.specs.len());
            assert_class_b_invariants(&fixture, &run);
        }
    }
}

/// Generated `PaneSpec`s for the class-B oracle.
///
/// Two narrowings, both harness limitations rather than contract restrictions:
/// * `stacked: None` - the stacked path in `PaneResizer::apply_spans` goes through `StackedPanes`,
///   which calls `Pane` methods `MockPane` leaves as `unimplemented!()`.
/// * `geom_override: None` - `assert_cross_axis_untouched`/`assert_constraints_preserved` compare
///   the snapshot's `current_geom` against `spec.geom()`, which is only the same thing when there
///   is no override.
#[hegel::composite]
fn any_pane_spec(tc: &hegel::TestCase, id: u32) -> PaneSpec {
    // percentages that under- and over-sum 100 across a layout, plus the degenerate ends
    let percents = vec![0.0, 0.1, 1.0, 25.0, 33.3, 50.0, 60.0, 100.0, 150.0];
    let dimension = |tc: &hegel::TestCase| {
        if tc.draw(generators::booleans()) {
            let mut d = yinz_types::pane_size::Dimension::percent(
                tc.draw(generators::sampled_from(percents.clone())),
            );
            // a pane of size 0 is not a legal layout - the resizer rejects it outright
            d.set_inner(tc.draw(generators::integers::<usize>().min_value(1).max_value(200)));
            d
        } else {
            yinz_types::pane_size::Dimension::fixed(
                tc.draw(generators::integers::<usize>().min_value(1).max_value(200)),
            )
        }
    };
    let cols = dimension(tc);
    let rows = dimension(tc);
    PaneSpec {
        id: PaneId::Terminal(crate::TerminalId::new(id)),
        x: tc.draw(generators::integers::<usize>().max_value(200)),
        y: tc.draw(generators::integers::<usize>().max_value(200)),
        cols,
        rows,
        stacked: None,
        logical_position: tc.draw(generators::optional(
            generators::integers::<usize>().max_value(8),
        )),
        geom_override: None,
    }
}

#[hegel::test]
fn class_b_invariants_hold_for_any_layout(tc: hegel::TestCase) {
    // ids must be distinct: `run_layout` keys a `HashMap` by id, so duplicates would silently
    // drop panes and make `input_spec` lookups meaningless
    let pane_count = tc.draw(generators::integers::<u32>().max_value(6));
    let specs: Vec<PaneSpec> = (0..pane_count)
        .map(|id| tc.draw(any_pane_spec(id)))
        .collect();
    let direction = tc.draw(generators::sampled_from(vec![
        SplitDirection::Horizontal,
        SplitDirection::Vertical,
    ]));
    // `space` is a terminal coordinate in production: every caller reaches `PaneResizer::layout`
    // through `TiledPaneGrid`, which passes `display_area.cols`/`rows`. Values beyond screen scale
    // are not reachable, and a `space` near `usize::MAX` overflows `discretize_spans`.
    let space = tc.draw(generators::integers::<usize>().max_value(500));

    let fixture = Fixture {
        name: "generated",
        specs: specs.clone(),
        direction,
        space,
        class: Class::B,
        // bands are a hand-declared grouping of panes expected to tile the axis; there is no way
        // to derive them from an arbitrary layout, so the band assertion is not exercised here
        bands: vec![],
    };
    let run = run_layout(&specs, direction, space);
    assert!(
        !run.panes.is_empty() || specs.is_empty(),
        "layout of {:?} (direction {:?}, space {}) lost every pane",
        specs,
        direction,
        space
    );
    // A layout that cannot fit the space it was given (say a `Fixed(1)` pane in 0 columns) is
    // legitimately unsatisfiable. `PaneResizer::layout` bails before `apply_spans`, so the
    // contract there is that nothing moved - the output invariants have no output to check.
    if let Outcome::OtherErr(_) | Outcome::LayoutRejected = run.outcome {
        for snapshot in &run.panes {
            assert_geom_field_eq(
                &snapshot.current,
                &input_spec(&specs, snapshot.id).geom(),
                &format!("{:?} after a failed layout", snapshot.id),
            );
        }
        return;
    }
    assert_class_b_invariants(&fixture, &run);
}
