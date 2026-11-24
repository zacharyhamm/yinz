//! The pre-check tests are ported from Zellij's `grapheme_grid_tests.rs` (c8585459).
use crate::panes::grapheme_join::{
    self, group_end, prepare_unit, GroupEnd, JOIN_CLASS_TABLE, MAX_GROUP_CELLS, MAX_GROUP_COLUMNS,
};
use crate::panes::terminal_character::{RcCharacterStyles, TerminalCharacter, DEFAULT_STYLES};
use crate::panes::AnsiCode;
use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthChar;

fn has_boundary(left: char, right: char) -> bool {
    let mut text = String::new();
    text.push(left);
    let offset = text.len();
    text.push(right);
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    cursor.is_boundary(&text, 0).unwrap_or(true)
}

struct SampleRng(u64);

impl SampleRng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn the_join_pre_check_never_misses_a_pair_without_a_boundary() {
    let mut sample: Vec<char> = Vec::new();
    for (lo, hi, _) in JOIN_CLASS_TABLE {
        for value in [*lo, *hi, lo.saturating_sub(1), hi + 1] {
            if let Some(c) = char::from_u32(value) {
                sample.push(c);
            }
        }
    }
    let mut rng = SampleRng(0x2027_2027_2027);
    for plane in [0u32, 1, 14] {
        let mut added = 0;
        while added < 10_000 {
            let value = plane * 0x10000 + (rng.next() % 0x10000) as u32;
            if let Some(c) = char::from_u32(value) {
                sample.push(c);
                added += 1;
            }
        }
    }
    sample.sort_unstable();
    sample.dedup();
    let partners: Vec<char> = [
        'a',
        '1',
        ' ',
        '\u{4F60}',
        '\u{1F468}',
        '\u{1F3FD}',
        '\u{200D}',
        '\u{FE0F}',
        '\u{0301}',
        '\u{1F1E9}',
        '\u{1100}',
        '\u{1161}',
        '\u{11A8}',
        '\u{AC00}',
        '\u{AC01}',
        '\u{093E}',
        '\u{0600}',
        '\u{0915}',
        '\u{094D}',
        '\u{0E33}',
        '\u{E0067}',
        '\u{20E3}',
        '\u{00AD}',
        '\u{1F3F3}',
        '\u{231A}',
        '\u{0E49}',
    ]
    .to_vec();
    for &c in &sample {
        for &p in &partners {
            if p.width().unwrap_or(0) != 0 && !has_boundary(c, p) {
                assert!(
                    grapheme_join::may_join(c, p),
                    "missed pair U+{:04X} U+{:04X}",
                    c as u32,
                    p as u32
                );
            }
            if c.width().unwrap_or(0) != 0 && !has_boundary(p, c) {
                assert!(
                    grapheme_join::may_join(p, c),
                    "missed pair U+{:04X} U+{:04X}",
                    p as u32,
                    c as u32
                );
            }
        }
    }
}

#[test]
fn the_join_pre_check_rejects_plain_text_quickly() {
    assert!(!grapheme_join::may_join('a', 'b'));
    assert!(!grapheme_join::may_join('\u{4F60}', '\u{597D}'));
    assert!(grapheme_join::may_join('\u{200D}', 'b'));
    assert!(grapheme_join::may_join('\u{1F44D}', '\u{1F3FD}'));
    assert!(grapheme_join::may_join('\u{0915}', '\u{093E}'));
}

/// Cells as the grid stores them: zero-width code points attached to the cell before them,
/// everything with a width in a cell of its own.
fn cells(text: &str) -> Vec<TerminalCharacter> {
    let mut cells: Vec<TerminalCharacter> = Vec::new();
    for c in text.chars() {
        let cell = TerminalCharacter::new(c);
        if cell.width() == 0 {
            assert!(cells.last_mut().unwrap().attach(c));
        } else {
            cells.push(cell);
        }
    }
    cells
}

fn units(cells: &[TerminalCharacter], same_style: impl Fn(usize, usize) -> bool) -> Vec<GroupEnd> {
    let mut ends = Vec::new();
    let mut index = 0;
    while index < cells.len() {
        let end = group_end(cells, index, &same_style);
        index = end.end;
        ends.push(end);
    }
    ends
}

