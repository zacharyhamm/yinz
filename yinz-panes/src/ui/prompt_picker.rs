//! Fuzzy search over every prompt in the archive, for reuse from an agent's prompt field.
//!
//! The list is far larger than anything else the palette filters, so [`matches`] runs on a
//! worker. Results reach the widget as a semantic section, which it shows as given rather than
//! filtering again on the screen thread.
use std::cmp::Reverse;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use nucleo_matcher::{
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};
use yinz_bus::command_palette::PromptRecord;

use super::command_palette::{escape_literal, Entry, EntryKind, PluginSection};

pub const TITLE: &str = "Past prompts";
pub const ENTRY_PREFIX: &str = "prompt-library:";
/// Rows one result set shows. A person reads the top of the ranking, not all of it.
pub const MAX_RESULTS: usize = 100;
/// Longest draft used as the opening query. A recalled prompt is not a search.
const MAX_SEED_CHARS: usize = 200;
/// nucleo 0.3 scores each atom in u16: with the default config, a character earns at most
/// 16 match points plus 10 bonus points (20 for the first). 1,024 stays well below overflow.
const MAX_ATOM_CHARS: usize = 1_024;
const LABEL_CHARS: usize = 200;
/// Rows a worker claims at a time, which also bounds how late it notices cancellation.
const BLOCK_ROWS: usize = 256;
/// Leaves the rest of the machine's cores for the panes still rendering.
const MAX_WORKERS: usize = 8;

/// The ranked positions in `prompts`, or `None` once `active` is cleared by a newer query.
///
/// `prompts` is newest first, so equal scores and the empty query both favour recent use.
pub fn matches(prompts: &[PromptRecord], query: &str, active: &AtomicBool) -> Option<Vec<usize>> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    if pattern
        .atoms
        .iter()
        .any(|atom| atom.needle_text().len() > MAX_ATOM_CHARS)
    {
        // A completed empty result lets the picker finish this generation; None means cancelled.
        return Some(Vec::new());
    }
    if pattern.atoms.is_empty() {
        return Some((0..prompts.len().min(MAX_RESULTS)).collect());
    }
    let mut scored = score_all(prompts, &pattern, active)?;
    if scored.len() > MAX_RESULTS {
        scored.select_nth_unstable(MAX_RESULTS);
        scored.truncate(MAX_RESULTS);
    }
    scored.sort_unstable();
    Some(scored.into_iter().map(|(_, index)| index).collect())
}

/// Every matching position with its score, across a few cores.
///
/// Long multi-word queries over the whole archive take hundreds of milliseconds on one core.
/// Workers claim blocks because prompt lengths vary too much for equal shares to balance.
fn score_all(
    prompts: &[PromptRecord],
    pattern: &Pattern,
    active: &AtomicBool,
) -> Option<Vec<(Reverse<u32>, usize)>> {
    let next_block = AtomicUsize::new(0);
    let score_blocks = || {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buffer = Vec::new();
        let mut scored = Vec::new();
        loop {
            let start = next_block.fetch_add(BLOCK_ROWS, Ordering::Relaxed);
            if start >= prompts.len() {
                return Some(scored);
            }
            if !active.load(Ordering::Acquire) {
                return None;
            }
            for (offset, prompt) in prompts[start..].iter().take(BLOCK_ROWS).enumerate() {
                let text = Utf32Str::new(&prompt.text, &mut buffer);
                if let Some(score) = pattern.score(text, &mut matcher) {
                    scored.push((Reverse(score), start + offset));
                }
            }
        }
    };
    let workers = std::thread::available_parallelism()
        .map_or(1, |cores| cores.get())
        .min(MAX_WORKERS)
        .min(prompts.len().div_ceil(BLOCK_ROWS));
    std::thread::scope(|scope| {
        let helpers: Vec<_> = (1..workers).map(|_| scope.spawn(score_blocks)).collect();
        let mut scored = score_blocks();
        for helper in helpers {
            let found = helper.join().ok().flatten();
            scored = scored.zip(found).map(|(mut all, found)| {
                all.extend(found);
                all
            });
        }
        scored
    })
}

/// The picker's rows for these ranked positions, or `status` when there are none to show.
pub fn section(prompts: &[PromptRecord], matches: &[usize], status: &str) -> PluginSection {
    let entries: Vec<_> = matches
        .iter()
        .filter_map(|index| prompts.get(*index).map(|prompt| (*index, prompt)))
        .map(|(index, prompt)| Entry {
            id: format!("{ENTRY_PREFIX}{index}"),
            label: escape_literal(&one_line(&prompt.text, LABEL_CHARS)),
            description: escape_literal(&prompt.description),
            aliases: vec![],
            shortcut: String::new(),
            kind: EntryKind::Plugin,
        })
        .collect();
    PluginSection {
        title: TITLE.into(),
        status: entries.is_empty().then(|| status.into()),
        entries,
        semantic: true,
    }
}

/// The opening query for a draft: its words, unless it is too long to be a search.
pub fn seed_query(draft: &str) -> String {
    if draft.chars().count() > MAX_SEED_CHARS {
        return String::new();
    }
    one_line(draft, MAX_SEED_CHARS)
}

/// Whitespace runs collapsed to single spaces, at most `limit` chars. Stops reading at the limit.
fn one_line(text: &str, limit: usize) -> String {
    let mut line = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if count > 0 {
            line.push(' ');
            count += 1;
        }
        for c in word.chars() {
            // Past the limit, not only at it: a word can end on it and the space go over.
            if count >= limit {
                line.truncate(line.trim_end().len());
                return line;
            }
            line.push(c);
            count += 1;
        }
    }
    line
}

#[cfg(test)]
#[path = "./unit/prompt_picker_tests.rs"]
mod tests;
