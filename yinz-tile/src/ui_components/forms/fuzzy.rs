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
#[path = "unit/fuzzy.rs"]
mod tests;