fn end(end: usize, cut_inside_cluster: bool) -> GroupEnd {
    GroupEnd {
        end,
        cut_inside_cluster,
    }
}

const FAMILY: &str = "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}";

#[test]
fn a_family_is_one_unit_and_plain_text_never_groups() {
    let row = cells(&format!("a{FAMILY}b"));
    assert_eq!(row.len(), 5);
    assert_eq!(
        units(&row, |_, _| true),
        vec![end(1, false), end(4, false), end(5, false)]
    );
    let plain = cells("ex\u{4F60}");
    assert_eq!(
        units(&plain, |_, _| true),
        vec![end(1, false), end(2, false), end(3, false)]
    );
}

#[test]
fn a_style_change_inside_a_family_cuts_the_cluster_at_both_edges() {
    let row = cells(FAMILY);
    let styled = |a: usize, b: usize| a != 1 && b != 1;
    assert_eq!(
        units(&row, styled),
        vec![end(1, true), end(2, true), end(3, false)]
    );
}

#[test]
fn the_cell_and_column_caps_cut_inside_a_cluster() {
    // An endless ZWJ chain of people: each cell joins the next.
    let chain: String = std::iter::repeat_n("\u{1F469}\u{200D}", MAX_GROUP_CELLS + 4).collect();
    let row = cells(&chain);
    assert_eq!(row.len(), MAX_GROUP_CELLS + 4);
    assert_eq!(group_end(&row, 0, |_, _| true), end(MAX_GROUP_CELLS, true));
    let rest = group_end(&row, MAX_GROUP_CELLS, |_, _| true);
    assert_eq!(rest, end(MAX_GROUP_CELLS + 4, false));

    // Cells wider than the grid ever makes them reach the column cap first.
    let member = |text: &str| TerminalCharacter::from_cluster(text, 100, Default::default());
    let row = vec![
        member("\u{1F469}\u{200D}"),
        member("\u{1F469}\u{200D}"),
        member("\u{1F467}"),
    ];
    assert!(row.iter().map(|c| c.width()).sum::<usize>() > MAX_GROUP_COLUMNS);
    assert_eq!(group_end(&row, 0, |_, _| true), end(2, true));
}

#[test]
fn a_family_followed_by_ascii_and_unjoinable_pairs_are_never_cut() {
    let row = cells(&format!("{FAMILY}x"));
    // Only `x` differs in style: the style edge falls on a real cluster boundary.
    assert_eq!(
        units(&row, |_, next| next != 3),
        vec![end(3, false), end(4, false)]
    );
    let row = cells("ex");
    assert_eq!(
        units(&row, |_, _| false),
        vec![end(1, false), end(2, false)]
    );
}

#[test]
fn a_skin_tone_pair_split_by_style_is_cut() {
    let row = cells("\u{1F44D}\u{1F3FD}");
    assert_eq!(units(&row, |_, _| true), vec![end(2, false)]);
    assert_eq!(units(&row, |_, _| false), vec![end(1, true), end(2, false)]);
}

#[test]
fn prepare_unit_strips_vs16_from_narrow_cells_and_marks_complex_units() {
    let warning = cells("\u{26A0}\u{FE0F}");
    assert_eq!(warning[0].width(), 1);
    let prepared = prepare_unit(&warning, 1);
    assert!(prepared.strip_vs16);
    assert_eq!(prepared.text, "\u{26A0}");

    let accent = cells("e\u{301}");
    let prepared = prepare_unit(&accent, 1);
    assert!(!prepared.complex, "combining marks are written plainly");
    assert_eq!(prepared.text, "e\u{301}");

    let family = cells(FAMILY);
    assert!(prepare_unit(&family, 6).complex);
}

#[test]
fn styles_do_not_affect_group_detection_when_equal() {
    let red: RcCharacterStyles = DEFAULT_STYLES
        .foreground(Some(AnsiCode::ColorIndex(1)))
        .into();
    let mut row = cells(FAMILY);
    for cell in &mut row {
        cell.styles = red.clone();
    }
    assert_eq!(
        units(&row, |a, b| row[a].styles == row[b].styles),
        vec![end(3, false)]
    );
}
