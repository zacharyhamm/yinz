use super::*;
use std::sync::Arc;

fn prompts(texts: &[&str]) -> Vec<PromptRecord> {
    texts
        .iter()
        .map(|text| PromptRecord {
            text: Arc::from(*text),
            description: String::new(),
        })
        .collect()
}

fn active() -> AtomicBool {
    AtomicBool::new(true)
}

#[test]
fn empty_query_lists_newest_first_up_to_the_cap() {
    let many = vec!["prompt"; MAX_RESULTS + 5];
    let found = matches(&prompts(&many), "  ", &active()).unwrap();
    assert_eq!(found, (0..MAX_RESULTS).collect::<Vec<_>>());
}

#[test]
fn ranks_by_score_and_breaks_ties_by_recency() {
    let list = prompts(&[
        "unrelated",
        "fix the build",
        "f  i  x scattered widely",
        "fix the tests",
    ]);
    let found = matches(&list, "fix", &active()).unwrap();
    assert_eq!(&found[..2], [1, 3]);
    assert_eq!(found[2], 2);
    assert!(!found.contains(&0));
}

#[test]
fn every_word_must_match_and_case_is_ignored() {
    let list = prompts(&["Open the PR", "open a ticket"]);
    assert_eq!(matches(&list, "open pr", &active()).unwrap(), [0]);
}

#[test]
fn searches_past_the_first_line() {
    let list = prompts(&["first line\nsecond line mentions attestation"]);
    assert_eq!(matches(&list, "attestation", &active()).unwrap(), [0]);
}

#[test]
fn keeps_only_the_best_results_when_over_the_cap() {
    let mut texts = vec!["n e e d l e"; MAX_RESULTS + 5];
    texts.push("needle");
    let list = prompts(&texts);
    let found = matches(&list, "needle", &active()).unwrap();
    assert_eq!(found.len(), MAX_RESULTS);
    assert_eq!(found[0], MAX_RESULTS + 5);
}

#[test]
fn a_superseded_search_returns_nothing() {
    let list = prompts(&["needle"]);
    assert_eq!(matches(&list, "needle", &AtomicBool::new(false)), None);
}

#[test]
fn oversized_atoms_complete_with_no_matches() {
    for query in ["a".repeat(2_600), "界".repeat(1_025)] {
        let list = prompts(&[&query, "normal query"]);
        assert_eq!(matches(&list, &query, &active()), Some(vec![]));
        assert_eq!(matches(&list, "normal", &active()), Some(vec![1]));
    }
}

#[test]
fn the_atom_limit_counts_unicode_characters_and_allows_the_boundary() {
    for query in ["a".repeat(1_024), "界".repeat(1_024)] {
        assert_eq!(
            matches(&prompts(&[&query]), &query, &active()),
            Some(vec![0])
        );
    }
}

#[test]
fn an_oversized_atom_rejects_the_whole_multiword_query() {
    let query = format!("fix {}", "a".repeat(1_025));
    assert_eq!(
        matches(&prompts(&[&query]), &query, &active()),
        Some(vec![])
    );
    // Many short atoms remain valid even when the whole query is over the atom limit.
    let query = "fix ".repeat(300);
    assert_eq!(
        matches(&prompts(&["fix"]), &query, &active()),
        Some(vec![0])
    );
}

#[test]
fn rows_are_one_line_literal_and_truncated() {
    let long = "word ".repeat(100);
    let list = prompts(&["*bold*\n\n  second line", &long]);
    let section = section(&list, &[1, 0, 9], "none");
    assert!(section.semantic);
    assert_eq!(section.status, None);
    assert_eq!(section.entries.len(), 2);
    assert_eq!(section.entries[0].id, format!("{ENTRY_PREFIX}1"));
    assert!(section.entries[0].label.chars().count() <= LABEL_CHARS);
    assert_eq!(
        section.entries[1].label,
        escape_literal("*bold* second line")
    );
}

#[test]
fn a_word_ending_exactly_on_the_limit_still_ends_the_line() {
    assert_eq!(one_line("abcd efgh ijkl", 4), "abcd");
}

#[test]
fn an_empty_result_carries_the_status() {
    let section = section(&[], &[], "No matching prompts");
    assert_eq!(section.status.as_deref(), Some("No matching prompts"));
    assert!(section.entries.is_empty());
}

#[test]
fn seeds_short_drafts_flattened_and_skips_long_ones() {
    assert_eq!(seed_query("fix\n  the build"), "fix the build");
    assert_eq!(seed_query(&"x".repeat(MAX_SEED_CHARS + 1)), "");
}
