//! Coverage for `Row`'s cached metrics - `width` and its companion `all_single_width` - and for
//! the `add_character_at` fast path they enable.
//!
//! The fast path is only correct while the caches are honest, and a stale cache shows up as
//! silently corrupt rendering rather than a panic. So most of what follows asserts the two caches
//! against ground truth recomputed from `columns`, after every kind of mutation `Row` exposes.

use super::{Osc133Marker, Osc133MarkerKind, Row};
use crate::panes::terminal_character::{TerminalCharacter, EMPTY_TERMINAL_CHARACTER};
use hegel::generators::{self, Generator};
use std::collections::VecDeque;

const WIDE: char = '你'; // two columns
const EMOJI: char = '📦'; // two columns
const COMBINING: char = '\u{0301}'; // combining acute accent, zero columns

fn character(c: char) -> TerminalCharacter {
    TerminalCharacter::new(c)
}

fn row_of(text: &str) -> Row {
    Row::from_columns(text.chars().map(character).collect::<VecDeque<_>>())
}

fn text_of(row: &Row) -> String {
    row.columns.iter().map(|c| c.character).collect()
}

/// The two caches, checked against values recomputed from `columns`.
///
/// `width()` always walks the row, so it is the oracle for the `width` cache; `all_single_width`
/// returns the cache whenever it is populated, so comparing it against a fresh scan of `columns`
/// catches a mutation that forgot to invalidate.
fn assert_caches_are_honest(row: &mut Row, context: &str) {
    let recomputed_width = row.width();
    assert_eq!(
        row.width_cached(),
        recomputed_width,
        "{}: cached width disagrees with a fresh scan",
        context
    );
    let recomputed_all_single_width = row.columns.iter().all(|c| c.width() == 1);
    assert_eq!(
        row.all_single_width(),
        recomputed_all_single_width,
        "{}: cached all_single_width disagrees with a fresh scan",
        context
    );
}

/// A row whose caches are warm and currently say "every cell is one column wide".
fn warm_single_width_row(text: &str) -> Row {
    let mut row = row_of(text);
    assert_eq!(row.width_cached(), text.chars().count());
    assert!(
        row.all_single_width(),
        "fixture should start out all single width"
    );
    row
}

// -- the caches themselves ------------------------------------------------------------------

#[test]
fn width_cached_populates_both_caches_together() {
    let mut row = row_of("hello");
    assert_eq!(row.width_cached(), 5);
    assert!(row.all_single_width());

    let mut wide_row = row_of("a你b");
    assert_eq!(wide_row.width_cached(), 4);
    assert!(!wide_row.all_single_width());
}

#[test]
fn all_single_width_computes_itself_when_only_the_width_cache_is_warm() {
    // `width_cached` returns early when the width alone is known, so `all_single_width` must not
    // rely on it to fill the companion cache in
    let mut row = row_of("abc");
    row.width = Some(3);
    row.all_single_width = None;

    assert!(row.all_single_width());
    assert_eq!(row.all_single_width, Some(true));
}

#[test]
fn zero_width_characters_are_not_all_single_width() {
    let mut row = row_of("a\u{0301}b");
    assert_eq!(row.width_cached(), 2);
    assert!(
        !row.all_single_width(),
        "a combining mark occupies no column, so column index and columns index diverge"
    );
}

// -- every mutation invalidates ------------------------------------------------------------

