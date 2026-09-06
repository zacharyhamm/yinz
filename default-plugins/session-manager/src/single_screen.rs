use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::path::PathBuf;
use std::time::Duration;
use zellij_tile::prelude::SessionName;

use crate::new_session_info::LayoutList;
use crate::ui::components::UnifiedResultsRenderCache;
use crate::ui::SessionUiInfo;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum SingleScreenMode {
    #[default]
    SearchAndSelect,
    SelectingLayout,
}

#[derive(Debug)]
pub enum UnifiedSearchResult {
    ActiveSession {
        score: i64,
        indices: Vec<usize>,
        session_name: SessionName,
        connected_users: usize,
        tab_count: usize,
        pane_count: usize,
        is_current_session: bool,
        creation_time: Duration,
    },
    ResurrectableSession {
        score: i64,
        indices: Vec<usize>,
        session_name: SessionName,
        ctime: Duration,
    },
}

/// Which kind of entry was under the cursor when the user pressed `Delete`.
/// Carries the session name so the host code can issue the right shim call
/// (`kill_sessions` vs `delete_dead_session`) without re-borrowing the
/// underlying `UnifiedSearchResult`.
#[derive(Debug, Clone)]
pub enum DeleteTarget {
    Active(SessionName),
    Resurrectable(SessionName),
}

impl UnifiedSearchResult {
    pub fn as_delete_target(&self) -> DeleteTarget {
        match self {
            UnifiedSearchResult::ActiveSession { session_name, .. } => {
                DeleteTarget::Active(session_name.clone())
            },
            UnifiedSearchResult::ResurrectableSession { session_name, .. } => {
                DeleteTarget::Resurrectable(session_name.clone())
            },
        }
    }
    pub fn session_name(&self) -> &str {
        match self {
            UnifiedSearchResult::ActiveSession { session_name, .. } => session_name.as_str(),
            UnifiedSearchResult::ResurrectableSession { session_name, .. } => session_name.as_str(),
        }
    }
    fn score(&self) -> i64 {
        match self {
            UnifiedSearchResult::ActiveSession { score, .. } => *score,
            UnifiedSearchResult::ResurrectableSession { score, .. } => *score,
        }
    }
    /// Ordering by type (active before resurrectable), then by creation time ascending
    /// (smaller elapsed duration = more recently created = appears first).
    fn cmp_by_type_then_recency(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (
                UnifiedSearchResult::ActiveSession {
                    creation_time: ct_a,
                    ..
                },
                UnifiedSearchResult::ActiveSession {
                    creation_time: ct_b,
                    ..
                },
            ) => ct_a.cmp(ct_b),
            (
                UnifiedSearchResult::ResurrectableSession { ctime: ct_a, .. },
                UnifiedSearchResult::ResurrectableSession { ctime: ct_b, .. },
            ) => ct_a.cmp(ct_b),
            (
                UnifiedSearchResult::ActiveSession { .. },
                UnifiedSearchResult::ResurrectableSession { .. },
            ) => std::cmp::Ordering::Less,
            (
                UnifiedSearchResult::ResurrectableSession { .. },
                UnifiedSearchResult::ActiveSession { .. },
            ) => std::cmp::Ordering::Greater,
        }
    }
}

#[derive(Default)]
pub struct SingleScreenState {
    pub search_term: String,
    pub unified_results: Vec<UnifiedSearchResult>,
    pub selected_index: Option<usize>,
    pub mode: SingleScreenMode,
    pub layout_list: LayoutList,
    pub new_session_folder: Option<PathBuf>,
    pub is_welcome_screen: bool,
    pub render_cache: UnifiedResultsRenderCache,
    fuzzy_matcher: Option<SkimMatcherV2>,
}

impl std::fmt::Debug for SingleScreenState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleScreenState")
            .field("search_term", &self.search_term)
            .field("selected_index", &self.selected_index)
            .field("mode", &self.mode)
            .field("result_count", &self.unified_results.len())
            .finish()
    }
}

