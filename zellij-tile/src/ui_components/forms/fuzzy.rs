//! Filtering a list of options by what the user typed.
//!
//! Five places in the builtin plugins hold their own copy of these five lines —
//! layout-manager's layout search, three separate lists inside session-manager,
//! strider's file search — each pairing `SkimMatcherV2::fuzzy_indices` with its
//! own sort and its own idea of what an empty query means. This is that idiom
//! with the answers pinned down: an empty query matches everything in order,
//! and ties are broken by the original position so a refilter cannot reshuffle
//! rows that scored the same.

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

/// One option that matched, and where it matched.
///
/// `indices` are **char** positions into the option, which is what
/// [`Text::color_indices`](crate::ui_components::Text::color_indices) wants, so
/// a caller can highlight the matched characters without converting anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Position of this option in the list that was filtered.
    pub index: usize,
    pub score: i64,
    pub indices: Vec<usize>,
}

/// The options matching `query`, best first.
///
/// An empty query is not a filter: every option comes back, in its original
/// order, with no highlighted characters. That is what makes a freshly opened
/// dropdown show its whole list rather than nothing.
pub fn fuzzy_filter(options: &[String], query: &str) -> Vec<Match> {
    if query.is_empty() {
        return options
            .iter()
            .enumerate()
            .map(|(index, _)| Match {
                index,
                score: 0,
                indices: vec![],
            })
            .collect();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches: Vec<Match> = options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| {
            matcher
                .fuzzy_indices(option, query)
                .map(|(score, indices)| Match {
                    index,
                    score,
                    indices,
                })
        })
        .collect();
    // Score descending, then original order. Without the second key, two
    // equally good matches can swap places between keystrokes and the row under
    // the highlight changes for no reason the user can see.
    matches.sort_by_key(|m| (std::cmp::Reverse(m.score), m.index));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;

    fn options(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_empty_query_keeps_every_option_in_order() {
        let options = options(&["one", "two", "three"]);
        let matches = fuzzy_filter(&options, "");
        assert_eq!(
            matches.iter().map(|m| m.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(matches.iter().all(|m| m.indices.is_empty()));
    }

    #[test]
    fn a_query_drops_the_options_it_does_not_match() {
        let options = options(&["default", "compact", "dev-three-pane"]);
        let matches = fuzzy_filter(&options, "dt");
        assert_eq!(
            matches.iter().map(|m| m.index).collect::<Vec<_>>().len(),
            2,
            "'compact' has no 'd', so only the other two match: {:?}",
            matches
        );
        assert!(
            matches.iter().all(|m| m.index != 1),
            "'compact' should not have matched"
        );
        assert!(
            matches[0].score >= matches[1].score,
            "the better match should come first"
        );
    }

    #[test]
    fn matched_characters_come_back_as_char_indices() {
        let options = options(&["compact"]);
        let matches = fuzzy_filter(&options, "cmp");
        assert_eq!(matches[0].indices, vec![0, 2, 3]);
    }

    /// A non-ASCII option indexed by byte would highlight the wrong characters,
    /// or panic when the caller sliced on it.
    #[test]
    fn indices_are_char_positions_not_byte_offsets() {
        let options = options(&["日本語 layout"]);
        let matches = fuzzy_filter(&options, "layout");
        let first = matches[0].indices[0];
        assert_eq!(
            first, 4,
            "expected the char index of 'l', got {} (byte offset is 10)",
            first
        );
    }

    #[test]
    fn nothing_matches_an_impossible_query() {
        assert!(fuzzy_filter(&options(&["one", "two"]), "zzz").is_empty());
    }

    /// Every caller uses `index` to look the option back up, so an index that
    /// does not point at the option that matched is a wrong row on screen.
    #[hegel::test]
    fn every_match_points_at_a_real_option(tc: hegel::TestCase) {
        let options: Vec<String> = tc.draw(generators::vecs(generators::text()).max_size(20));
        let query = tc.draw(generators::text());

        let matches = fuzzy_filter(&options, &query);
        assert!(matches.len() <= options.len());
        for m in &matches {
            let option = options
                .get(m.index)
                .unwrap_or_else(|| panic!("index {} is past the end", m.index));
            let char_count = option.chars().count();
            for &i in &m.indices {
                assert!(
                    i < char_count,
                    "highlight {} is past the end of {:?}",
                    i,
                    option
                );
            }
        }
    }

    #[hegel::test]
    fn matches_are_sorted_by_score_then_position(tc: hegel::TestCase) {
        let options: Vec<String> = tc.draw(generators::vecs(generators::text()).max_size(20));
        let query = tc.draw(generators::text());

        let matches = fuzzy_filter(&options, &query);
        for pair in matches.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            assert!(
                (std::cmp::Reverse(first.score), first.index)
                    <= (std::cmp::Reverse(second.score), second.index),
                "{:?} sorted before {:?}",
                first,
                second
            );
        }
    }
}
