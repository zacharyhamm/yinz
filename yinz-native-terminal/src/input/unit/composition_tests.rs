use super::*;

fn preedit(text: &str) -> Ime {
    let end = text.len();
    Ime::Preedit(text.to_owned(), Some((end, end)))
}

#[test]
fn a_whole_composition_leaves_only_the_committed_text() {
    let mut composition = Composition::new();
    let mut committed = Vec::new();

    for event in [
        Ime::Enabled,
        preedit("ni"),
        preedit("ni hao"),
        Ime::Commit("你好".to_owned()),
        Ime::Disabled,
    ] {
        if let Reaction::Commit(text) = composition.ime(event) {
            committed.push(text);
        }
    }

    assert_eq!(committed, vec!["你好".to_owned()]);
    assert_eq!(composition.shown(), None);
    assert_eq!(composition.gate(), Gate::Open);
}

#[test]
fn a_preedit_is_shown_and_never_committed_on_its_own() {
    let mut composition = Composition::new();
    assert_eq!(composition.ime(preedit("ni")), Reaction::Redraw);
    assert_eq!(
        composition.shown().map(|shown| shown.text().to_owned()),
        Some("ni".to_owned())
    );
    assert_eq!(composition.gate(), Gate::Composing);
}

#[test]
fn an_empty_preedit_clears_the_composition() {
    let mut composition = Composition::new();
    composition.ime(preedit("ni"));
    assert_eq!(
        composition.ime(Ime::Preedit(String::new(), None)),
        Reaction::Redraw
    );
    assert_eq!(composition.shown(), None);
    assert_eq!(composition.gate(), Gate::Open);
}

#[test]
fn an_unchanged_preedit_asks_for_no_redraw() {
    let mut composition = Composition::new();
    composition.ime(preedit("ni"));
    assert_eq!(composition.ime(preedit("ni")), Reaction::Nothing);
}

#[test]
fn a_disabled_ime_mid_composition_leaves_no_residue() {
    let mut composition = Composition::new();
    composition.ime(preedit("ni"));
    assert_eq!(composition.ime(Ime::Disabled), Reaction::Redraw);
    assert_eq!(composition.shown(), None);
    assert_eq!(composition.gate(), Gate::Open);
}

#[test]
fn a_dead_key_is_shown_and_resolves_once() {
    let mut composition = Composition::new();
    assert_eq!(composition.dead_key(Some('`')), Reaction::Redraw);
    assert_eq!(composition.gate(), Gate::Resolving);
    assert_eq!(
        composition.shown().map(|shown| shown.text().to_owned()),
        Some("`".to_owned())
    );
    assert_eq!(composition.resolve(), Reaction::Redraw);
    assert_eq!(composition.gate(), Gate::Open);
    assert_eq!(composition.shown(), None);
}

#[test]
fn a_dead_key_the_platform_does_not_name_still_gates_and_draws_nothing() {
    let mut composition = Composition::new();
    assert_eq!(composition.dead_key(None), Reaction::Nothing);
    assert_eq!(composition.gate(), Gate::Resolving);
    assert_eq!(composition.shown(), None);
}

#[test]
fn a_composition_that_starts_while_a_dead_key_waits_owns_the_gate() {
    let mut composition = Composition::new();
    composition.dead_key(Some('`'));
    composition.ime(preedit("ni"));
    assert_eq!(composition.gate(), Gate::Composing);
    composition.ime(Ime::Preedit(String::new(), None));
    assert_eq!(
        composition.gate(),
        Gate::Open,
        "the pending accent went with the composition that replaced it"
    );
}

#[test]
fn a_commit_arriving_on_a_pending_dead_key_clears_it() {
    let mut composition = Composition::new();
    composition.dead_key(Some('`'));
    assert_eq!(
        composition.ime(Ime::Commit("à".to_owned())),
        Reaction::Commit("à".to_owned())
    );
    assert_eq!(composition.gate(), Gate::Open);
    assert_eq!(composition.shown(), None);
}

#[test]
fn the_columns_of_a_preedit_are_width_aware() {
    let preedit = Preedit::new("你a", None);
    assert_eq!(
        preedit.columns(),
        vec![
            Column {
                text: "你".to_owned(),
                width: 2,
                continues: false,
            },
            Column {
                text: "a".to_owned(),
                width: 1,
                continues: false,
            }
        ]
    );
}

#[test]
fn a_zero_width_character_takes_no_column() {
    assert_eq!(Preedit::new("a\u{301}", None).columns().len(), 1);
}

#[test]
fn a_combining_mark_in_a_preedit_stays_on_the_letter_it_follows() {
    let columns = Preedit::new("e\u{301}x", None).columns();
    assert_eq!(
        columns,
        vec![
            Column {
                text: "e\u{301}".to_owned(),
                width: 1,
                continues: false,
            },
            Column {
                text: "x".to_owned(),
                width: 1,
                continues: false,
            }
        ]
    );
    assert_eq!(columns[0].character(), 'e');
}

#[test]
fn a_family_emoji_in_a_preedit_is_one_group_of_per_code_point_cells() {
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let columns = Preedit::new(format!("{}a", family), None).columns();
    let described: Vec<(&str, usize, bool)> = columns
        .iter()
        .map(|column| (column.text.as_str(), column.width, column.continues))
        .collect();
    assert_eq!(
        described,
        vec![
            ("\u{1f468}\u{200d}", 2, false),
            ("\u{1f469}\u{200d}", 2, true),
            ("\u{1f467}", 2, true),
            ("a", 1, false),
        ]
    );
}

#[test]
fn the_cursor_column_after_a_cluster_counts_each_code_point() {
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let end = family.len();
    assert_eq!(
        Preedit::new(family, Some((end, end))).cursor_column(),
        Some(6)
    );
    let accented = "e\u{301}";
    let end = accented.len();
    assert_eq!(
        Preedit::new(accented, Some((end, end))).cursor_column(),
        Some(1)
    );
}

#[test]
fn a_mark_with_nothing_before_it_is_dropped() {
    assert!(Preedit::new("\u{301}", None).columns().is_empty());
    assert_eq!(Preedit::new("\u{301}a", None).columns().len(), 1);
}

#[test]
fn the_cursor_column_counts_display_columns_not_bytes() {
    let text = "你好";
    assert_eq!(Preedit::new(text, Some((0, 0))).cursor_column(), Some(0));
    assert_eq!(Preedit::new(text, Some((3, 3))).cursor_column(), Some(2));
    assert_eq!(Preedit::new(text, Some((6, 6))).cursor_column(), Some(4));
    assert_eq!(
        Preedit::new(text, Some((99, 99))).cursor_column(),
        Some(4),
        "a cursor past the end of the text sits at its end"
    );
    assert_eq!(Preedit::new(text, None).cursor_column(), None);
}