impl SingleScreenState {
    pub fn update_search_term(
        &mut self,
        active_sessions: &[SessionUiInfo],
        resurrectable_sessions: &[(SessionName, Duration)],
    ) {
        let previously_selected_name = self.previously_selected_name();
        self.selected_index = None;

        if self.search_term.is_empty() {
            self.unified_results =
                Self::collect_all_sessions(active_sessions, resurrectable_sessions);
            self.unified_results
                .sort_by(|a, b| a.cmp_by_type_then_recency(b));
        } else {
            self.unified_results =
                self.collect_fuzzy_matched_sessions(active_sessions, resurrectable_sessions);
            self.unified_results.sort_by(|a, b| {
                let score_cmp = b.score().cmp(&a.score());
                if score_cmp != std::cmp::Ordering::Equal {
                    return score_cmp;
                }
                a.cmp_by_type_then_recency(b)
            });
        }

        self.render_cache.rebuild(&self.unified_results);
        self.restore_selection(previously_selected_name);
    }

    fn previously_selected_name(&self) -> Option<String> {
        self.selected_index
            .and_then(|i| self.unified_results.get(i))
            .map(|r| r.session_name().to_owned())
    }

    fn restore_selection(&mut self, previously_selected_name: Option<String>) {
        if let Some(prev_name) = previously_selected_name {
            self.selected_index = self
                .unified_results
                .iter()
                .position(|r| r.session_name() == prev_name);
        }
    }

    /// After deleting an entry, ensure the cursor stays on a sensible row.
    /// `update_search_term` already keeps the selection on the same entry by
    /// name if it still exists; this method picks up the case where the
    /// deleted entry **was** the selected one, by falling back to the same
    /// numeric row clamped to the new `unified_results` length.
    pub fn restore_selection_after_delete(&mut self, previous_index: Option<usize>) {
        if self.selected_index.is_some() {
            return;
        }
        self.selected_index = crate::session_list::clamp_index_after_delete(
            previous_index,
            self.unified_results.len(),
        );
    }

    fn collect_all_sessions(
        active_sessions: &[SessionUiInfo],
        resurrectable_sessions: &[(SessionName, Duration)],
    ) -> Vec<UnifiedSearchResult> {
        let mut results = Vec::new();
        for session in active_sessions {
            results.push(Self::active_session_to_result(session, 0, vec![]));
        }
        for (name, ctime) in resurrectable_sessions {
            results.push(UnifiedSearchResult::ResurrectableSession {
                score: 0,
                indices: vec![],
                session_name: name.clone(),
                ctime: *ctime,
            });
        }
        results
    }

    fn collect_fuzzy_matched_sessions(
        &mut self,
        active_sessions: &[SessionUiInfo],
        resurrectable_sessions: &[(SessionName, Duration)],
    ) -> Vec<UnifiedSearchResult> {
        let matcher = self
            .fuzzy_matcher
            .get_or_insert_with(|| SkimMatcherV2::default().use_cache(true));
        let mut results = Vec::new();

        for session in active_sessions {
            if let Some((score, indices)) =
                matcher.fuzzy_indices(session.name.as_str(), &self.search_term)
            {
                results.push(Self::active_session_to_result(session, score, indices));
            }
        }
        for (name, ctime) in resurrectable_sessions {
            if let Some((score, indices)) = matcher.fuzzy_indices(name.as_str(), &self.search_term)
            {
                results.push(UnifiedSearchResult::ResurrectableSession {
                    score,
                    indices,
                    session_name: name.clone(),
                    ctime: *ctime,
                });
            }
        }
        results
    }

    fn active_session_to_result(
        session: &SessionUiInfo,
        score: i64,
        indices: Vec<usize>,
    ) -> UnifiedSearchResult {
        UnifiedSearchResult::ActiveSession {
            score,
            indices,
            session_name: session.name.clone(),
            connected_users: session.connected_users,
            tab_count: session.tabs.len(),
            pane_count: session.tabs.iter().fold(0, |acc, t| acc + t.panes.len()),
            is_current_session: session.is_current_session,
            creation_time: session.creation_time,
        }
    }

