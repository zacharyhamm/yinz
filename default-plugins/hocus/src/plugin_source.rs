//! Reading a plugin without reading all of it, and changing it without re-sending it: the pure
//! halves of `plugin_search`, `plugin_outline`, `plugin_list`'s lengths, and `plugin_edit`'s
//! batches and diagnostics.
//!
//! Nothing here touches the host. Every function is handed the text it works on, which is what
//! lets the tests never touch a filesystem - and what keeps `tools.rs` to formatting and dispatch.
//! The point of all of it is the same one `plugin_docs` makes: hocus re-sends the whole transcript
//! every turn, so a page read to find one function is paid for on every round after it. A
//! search hit, an outline line and an edit answer that says where it landed are the cheap
//! substitutes.

use regex::RegexBuilder;
use std::collections::BTreeSet;
use std::ops::Range;
use yinz_tile::plugin_authoring;

// Search ------------------------------------------------------------------------------------

/// Matching lines a search answers with unless asked otherwise. Every hit is re-sent on every
/// later turn of the conversation, and this many is enough to find a definition in a folder of
/// plugins; the tail says how many more there were.
pub const SEARCH_HITS: usize = 40;
/// The most a call can ask for.
pub const MAX_SEARCH_HITS: usize = 200;
/// The most lines either side of a hit a call can ask for.
pub const MAX_CONTEXT: usize = 10;
/// How large a compiled pattern may grow. A pathological pattern is refused with the regex's
/// own error rather than built inside a wasm plugin's memory.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// A `plugin_search`, or a `plugin_docs query=`, as the model asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search<'a> {
    pub query: &'a str,
    pub case_insensitive: bool,
    /// Lines shown either side of a hit.
    pub context: usize,
    pub max_hits: usize,
}

/// ripgrep's shape, which the model already reads from `search_files`: `name:LINE:text` for a
/// hit, `name-LINE-text` for a context line, and `--` between groups that do not touch when
/// context was asked for. The text after the second separator is the line verbatim, so it can be
/// copied into an `old_string`.
///
/// `Ok(None)` when nothing matched, so each caller can say what was searched; `Err` is the
/// regex's own error, which is the whole answer to a bad pattern.
pub fn search(files: &[(&str, &str)], search: &Search) -> Result<Option<String>, String> {
    let regex = RegexBuilder::new(search.query)
        .case_insensitive(search.case_insensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|error| error.to_string())?;
    let mut total = 0;
    let mut shown = 0;
    let mut out: Vec<String> = Vec::new();
    for (name, contents) in files {
        let lines: Vec<&str> = contents.lines().collect();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .map(|(index, _)| index)
            .collect();
        total += hits.len();
        let room = search.max_hits.saturating_sub(shown);
        let kept: BTreeSet<usize> = hits.iter().copied().take(room).collect();
        shown += kept.len();
        // Context windows that overlap or touch print as one group, as rg prints them.
        let mut groups: Vec<Range<usize>> = Vec::new();
        for &hit in &kept {
            let start = hit.saturating_sub(search.context);
            let end = (hit + search.context + 1).min(lines.len());
            match groups.last_mut() {
                Some(last) if start <= last.end => last.end = last.end.max(end),
                _ => groups.push(start..end),
            }
        }
        for group in groups {
            if search.context > 0 && !out.is_empty() {
                out.push("--".to_owned());
            }
            for index in group {
                let separator = if kept.contains(&index) { ':' } else { '-' };
                out.push(format!(
                    "{name}{separator}{}{separator}{}",
                    index + 1,
                    lines[index]
                ));
            }
        }
    }
    if total == 0 {
        return Ok(None);
    }
    let mut text = out.join("\n");
    if shown < total {
        text.push_str(&format!(
            "\n\n… {shown} of {total} matching lines shown; raise max_results, or narrow the \
             query or name"
        ));
    }
    Ok(Some(text))
}

// Outline and listing ---------------------------------------------------------------------------

/// A plugin name as a listing prints it: `keys.ts`, whether or not the caller wrote the suffix.
pub fn file_name(name: &str) -> String {
    let name = name.trim();
    // Preserve a rejected name in diagnostics; resolve_plugin_file rejects this extension.
    if name.ends_with(".lua") || plugin_authoring::is_deno_plugin(std::path::Path::new(name)) {
        name.to_owned()
    } else {
        format!("{name}.ts")
    }
}

/// What `plugin_outline` answers: the file's length, then every entry of its outline on a line
/// of its own, numbers right-aligned so the column reads.
pub fn outline_answer(name: &str, source: &str) -> String {
    let file = file_name(name);
    let total = source.lines().count();
    if total == 0 {
        return "the file is empty".to_owned();
    }
    let entries = match crate::tools::check_report(
        name,
        source,
        yinz_tile::plugin_authoring::PluginCheckMode::Syntax,
    ) {
        Ok(report) if report.diagnostics.is_empty() => report.outline,
        Ok(report) => return report.diagnostics.join("\n"),
        Err(error) => return error,
    };
    if entries.is_empty() {
        return format!(
            "{file}: {total} lines; nothing to outline - no functions, handlers, imports or \
             permission requests"
        );
    }
    let width = entries
        .iter()
        .map(|entry| entry.line)
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let mut out = format!("{file}: {total} lines");
    for entry in entries {
        out.push_str(&format!("\n{:>width$}: {}", entry.line, entry.text));
    }
    out
}

/// One plugin as `plugin_list` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed<'a> {
    /// The name the tools take back: `keys.ts`, `app/ui.ts`.
    pub name: &'a str,
    /// Its length, or why it could not be read.
    pub lines: Result<usize, String>,
    /// Whether a `.bak` of it is there from an earlier write or edit.
    pub backup: bool,
}

/// `keys.ts  412 lines`, one per plugin, with the backup noted where there is one - so the
/// model can tell a 40-line plugin from a 900-line one before deciding how to read it.
pub fn listing(entries: &[Listed]) -> String {
    if entries.is_empty() {
        return "no plugins yet".to_owned();
    }
    entries
        .iter()
        .map(|entry| {
            let length = match &entry.lines {
                Ok(1) => "1 line".to_owned(),
                Ok(lines) => format!("{lines} lines"),
                Err(error) => format!("unreadable: {error}"),
            };
            let backup = match entry.backup {
                true => format!(
                    "  ({} kept from an earlier edit)",
                    plugin_authoring::backup_name(entry.name)
                ),
                false => String::new(),
            };
            format!("{}  {length}{backup}", entry.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub use yinz_tile::plugin_authoring::edits::*;
