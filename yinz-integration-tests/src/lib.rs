#![cfg(unix)]

pub mod client_screen;
pub mod fake_client_os_api;
pub mod fake_pty;
pub mod fake_server_os_api;
pub mod harness;
pub mod keys;
pub mod kitty_protocol;
pub mod nested;
pub mod runner;
pub mod test_env;

pub use client_screen::{
    col, AnsiCode, CellStyle, ClientScreen, Coord, CoordBuilder, CursorPosition, GridSnapshot,
};
pub use fake_pty::{FakePtyHandle, WarpFailure};
pub use harness::{
    claim_first_terminal_and_wait_for_prompt, split_down_and_wait_for_prompt,
    split_right_and_wait_for_prompt, start_yinz, tab_bar_shows_no_lone_pane_id, PROMPT,
    TERMINAL_SIZE,
};
pub use nested::{
    composite_contains_settled_guest_grid, guest_ascended_bar_settled, host_descended_bar_settled,
    host_descended_bar_with_ascend_keys_settled, wait_for_settled_composite,
    NestedDepthThreeHarness, NestedHarness,
};
pub use runner::{assert_same_rendered_grid, normalized, TestClient, TestRunner, TestSession};
pub use yinz_types::data::LayoutInfo;
pub use yinz_types::pane_size::Size;

use std::time::Duration;

/// The older name of [`yinz_test_support::TIMEOUT_ENV_VAR`], still honoured when the shared name
/// is unset.
const LEGACY_TIMEOUT_ENV_VAR: &str = "YINZ_TEST_TIMEOUT_MS";

/// How long one harness wait lasts by default.
///
/// Startup includes database fsyncs that can exceed ten seconds under parallel load, so this stays
/// generous. It must also stay under the default nextest profile's `terminate-after` (60 s in
/// `.config/nextest.toml`): a wait as long as that cut-off loses the race, nextest kills the test,
/// and it reports a bare TIMEOUT instead of the wait's panic with the grid and the log tail.
const DEFAULT_WAIT: Duration = yinz_test_support::DEFAULT_WAIT;

/// Where an Agents panel list row names `kind`: just after the row's status glyph, which tells
/// the row apart from the history column's "Task: claude" and from pane titles.
pub fn agent_row_column(line: &str, kind: &str) -> Option<usize> {
    line.match_indices(&format!(" {kind} "))
        .map(|(index, _)| index)
        .find(|&index| {
            line[..index]
                .chars()
                .next_back()
                .is_some_and(|glyph| "●○!■×?".contains(glyph))
        })
}

/// Whether any line of `text` is an Agents panel row for `kind`.
pub fn lists_agent(text: &str, kind: &str) -> bool {
    text.lines()
        .any(|line| agent_row_column(line, kind).is_some())
}

/// The deadline of every harness wait: grid, raw output, pty spawn, CLI client, server join.
///
/// Set `YINZ_TEST_WAIT_TIMEOUT_MS` (the same variable as `yinz_test_support::default_timeout`) to
/// shorten it, for example while debugging a wait that fails.
pub fn default_timeout() -> Duration {
    timeout_from(
        std::env::var(yinz_test_support::TIMEOUT_ENV_VAR).ok(),
        std::env::var(LEGACY_TIMEOUT_ENV_VAR).ok(),
    )
}

fn timeout_from(shared: Option<String>, legacy: Option<String>) -> Duration {
    for (name, raw) in [
        (yinz_test_support::TIMEOUT_ENV_VAR, shared),
        (LEGACY_TIMEOUT_ENV_VAR, legacy),
    ] {
        if let Some(raw) = raw {
            let ms = raw.parse::<u64>().unwrap_or_else(|_| {
                panic!("{name} must be a whole number of milliseconds, got {raw:?}")
            });
            return Duration::from_millis(ms);
        }
    }
    DEFAULT_WAIT
}

#[cfg(test)]
#[path = "unit/default_timeout.rs"]
mod default_timeout_tests;