    fn is_current_session(&self, index: usize) -> bool {
        matches!(
            self.unified_results.get(index),
            Some(UnifiedSearchResult::ActiveSession {
                is_current_session: true,
                ..
            })
        )
    }

    fn next_selectable_down(&self, from: usize) -> Option<usize> {
        let len = self.unified_results.len();
        for offset in 1..=len {
            let candidate = (from + offset) % len;
            if !self.is_current_session(candidate) {
                return Some(candidate);
            }
        }
        None
    }

    fn next_selectable_up(&self, from: usize) -> Option<usize> {
        let len = self.unified_results.len();
        for offset in 1..=len {
            let candidate = (from + len - offset) % len;
            if !self.is_current_session(candidate) {
                return Some(candidate);
            }
        }
        None
    }

    pub fn move_selection_down(&mut self) {
        if self.unified_results.is_empty() {
            return;
        }
        match self.selected_index {
            None => {
                self.selected_index =
                    self.next_selectable_down(self.unified_results.len().saturating_sub(1));
            },
            Some(i) => {
                self.selected_index = self.next_selectable_down(i);
            },
        }
    }

    pub fn move_selection_up(&mut self) {
        if self.unified_results.is_empty() {
            return;
        }
        match self.selected_index {
            None => {
                self.selected_index = self.next_selectable_up(0);
            },
            Some(i) => {
                self.selected_index = self.next_selectable_up(i);
            },
        }
    }

    pub fn tab_complete(
        &mut self,
        active_sessions: &[SessionUiInfo],
        resurrectable_sessions: &[(SessionName, Duration)],
    ) {
        let first_non_current = self.unified_results.iter().find(|r| {
            !matches!(
                r,
                UnifiedSearchResult::ActiveSession {
                    is_current_session: true,
                    ..
                }
            )
        });
        if let Some(result) = first_non_current {
            let name = result.session_name().to_owned();
            self.search_term = name;
            self.update_search_term(active_sessions, resurrectable_sessions);
        }
    }

    pub fn get_selected_result(&self) -> Option<&UnifiedSearchResult> {
        self.selected_index
            .and_then(|i| self.unified_results.get(i))
    }

    pub fn transition_to_layout_selection(&mut self) {
        self.mode = SingleScreenMode::SelectingLayout;
    }