/// A named change to a row, applied once its caches are warm.
type Mutation = (&'static str, Box<dyn Fn(&mut Row)>);

/// Each case warms the caches on an all-single-width row, applies one mutation that introduces a
/// wide or zero-width character, and checks both caches followed suit.
#[test]
fn every_mutation_keeps_the_caches_honest() {
    // (name, mutation)
    let mutations: Vec<Mutation> = vec![
        ("push", Box::new(|row: &mut Row| row.push(character(WIDE)))),
        (
            "add_character_at past the end",
            Box::new(|row: &mut Row| row.add_character_at(character(WIDE), 10)),
        ),
        (
            "add_character_at at the end",
            Box::new(|row: &mut Row| row.add_character_at(character(WIDE), 5)),
        ),
        (
            "add_character_at in the middle",
            Box::new(|row: &mut Row| row.add_character_at(character(WIDE), 2)),
        ),
        (
            "add_character_at a combining mark",
            Box::new(|row: &mut Row| row.add_character_at(character(COMBINING), 2)),
        ),
        (
            "insert_character_at",
            Box::new(|row: &mut Row| row.insert_character_at(character(WIDE), 2)),
        ),
        (
            "replace_character_at",
            Box::new(|row: &mut Row| row.replace_character_at(character(WIDE), 2)),
        ),
        (
            "replace_columns",
            Box::new(|row: &mut Row| row.replace_columns("a你b".chars().map(character).collect())),
        ),
        ("truncate", Box::new(|row: &mut Row| row.truncate(3))),
        (
            "replace_and_pad_end",
            Box::new(|row: &mut Row| row.replace_and_pad_end(1, 4, character(WIDE))),
        ),
        (
            "replace_and_pad_beginning",
            Box::new(|row: &mut Row| row.replace_and_pad_beginning(2, character(WIDE))),
        ),
        (
            "append",
            Box::new(|row: &mut Row| row.append(&mut row_of("你"))),
        ),
        (
            "drain_until",
            Box::new(|row: &mut Row| {
                row.drain_until(2);
            }),
        ),
        (
            "delete_and_return_character",
            Box::new(|row: &mut Row| {
                row.delete_and_return_character(1);
            }),
        ),
        (
            "split_to_rows_of_length",
            Box::new(|row: &mut Row| {
                row.split_to_rows_of_length(2);
            }),
        ),
        (
            "with_character",
            Box::new(|row: &mut Row| {
                let taken = std::mem::replace(row, Row::new());
                *row = taken.with_character(character(EMOJI));
            }),
        ),
    ];

    for (name, mutate) in mutations {
        let mut row = warm_single_width_row("hello");
        mutate(&mut row);
        assert_caches_are_honest(&mut row, name);
    }
}

#[test]
fn appending_a_wide_character_to_a_warm_row_flips_the_companion_cache() {
    // `add_character_at`'s `Ordering::Equal` arm updates the width in place rather than
    // invalidating, so it has to correct `all_single_width` itself
    let mut row = warm_single_width_row("abc");

    row.add_character_at(character(WIDE), 3);

    assert_eq!(row.width_cached(), 5);
    assert!(!row.all_single_width());
    assert_caches_are_honest(&mut row, "append wide onto warm row");

    // and the row still behaves once it is no longer all-single-width: writing over the second
    // cell of the wide character pads in the proper place
    row.add_character_at(character('z'), 4);
    assert_eq!(text_of(&row), "abc z");
    assert_caches_are_honest(&mut row, "overwrite second half of wide character");
}

#[test]
fn appending_single_width_characters_leaves_the_row_all_single_width() {
    let mut row = warm_single_width_row("abc");

    row.add_character_at(character('d'), 3);
    row.add_character_at(character('e'), 4);

    assert!(row.all_single_width());
    assert_eq!(row.width_cached(), 5);
    assert_eq!(text_of(&row), "abcde");
}

// -- the add_character_at fast path ----------------------------------------------------------

/// What the pre-existing slow path produces when a single-width character replaces another
/// single-width character in a row of single-width characters: the cell at `x` is swapped, and a
/// marker sitting on `x` is dropped unless the cell it marked was blank.
fn reference_overwrite(row: &Row, c: char, x: usize) -> (String, Vec<Osc133Marker>) {
    let mut text: Vec<char> = row.columns.iter().map(|column| column.character).collect();
    let replacing_blank = text[x] == ' ';
    text[x] = c;
    let markers = row
        .osc133_markers
        .iter()
        .copied()
        .filter(|marker| marker.column != x || replacing_blank)
        .collect();
    (text.into_iter().collect(), markers)
}

#[test]
fn fast_path_overwrite_matches_the_slow_path_result() {
    for x in 0..5 {
        for replacement in ['z', ' '] {
            let mut row = warm_single_width_row("ab cd");
            row.add_osc133_marker(x, Osc133MarkerKind::Prompt);
            let (expected_text, expected_markers) = reference_overwrite(&row, replacement, x);

            row.add_character_at(character(replacement), x);

            assert_eq!(
                text_of(&row),
                expected_text,
                "overwriting column {} with {:?}",
                x,
                replacement
            );
            assert_eq!(
                row.osc133_markers, expected_markers,
                "osc133 markers after overwriting column {} with {:?}",
                x, replacement
            );
            assert_caches_are_honest(&mut row, "fast path overwrite");
        }
    }
}

#[test]
fn fast_path_keeps_a_marker_when_it_overwrites_a_blank() {
    let mut row = warm_single_width_row("ab cd");
    row.add_osc133_marker(2, Osc133MarkerKind::Prompt);

    row.add_character_at(character('X'), 2);

    assert_eq!(
        row.osc133_markers,
        vec![Osc133Marker {
            column: 2,
            kind: Osc133MarkerKind::Prompt,
        }],
        "the cell under the marker was blank, so the marker survives"
    );
}

#[test]
fn fast_path_drops_a_marker_when_it_overwrites_a_non_blank_cell() {
    let mut row = warm_single_width_row("ab cd");
    row.add_osc133_marker(1, Osc133MarkerKind::Prompt);

    row.add_character_at(character('X'), 1);

    assert!(
        row.osc133_markers.is_empty(),
        "the marked cell held a real character, so the marker is stale"
    );
}

#[test]
fn fast_path_is_declined_when_the_incoming_character_is_not_single_width() {
    let mut row = warm_single_width_row("abcde");

    row.add_character_at(character(WIDE), 1);

    // the wide character consumed the cell after it
    assert_eq!(text_of(&row), "a你de");
    assert_eq!(row.width(), 5);
    assert_caches_are_honest(&mut row, "wide character into single width row");
}

#[test]
fn fast_path_is_declined_when_the_row_already_holds_a_wide_character() {
    let mut row = row_of("a你de");
    assert_eq!(row.width_cached(), 5);
    assert!(!row.all_single_width());

    // column 3 is `d`, which sits at columns index 2 - the fast path's index shortcut would have
    // written to the wrong cell here
    row.add_character_at(character('Z'), 3);

    assert_eq!(text_of(&row), "a你Ze");
    assert_caches_are_honest(&mut row, "single width character into wide row");
}

#[test]
fn fast_path_is_declined_for_zero_width_characters() {
    let mut row = warm_single_width_row("abcde");

    row.add_character_at(character(COMBINING), 2);

    // the slow path notices the incoming character is narrower than the one it replaced and pads
    // the freed column, so the row still covers five columns - the fast path would instead have
    // left a five-cell row that only measures four columns wide
    assert_eq!(row.width(), 5);
    assert_eq!(text_of(&row), "ab\u{0301} de");
    assert!(!row.all_single_width());
    assert_caches_are_honest(&mut row, "combining mark into single width row");
}

#[test]
fn a_row_stays_correct_across_repeated_fast_and_slow_path_writes() {
    // the sequence a redraw-heavy program produces: rewrite a line in place, then let a wide
    // character land in it, then keep rewriting
    let mut row = warm_single_width_row("..........");

    for x in 0..10 {
        row.add_character_at(character('a'), x);
    }
    assert_eq!(text_of(&row), "aaaaaaaaaa");
    assert!(row.all_single_width());
    assert_caches_are_honest(&mut row, "all fast path");

    row.add_character_at(character(EMOJI), 4);
    assert_eq!(row.width(), 10);
    assert_caches_are_honest(&mut row, "emoji lands mid row");

    // now every write has to take the slow path, because column 6 is no longer columns index 6
    row.add_character_at(character('b'), 6);
    assert_eq!(text_of(&row), "aaaa📦baaa");
    assert_caches_are_honest(&mut row, "write after the emoji");

    // overwrite the emoji itself and the row becomes all-single-width again
    row.add_character_at(character('c'), 4);
    row.add_character_at(character('d'), 5);
    assert_eq!(text_of(&row), "aaaacdbaaa");
    assert!(
        row.all_single_width(),
        "the wide character is gone, so the fast path should be available again"
    );
    assert_caches_are_honest(&mut row, "row is all single width once more");

    row.add_character_at(character('e'), 7);
    assert_eq!(text_of(&row), "aaaacdbeaa");
    assert_caches_are_honest(&mut row, "fast path again");
}

#[test]
fn padding_a_row_out_and_then_overwriting_it_stays_correct() {
    // `Ordering::Less` pads with blanks and invalidates; the following writes take the fast path
    let mut row = warm_single_width_row("ab");

    row.add_character_at(character('z'), 5);
    assert_eq!(text_of(&row), "ab   z");
    assert_caches_are_honest(&mut row, "padded out");

    row.add_character_at(character('Y'), 3);
    assert_eq!(text_of(&row), "ab Y z");
    assert_caches_are_honest(&mut row, "overwrite inside the padding");
}

#[test]
fn empty_terminal_character_is_single_width() {
    // the padding character has to count as single width, or padding a row would permanently
    // disable the fast path for it
    assert_eq!(EMPTY_TERMINAL_CHARACTER.width(), 1);
}

// -- property tests --------------------------------------------------------------------------

/// The alphabet the generated rows are drawn from: single-width ASCII, wide characters, and a
/// zero-width combining mark. The mix matters - the fast path is only reachable while a row holds
/// nothing but single-width cells, and the bugs live at the transitions.
fn character_generator() -> impl Generator<char> {
    generators::sampled_from(vec!['a', 'b', ' ', WIDE, EMOJI, COMBINING])
}

#[hegel::test]
fn fast_path_overwrite_agrees_with_the_reference_for_any_single_width_row(tc: hegel::TestCase) {
    let text: String = tc.draw(
        generators::vecs(generators::sampled_from(vec!['a', 'b', ' ']))
            .min_size(1)
            .max_size(40)
            .map(|characters: Vec<char>| characters.into_iter().collect()),
    );
    let column_count = text.chars().count();
    let x = tc.draw(generators::integers::<usize>().max_value(column_count - 1));
    let replacement = tc.draw(generators::sampled_from(vec!['a', 'b', ' ', 'Z']));
    let marker_column = tc.draw(generators::integers::<usize>().max_value(column_count - 1));

    let mut row = warm_single_width_row(&text);
    row.add_osc133_marker(marker_column, Osc133MarkerKind::Prompt);
    let (expected_text, expected_markers) = reference_overwrite(&row, replacement, x);

    row.add_character_at(character(replacement), x);

    assert_eq!(text_of(&row), expected_text);
    assert_eq!(row.osc133_markers, expected_markers);
    assert_caches_are_honest(&mut row, "generated fast path overwrite");
}

/// A stateful model test: apply an arbitrary sequence of `Row` mutations and check after each one
/// that neither cache has gone stale. This is the test most likely to catch a mutation that
/// forgets to call `invalidate_width_cache`.
struct RowCacheModel {
    row: Row,
}

#[hegel::state_machine]
impl RowCacheModel {
    #[rule]
    fn warm_the_caches(&mut self, _: hegel::TestCase) {
        // half the point of the model is that mutations interleave with cache warming
        self.row.width_cached();
        self.row.all_single_width();
    }

    #[rule]
    fn push(&mut self, tc: hegel::TestCase) {
        let c = tc.draw(character_generator());
        self.row.push(character(c));
    }

    #[rule]
    fn add_character_at(&mut self, tc: hegel::TestCase) {
        let c = tc.draw(character_generator());
        let x = tc.draw(generators::integers::<usize>().max_value(40));
        self.row.add_character_at(character(c), x);
    }

    #[rule]
    fn insert_character_at(&mut self, tc: hegel::TestCase) {
        let c = tc.draw(character_generator());
        let x = tc.draw(generators::integers::<usize>().max_value(40));
        self.row.insert_character_at(character(c), x);
    }

    #[rule]
    fn replace_character_at(&mut self, tc: hegel::TestCase) {
        let c = tc.draw(character_generator());
        let x = tc.draw(generators::integers::<usize>().max_value(40));
        self.row.replace_character_at(character(c), x);
    }

    #[rule]
    fn replace_columns(&mut self, tc: hegel::TestCase) {
        let characters: Vec<char> = tc.draw(
            generators::vecs(character_generator())
                .min_size(1)
                .max_size(20),
        );
        self.row
            .replace_columns(characters.into_iter().map(character).collect());
    }

    #[rule]
    fn truncate(&mut self, tc: hegel::TestCase) {
        let x = tc.draw(generators::integers::<usize>().max_value(40));
        self.row.truncate(x);
    }

    #[rule]
    fn replace_and_pad_end(&mut self, tc: hegel::TestCase) {
        let from = tc.draw(generators::integers::<usize>().max_value(40));
        let to = tc.draw(generators::integers::<usize>().max_value(40));
        let c = tc.draw(character_generator());
        self.row
            .replace_and_pad_end(from.min(to), from.max(to), character(c));
    }

    #[rule]
    fn replace_and_pad_beginning(&mut self, tc: hegel::TestCase) {
        let to = tc.draw(generators::integers::<usize>().max_value(40));
        let c = tc.draw(character_generator());
        self.row.replace_and_pad_beginning(to, character(c));
    }

    #[rule]
    fn append(&mut self, tc: hegel::TestCase) {
        let characters: Vec<char> = tc.draw(generators::vecs(character_generator()).max_size(10));
        let mut to_append = Row::from_columns(characters.into_iter().map(character).collect());
        self.row.append(&mut to_append);
    }

    #[rule]
    fn drain_until(&mut self, tc: hegel::TestCase) {
        let x = tc.draw(generators::integers::<usize>().max_value(40));
        self.row.drain_until(x);
    }

    #[rule]
    fn delete_and_return_character(&mut self, tc: hegel::TestCase) {
        let x = tc.draw(generators::integers::<usize>().max_value(40));
        self.row.delete_and_return_character(x);
    }

    #[rule]
    fn split_to_rows_of_length(&mut self, tc: hegel::TestCase) {
        let max_row_length = tc.draw(generators::integers::<usize>().min_value(1).max_value(20));
        self.row.split_to_rows_of_length(max_row_length);
    }

    /// Neither cache may ever disagree with a fresh scan of `columns`. Checked without warming
    /// them, so a cache that happens to be cold is simply skipped rather than filled in here.
    #[invariant]
    fn caches_never_go_stale(&self, _: hegel::TestCase) {
        if let Some(cached_width) = self.row.width {
            assert_eq!(
                cached_width,
                self.row.width(),
                "cached width went stale: {:?}",
                text_of(&self.row)
            );
        }
        if let Some(cached_all_single_width) = self.row.all_single_width {
            assert_eq!(
                cached_all_single_width,
                self.row.columns.iter().all(|c| c.width() == 1),
                "cached all_single_width went stale: {:?}",
                text_of(&self.row)
            );
        }
    }
}

#[hegel::test]
fn row_caches_survive_any_sequence_of_mutations(tc: hegel::TestCase) {
    let model = RowCacheModel {
        row: warm_single_width_row("hello world"),
    };
    hegel::stateful::run(model, tc);
}

#[test]
fn erase_line_end_uses_display_columns() {
    for (content, from, to, expected) in [
        ("ab你cd", 2, 6, "ab    "),
        ("ab你cd", 3, 6, "ab    "),
        ("ab你c好", 3, 8, "ab      "),
        ("ab你c好", 5, 8, "ab你c   "),
        ("你好你好", 3, 8, "你      "),
        ("你好你好", 2, 8, "你      "),
        ("你好", 0, 6, "      "),
        ("ab", 4, 6, "ab    "),
    ] {
        let mut row = row_of(content);
        row.width_cached();
        row.replace_and_pad_end(from, to, EMPTY_TERMINAL_CHARACTER);
        assert_eq!(text_of(&row), expected, "{content:?}, {from}");
        assert_eq!(row.width(), to);
        assert_caches_are_honest(&mut row, content);
    }
}

#[test]
fn erase_line_beginning_uses_display_columns() {
    for (content, to, expected, width) in [
        ("ab你c好", 2, "    c好", 7),
        ("ab你c好", 3, "    c好", 7),
        ("ab你c好", 4, "     好", 7),
        ("你好你好", 2, "    你好", 8),
        ("你好你好", 3, "    你好", 8),
        ("ab", 0, " b", 2),
        ("ab", 4, "     ", 5),
    ] {
        let mut row = row_of(content);
        row.width_cached();
        row.replace_and_pad_beginning(to, EMPTY_TERMINAL_CHARACTER);
        assert_eq!(text_of(&row), expected, "{content:?}, {to}");
        assert_eq!(row.width(), width);
        assert_caches_are_honest(&mut row, content);
    }
}

#[test]
fn erase_line_end_preserves_gap_background_and_metadata() {
    use crate::panes::terminal_character::AnsiCode;
    for content in ["ab", "ab你好"] {
        let mut row = row_of(content);
        row.bg_color = Some(AnsiCode::ColorIndex(2));
        let mut erase = EMPTY_TERMINAL_CHARACTER;
        erase
            .styles
            .update(|s| s.background = Some(AnsiCode::ColorIndex(1)));
        for column in [1, 3, 5] {
            row.add_osc133_marker(column, Osc133MarkerKind::Prompt);
            row.begin_placeholder_marks(column);
        }
        row.replace_and_pad_end(3, 8, erase.clone());
        assert_eq!(text_of(&row), "ab      ");
        assert_eq!(
            row.columns[2].styles.background,
            Some(AnsiCode::ColorIndex(2))
        );
        assert!(row.columns.iter().skip(3).all(|c| *c == erase));
        assert_eq!(
            row.osc133_markers
                .iter()
                .map(|m| m.column)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(
            row.placeholder_marks
                .iter()
                .map(|m| m.column)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_caches_are_honest(&mut row, content);
    }
}

#[test]
fn erase_line_beginning_retains_metadata_after_wide_character() {
    let mut row = row_of("ab你好");
    for column in [2, 3, 4, 5] {
        row.add_osc133_marker(column, Osc133MarkerKind::Prompt);
        row.begin_placeholder_marks(column);
    }
    row.replace_and_pad_beginning(3, EMPTY_TERMINAL_CHARACTER);
    assert_eq!(text_of(&row), "    好");
    assert_eq!(
        row.osc133_markers
            .iter()
            .map(|m| m.column)
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert_eq!(
        row.placeholder_marks
            .iter()
            .map(|m| m.column)
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
}