    pub fn transition_to_search(&mut self) {
        self.mode = SingleScreenMode::SearchAndSelect;
        self.layout_list.layout_search_term.clear();
        self.layout_list.clear_selection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_active_session(
        name: &str,
        tabs: usize,
        panes_per_tab: usize,
        connected: usize,
        is_current: bool,
        creation_secs: u64,
    ) -> SessionUiInfo {
        SessionUiInfo {
            name: name.to_string().into(),
            tabs: (0..tabs)
                .map(|i| crate::ui::TabUiInfo {
                    name: format!("tab-{}", i),
                    panes: (0..panes_per_tab)
                        .map(|j| crate::ui::PaneUiInfo {
                            name: format!("pane-{}", j),
                            exit_code: None,
                            pane_id: zellij_tile::prelude::PaneId::from_legacy(j as u32, false),
                        })
                        .collect(),
                    position: zellij_tile::prelude::TabPosition::new(i),
                })
                .collect(),
            connected_users: connected,
            is_current_session: is_current,
            creation_time: Duration::from_secs(creation_secs),
        }
    }

    fn make_resurrectable(name: &str, ctime_secs: u64) -> (SessionName, Duration) {
        (SessionName::new(name), Duration::from_secs(ctime_secs))
    }

    // ---------------------------------------------------------------
    // Section 1: Session Display and Sorting
    // ---------------------------------------------------------------

    #[test]
    fn test_1_5_pane_counts_reflect_tab_structure() {
        let mut state = SingleScreenState {
            search_term: String::new(),
            ..Default::default()
        };
        let active = vec![make_active_session("sess", 3, 2, 1, false, 100)];
        state.update_search_term(&active, &[]);
        assert_eq!(state.unified_results.len(), 1);
        match &state.unified_results[0] {
            UnifiedSearchResult::ActiveSession {
                tab_count,
                pane_count,
                ..
            } => {
                assert_eq!(*tab_count, 3);
                assert_eq!(*pane_count, 6);
            },
            _ => panic!("Expected ActiveSession"),
        }
    }

    // ---------------------------------------------------------------
    // Section 2: Search and Fuzzy Matching
    // ---------------------------------------------------------------

    #[test]
    fn test_2_2_typing_filters_results() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("alpha", 1, 1, 1, false, 100),
            make_active_session("beta", 1, 1, 1, false, 200),
            make_active_session("gamma", 1, 1, 1, false, 300),
        ];
        state.search_term = "alp".to_string();
        state.update_search_term(&active, &[]);
        assert_eq!(state.unified_results.len(), 1);
        assert_eq!(state.unified_results[0].session_name(), "alpha");
        match &state.unified_results[0] {
            UnifiedSearchResult::ActiveSession { score, indices, .. } => {
                assert!(*score > 0);
                assert!(!indices.is_empty());
            },
            _ => panic!("Expected ActiveSession"),
        }
    }

    #[test]
    fn test_2_3_fuzzy_matching_non_contiguous() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("my-project", 1, 1, 1, false, 100),
            make_active_session("mapping", 1, 1, 1, false, 200),
        ];
        state.search_term = "mprj".to_string();
        state.update_search_term(&active, &[]);
        // At least "my-project" should match (m, p, r, j fuzzy)
        let has_my_project = state
            .unified_results
            .iter()
            .any(|r| r.session_name() == "my-project");
        assert!(has_my_project);
    }

    #[test]
    fn test_2_4_score_based_ordering() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("abc", 1, 1, 1, false, 100),
            make_active_session("xabc", 1, 1, 1, false, 200),
            make_active_session("xxabc", 1, 1, 1, false, 300),
        ];
        state.search_term = "abc".to_string();
        state.update_search_term(&active, &[]);
        assert!(!state.unified_results.is_empty());
        // "abc" (exact match) should have the highest score and appear first
        assert_eq!(state.unified_results[0].session_name(), "abc");
        // Verify descending score order
        let scores: Vec<i64> = state
            .unified_results
            .iter()
            .map(|r| match r {
                UnifiedSearchResult::ActiveSession { score, .. } => *score,
                UnifiedSearchResult::ResurrectableSession { score, .. } => *score,
            })
            .collect();
        for i in 0..scores.len().saturating_sub(1) {
            assert!(scores[i] >= scores[i + 1]);
        }
    }

    #[test]
    fn test_2_7_search_term_matches_nothing() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("alpha", 1, 1, 1, false, 100),
            make_active_session("beta", 1, 1, 1, false, 200),
        ];
        state.search_term = "zzz".to_string();
        state.update_search_term(&active, &[]);
        assert!(state.unified_results.is_empty());
    }

    // ---------------------------------------------------------------
    // Section 4: Tab Completion
    // ---------------------------------------------------------------

    #[test]
    fn test_4_1_basic_tab_completion() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("mine", 1, 1, 1, true, 100),
            make_active_session("alpha", 1, 1, 1, false, 200),
            make_active_session("beta", 1, 1, 1, false, 300),
        ];
        state.search_term = String::new();
        state.update_search_term(&active, &[]);
        state.tab_complete(&active, &[]);
        // First non-current in sorted results should be "alpha" or whichever is first
        // With empty search, sorted by creation_time ascending: mine(100), alpha(200), beta(300)
        // First non-current is "alpha"
        assert_eq!(state.search_term, "alpha");
    }

    #[test]
    fn test_4_2_tab_completion_only_current_session_noop() {
        let mut state = SingleScreenState::default();
        let active = vec![make_active_session("mine", 1, 1, 1, true, 100)];
        state.search_term = String::new();
        state.update_search_term(&active, &[]);
        state.tab_complete(&active, &[]);
        assert_eq!(state.search_term, "");
    }

    #[test]
    fn test_4_3_tab_completion_picks_resurrectable_if_no_non_current_active() {
        let mut state = SingleScreenState::default();
        let active = vec![make_active_session("mine", 1, 1, 1, true, 100)];
        let resurrectable = vec![make_resurrectable("old-session", 100)];
        state.search_term = String::new();
        state.update_search_term(&active, &resurrectable);
        state.tab_complete(&active, &resurrectable);
        assert_eq!(state.search_term, "old-session");
    }

    #[test]
    fn test_4_4_tab_completion_with_partial_search_term() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("mine", 1, 1, 1, true, 100),
            make_active_session("alpha", 1, 1, 1, false, 200),
            make_active_session("beta", 1, 1, 1, false, 300),
        ];
        let resurrectable: Vec<(SessionName, Duration)> = vec![];
        state.search_term = "be".to_string();
        state.update_search_term(&active, &resurrectable);
        state.tab_complete(&active, &resurrectable);
        assert_eq!(state.search_term, "beta");
    }

    #[test]
    fn test_4_5_tab_completion_with_empty_results_noop() {
        let mut state = SingleScreenState::default();
        let active = vec![
            make_active_session("alpha", 1, 1, 1, false, 100),
            make_active_session("beta", 1, 1, 1, false, 200),
        ];
        state.search_term = "zzz".to_string();
        state.update_search_term(&active, &[]);
        // Results should be empty — no matches
        assert!(state.unified_results.is_empty());
        state.tab_complete(&active, &[]);
        assert_eq!(state.search_term, "zzz"); // unchanged
    }

    // ---------------------------------------------------------------
    // Section 5: Mode Transitions
    // ---------------------------------------------------------------

    #[test]
    fn test_5_1_transition_to_layout_selection() {
        let mut state = SingleScreenState {
            mode: SingleScreenMode::SearchAndSelect,
            search_term: "my-session".to_string(),
            ..Default::default()
        };
        state.transition_to_layout_selection();
        assert_eq!(state.mode, SingleScreenMode::SelectingLayout);
        assert_eq!(state.search_term, "my-session"); // preserved
    }

    #[test]
    fn test_5_2_transition_back_to_search() {
        let mut state = SingleScreenState {
            mode: SingleScreenMode::SelectingLayout,
            ..Default::default()
        };
        state.layout_list.layout_search_term = "some-layout".to_string();
        state.layout_list.selected_layout_index = 3;
        state.transition_to_search();
        assert_eq!(state.mode, SingleScreenMode::SearchAndSelect);
        assert_eq!(state.layout_list.layout_search_term, "");
        assert_eq!(state.layout_list.selected_layout_index, 0);
    }

    #[test]
    fn test_5_3_round_trip_transition_preserves_search_term() {
        let mut state = SingleScreenState {
            search_term: "my-new-session".to_string(),
            mode: SingleScreenMode::SearchAndSelect,
            ..Default::default()
        };
        state.transition_to_layout_selection();
        assert_eq!(state.mode, SingleScreenMode::SelectingLayout);
        state.transition_to_search();
        assert_eq!(state.search_term, "my-new-session");
        assert_eq!(state.mode, SingleScreenMode::SearchAndSelect);
    }
    mod props {
        use super::*;
        use hegel::generators;

        /// A results list built directly, as the navigation tests do: one `is_current`
        /// flag per entry, with distinct names, so every index is identifiable.
        #[hegel::composite]
        fn any_current_flags(tc: &hegel::TestCase) -> Vec<bool> {
            // capped for allocation sanity; the empty list stays reachable
            tc.draw(generators::vecs(generators::booleans()).max_size(6))
        }

        fn build_results(flags: &[bool]) -> Vec<UnifiedSearchResult> {
            flags
                .iter()
                .copied()
                .enumerate()
                .map(|(i, is_current)| UnifiedSearchResult::ActiveSession {
                    score: 0,
                    indices: vec![],
                    session_name: SessionName::new(format!("sess-{}", i)),
                    connected_users: 1,
                    tab_count: 1,
                    pane_count: 1,
                    is_current_session: is_current,
                    creation_time: Duration::from_secs(i as u64 * 100),
                })
                .collect()
        }

        fn selectable_indices(results: &[UnifiedSearchResult]) -> Vec<usize> {
            results
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    !matches!(
                        r,
                        UnifiedSearchResult::ActiveSession {
                            is_current_session: true,
                            ..
                        }
                    )
                })
                .map(|(i, _)| i)
                .collect()
        }

        fn state_with(flags: &[bool]) -> SingleScreenState {
            SingleScreenState {
                unified_results: build_results(flags),
                ..Default::default()
            }
        }

        /// Replaces `test_3_1_down_from_none_selects_first_non_current`,
        /// `test_3_3_up_from_none_selects_last_non_current`,
        /// `test_3_5_navigation_with_only_current_session` and
        /// `test_3_6_navigation_with_empty_results`.
        #[hegel::test]
        fn navigating_from_no_selection_lands_on_the_first_or_last_selectable(tc: hegel::TestCase) {
            let flags = tc.draw(any_current_flags());
            let selectable = selectable_indices(&build_results(&flags));

            let mut down = state_with(&flags);
            down.selected_index = None;
            down.move_selection_down();
            assert_eq!(
                down.selected_index,
                selectable.first().copied(),
                "down from no selection, selectable: {:?}",
                selectable
            );

            let mut up = state_with(&flags);
            up.selected_index = None;
            up.move_selection_up();
            assert_eq!(
                up.selected_index,
                selectable.last().copied(),
                "up from no selection, selectable: {:?}",
                selectable
            );
        }

        /// The wrap-around law that `test_3_2_down_from_last_wraps_to_first_non_current`,
        /// `test_3_4_up_from_first_wraps_to_last_non_current`,
        /// `test_3_7_sequential_down_visits_all_non_current` and
        /// `test_3_8_sequential_up_visits_all_non_current_in_reverse` each write out for
        /// one starting index.
        #[hegel::test]
        fn navigation_cycles_through_every_selectable_entry(tc: hegel::TestCase) {
            let flags = tc.draw(any_current_flags());
            let selectable = selectable_indices(&build_results(&flags));
            if selectable.is_empty() {
                return;
            }
            // sweep every starting index rather than drawing one
            for start in selectable.iter().copied() {
                let mut state = state_with(&flags);
                state.selected_index = Some(start);

                // one full cycle of `down` visits every selectable index, in order, and
                // returns to where it started
                let mut visited = vec![];
                for _ in 0..selectable.len() {
                    state.move_selection_down();
                    let selected = state.selected_index.expect("selection was lost");
                    assert!(
                        selectable.contains(&selected),
                        "landed on the current session at {} (selectable: {:?})",
                        selected,
                        selectable
                    );
                    visited.push(selected);
                }
                assert_eq!(
                    state.selected_index,
                    Some(start),
                    "a full cycle of {} downs from {} did not return (selectable: {:?})",
                    selectable.len(),
                    start,
                    selectable
                );
                let mut sorted_visited = visited.clone();
                sorted_visited.sort_unstable();
                assert_eq!(
                    sorted_visited, selectable,
                    "a full cycle from {} visited {:?}",
                    start, visited
                );

                // and `up` undoes `down`
                state.move_selection_down();
                state.move_selection_up();
                assert_eq!(
                    state.selected_index,
                    Some(start),
                    "up did not undo down from {} (selectable: {:?})",
                    start,
                    selectable
                );
            }
        }

        /// Replaces `test_3_9_get_selected_result_returns_correct_result` and
        /// `test_3_10_get_selected_result_returns_none_when_nothing_selected`.
        #[hegel::test]
        fn the_selected_result_is_the_one_at_the_selected_index(tc: hegel::TestCase) {
            let flags = tc.draw(any_current_flags());
            let mut state = state_with(&flags);
            // sweep every index plus "nothing selected"
            for selected in (0..flags.len()).map(Some).chain(std::iter::once(None)) {
                state.selected_index = selected;
                assert_eq!(
                    state
                        .get_selected_result()
                        .map(|r| r.session_name().to_owned()),
                    selected.map(|i| format!("sess-{}", i)),
                    "selected_index {:?} of {} results",
                    selected,
                    flags.len()
                );
            }
        }

        /// The session lists the state machine searches over, mirrored so the invariants
        /// can be stated without reaching into the plugin's own bookkeeping.
        struct SingleScreenModel {
            real: SingleScreenState,
            active: Vec<SessionUiInfo>,
            resurrectable: Vec<(SessionName, Duration)>,
        }

        /// A tiny name pool, so that added sessions collide with existing ones and the
        /// fuzzy search terms below actually match something.
        fn any_session_name(tc: &hegel::TestCase) -> String {
            tc.draw(generators::sampled_from(vec![
                "alpha".to_owned(),
                "beta".to_owned(),
                "gamma".to_owned(),
                "alpaca".to_owned(),
            ]))
        }

        impl SingleScreenModel {
            /// The order `update_search_term` is documented to produce for an empty
            /// search term: actives before resurrectables, each by ascending creation
            /// time, ties keeping input order (the production sort is stable).
            fn expected_order_for_empty_search(&self) -> Vec<String> {
                let mut actives: Vec<&SessionUiInfo> = self.active.iter().collect();
                actives.sort_by_key(|s| s.creation_time);
                let mut resurrectable = self.resurrectable.clone();
                resurrectable.sort_by_key(|(_, ctime)| *ctime);
                actives
                    .into_iter()
                    .map(|s| s.name.to_string())
                    .chain(
                        resurrectable
                            .into_iter()
                            .map(|(name, _)| name.into_string()),
                    )
                    .collect()
            }

            fn selected_name(&self) -> Option<String> {
                self.real
                    .get_selected_result()
                    .map(|r| r.session_name().to_owned())
            }
        }

        #[hegel::state_machine]
        impl SingleScreenModel {
            #[rule]
            fn add_active_session(&mut self, tc: hegel::TestCase) {
                let name = any_session_name(&tc);
                if self.active.iter().any(|s| s.name.as_str() == name) {
                    return;
                }
                let creation_secs = tc.draw(generators::integers::<u64>().max_value(3));
                let is_current = self.active.iter().all(|s| !s.is_current_session)
                    && tc.draw(generators::booleans());
                self.active.push(make_active_session(
                    &name,
                    1,
                    1,
                    1,
                    is_current,
                    creation_secs,
                ));
                // the plugin re-runs the search whenever new session info arrives
                self.real
                    .update_search_term(&self.active, &self.resurrectable);
            }

            #[rule]
            fn add_resurrectable_session(&mut self, tc: hegel::TestCase) {
                // deliberately allowed to share a name with an active session: that is
                // the tie the active-before-resurrectable rule exists to break
                let name = any_session_name(&tc);
                if self.resurrectable.iter().any(|(n, _)| n.as_str() == name) {
                    return;
                }
                let ctime_secs = tc.draw(generators::integers::<u64>().max_value(3));
                self.resurrectable
                    .push(make_resurrectable(&name, ctime_secs));
                self.real
                    .update_search_term(&self.active, &self.resurrectable);
            }

            #[rule]
            fn search(&mut self, tc: hegel::TestCase) {
                self.real.search_term = tc.draw(generators::sampled_from(vec![
                    String::new(),
                    "a".to_owned(),
                    "al".to_owned(),
                    "zzz".to_owned(),
                ]));
                // a selection survives a search only by name - this is exactly what
                // `test_2_8_selection_preserved_across_search_updates` checks once
                let previously_selected = self.selected_name();
                self.real
                    .update_search_term(&self.active, &self.resurrectable);
                match previously_selected {
                    Some(name) => assert_eq!(
                        self.selected_name(),
                        self.real
                            .unified_results
                            .iter()
                            .any(|r| r.session_name() == name)
                            .then_some(name.clone()),
                        "selection {:?} was not carried across the search",
                        name
                    ),
                    None => assert_eq!(
                        self.selected_name(),
                        None,
                        "a search invented a selection out of nothing"
                    ),
                }
            }

            #[rule]
            fn move_selection_down(&mut self, _: hegel::TestCase) {
                self.real.move_selection_down();
            }

            #[rule]
            fn move_selection_up(&mut self, _: hegel::TestCase) {
                self.real.move_selection_up();
            }

            #[rule]
            fn tab_complete(&mut self, _: hegel::TestCase) {
                self.real.tab_complete(&self.active, &self.resurrectable);
            }

            #[rule]
            fn delete_selected_session(&mut self, _: hegel::TestCase) {
                let Some(name) = self.selected_name() else {
                    return;
                };
                let previous_index = self.real.selected_index;
                self.active.retain(|s| s.name.as_str() != name);
                self.resurrectable.retain(|(n, _)| n.as_str() != name);
                self.real
                    .update_search_term(&self.active, &self.resurrectable);
                self.real.restore_selection_after_delete(previous_index);
            }

            #[invariant]
            fn the_selection_and_the_results_stay_consistent(&self, _: hegel::TestCase) {
                if let Some(selected_index) = self.real.selected_index {
                    assert!(
                        selected_index < self.real.unified_results.len(),
                        "selected index {} is out of range for {} results",
                        selected_index,
                        self.real.unified_results.len()
                    );
                }
                let known: Vec<&str> = self
                    .active
                    .iter()
                    .map(|s| s.name.as_str())
                    .chain(self.resurrectable.iter().map(|(name, _)| name.as_str()))
                    .collect();
                for result in &self.real.unified_results {
                    assert!(
                        known.contains(&result.session_name()),
                        "{:?} is not one of the sessions {:?}",
                        result.session_name(),
                        known
                    );
                }
                if self.real.search_term.is_empty() {
                    let rendered: Vec<String> = self
                        .real
                        .unified_results
                        .iter()
                        .map(|r| r.session_name().to_owned())
                        .collect();
                    assert_eq!(
                        rendered,
                        self.expected_order_for_empty_search(),
                        "an empty search term did not show every session in order"
                    );
                } else {
                    // Under a search term the order is by score descending, then the same
                    // type-and-recency tie-break. Skim's scores are read back off the
                    // results rather than modelled - the law under test is the ordering,
                    // not the scoring.
                    let mut expected: Vec<&UnifiedSearchResult> =
                        self.real.unified_results.iter().collect();
                    expected.sort_by(|a, b| {
                        b.score()
                            .cmp(&a.score())
                            .then_with(|| a.cmp_by_type_then_recency(b))
                    });
                    let key = |r: &UnifiedSearchResult| {
                        (
                            r.session_name().to_owned(),
                            matches!(r, UnifiedSearchResult::ActiveSession { .. }),
                        )
                    };
                    assert_eq!(
                        self.real
                            .unified_results
                            .iter()
                            .map(key)
                            .collect::<Vec<_>>(),
                        expected.into_iter().map(key).collect::<Vec<_>>(),
                        "search results for {:?} are not in score-then-recency order",
                        self.real.search_term
                    );
                }
            }
        }

        /// Replaces `test_1_1_active_sessions_sorted_by_recency`,
        /// `test_1_2_resurrectable_sessions_sorted_by_recency`,
        /// `test_1_3_mixed_active_and_resurrectable_active_first`,
        /// `test_1_4_current_session_included_in_results`, `test_1_6_empty_sessions_list`,
        /// `test_2_1_empty_search_term_shows_all`, `test_2_5_tie_breaking_active_before_resurrectable`,
        /// `test_2_6_tie_breaking_more_recent_first_at_equal_score_and_type` and
        /// `test_2_8_selection_preserved_across_search_updates`.
        #[hegel::test]
        fn the_selection_follows_its_session_through_any_sequence_of_operations(
            tc: hegel::TestCase,
        ) {
            let machine = SingleScreenModel {
                real: SingleScreenState::default(),
                active: vec![],
                resurrectable: vec![],
            };
            hegel::stateful::run(machine, tc);
        }
    }
}
