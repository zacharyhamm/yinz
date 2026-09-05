//! Naming a plugin in the user's plugin folder, for everything that writes one: `zellij plugins
//! write`, and hocus's `plugin_write`, which spells the same rules in wasm-flavoured paths.
//!
//! A name is `keys` or `folder/module` - a file, or one folder deep for a module a plugin
//! `require`s - and becomes `<dir>/<name>.lua`. Everything else is refused before anything
//! touches the disk, so a name cannot be used to build a path outside the folder.

use crate::data::PermissionType;
use crate::plugin_data::validate_namespace;
/// The store's own columns, and the prefix that makes a namespace a shared one. Re-exported
/// here because a plugin that writes plugins - and the guide's tests - check a source against
/// the same two rules the store itself enforces, and a second copy of either would drift.
pub use crate::plugin_data::{RESERVED_ATTR_NAMES, SHARED_NAMESPACE_PREFIX};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use strum::IntoEnumIterator;

/// The longest plugin name accepted, so a name cannot be used to build an unreasonable path.
pub const MAX_PLUGIN_NAME_LEN: usize = 64;

/// `<dir>/<name>.lua`, or why `name` is not a plugin name.
pub fn resolve_plugin_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name is required and must not be empty".to_owned());
    }
    let name = name.strip_suffix(".lua").unwrap_or(name);
    if name.len() > MAX_PLUGIN_NAME_LEN {
        return Err(format!(
            "a plugin name may be at most {MAX_PLUGIN_NAME_LEN} characters"
        ));
    }
    let components: Vec<&str> = name.split('/').collect();
    if components.len() > 2 {
        return Err(format!(
            "{name:?} is not a plugin name: at most one folder, for a module a plugin requires"
        ));
    }
    for component in &components {
        if component.is_empty() || *component == "." || *component == ".." {
            return Err(format!("{name:?} is not a plugin name"));
        }
        // One rule that refuses `\`, NUL, spaces and every traversal at once.
        if !component
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(
                "a plugin name is a file name, or folder/file: letters, digits, '.', '_' and \
                 '-' only"
                    .to_owned(),
            );
        }
        // Never option-shaped, whatever a shell later does with it.
        if component.starts_with('-') {
            return Err("a plugin name may not start with '-'".to_owned());
        }
    }
    if !dir.is_absolute() {
        return Err("the plugin folder is not an absolute path".to_owned());
    }
    let path = dir.join(format!("{name}.lua"));
    // Belt and braces: the rules above admit no `..`, and this says so a second way.
    if path.components().any(|c| matches!(c, Component::ParentDir)) || !path.starts_with(dir) {
        return Err(format!("{name:?} is not a plugin name"));
    }
    Ok(path)
}

/// The backup a write keeps of the file it replaces: `keys` becomes `keys.bak`, which is
/// `keys.bak.lua` on disk and `plugin_read name=keys.bak` from a tool.
///
/// A sibling rather than a dotfile or a folder of versions: one previous version is what a
/// rewrite that lost a feature needs, and naming it as a plugin name means every rule that
/// already governs a plugin path governs this one too - including the refusal to write through
/// a symlink.
pub fn backup_name(name: &str) -> String {
    let name = name.trim();
    let name = name.strip_suffix(".lua").unwrap_or(name);
    format!("{name}.bak")
}

/// Whether `name` names a backup rather than a plugin, so a listing can leave it out.
pub fn is_backup_name(name: &str) -> bool {
    let name = name.trim();
    name.ends_with(".bak") || name.ends_with(".bak.lua")
}

// Reading a source lexically ---------------------------------------------------------------

/// A source split into what is code and what is written inside it.
///
/// Every rule in this module is lexical, and a `find` over raw source cannot tell a call from
/// the same words written in a comment or quoted in a string. That matters more here than in
/// an ordinary linter: a `request_permission` this module reads is a permission hocus grants
/// on the user's behalf when they approve the write, so a call that only looks like one must
/// grant nothing.
struct Lexed {
    /// `source` with every comment and every string literal's contents replaced by spaces.
    /// Byte offsets, line breaks and the delimiters themselves are kept, so an offset found in
    /// `code` is that same offset in `source` and names the same line.
    code: String,
    /// Every string literal, as the offset of its first content byte and its text. Escapes are
    /// left as written: these are permission, namespace and attribute names, and unescaping
    /// could only invent one that is not there.
    strings: Vec<(usize, String)>,
}

impl Lexed {
    /// The string literals whose contents begin inside `range`.
    fn strings_in(&self, range: Range<usize>) -> impl Iterator<Item = (usize, &str)> {
        self.strings
            .iter()
            .filter(move |(at, _)| range.contains(at))
            .map(|(at, value)| (*at, value.as_str()))
    }

    /// The first string literal beginning at or after `from`, if nothing but whitespace and a
    /// string's own delimiters sits between the two - so `= "shared:todo"` is a literal and
    /// `= M.config.namespace` is not.
    fn literal_after(&self, from: usize, end: usize) -> Option<(usize, &str)> {
        let (at, value) = self.strings_in(from..end).next()?;
        self.code[from..at]
            .trim_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '[' | '='))
            .is_empty()
            .then_some((at, value))
    }

    /// The same, for a call's argument: an opening parenthesis may sit between too, so
    /// `require("zellij")` and `require "zellij"` both name `zellij`, and `require(name)` names
    /// nothing.
    fn literal_argument(&self, from: usize, end: usize) -> Option<(usize, &str)> {
        let (at, value) = self.strings_in(from..end).next()?;
        self.code[from..at]
            .trim_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '[' | '=' | '('))
            .is_empty()
            .then_some((at, value))
    }
}

/// `text`'s bytes as spaces - one per byte, so every offset after it still lines up - with its
/// line breaks kept, so a finding still names the line it is on.
fn blank(code: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\n' => code.push('\n'),
            _ => {
                for _ in 0..c.len_utf8() {
                    code.push(' ');
                }
            },
        }
    }
}

/// The length and the level of a long-bracket opener (`[[`, `[=[`, `[==[`) at `at`.
fn long_bracket(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let mut i = at + 1;
    while bytes.get(i) == Some(&b'=') {
        i += 1;
    }
    (bytes.get(i) == Some(&b'[')).then(|| (i + 1 - at, i - at - 1))
}

/// Where a long bracket's contents end and where the bracket itself ends: its matching `]=*]`,
/// or the end of the source when it never closes.
fn long_bracket_end(bytes: &[u8], from: usize, level: usize) -> (usize, usize) {
    let mut at = from;
    while at < bytes.len() {
        if bytes[at] == b']' {
            let mut i = at + 1;
            while bytes.get(i) == Some(&b'=') {
                i += 1;
            }
            if i - at - 1 == level && bytes.get(i) == Some(&b']') {
                return (at, i + 1);
            }
        }
        at += 1;
    }
    (bytes.len(), bytes.len())
}

/// Read a Lua source into its code and its strings.
///
/// Byte-wise on purpose, and safe to be: every byte it compares against is ASCII, and UTF-8
/// never puts an ASCII byte inside a multi-byte character, so a Japanese comment can be walked
/// a byte at a time without ever slicing one in half.
fn lex(source: &str) -> Lexed {
    let bytes = source.as_bytes();
    let mut code = String::with_capacity(source.len());
    let mut strings = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        // A comment is blanked whole, the `--` included, so a commented-out call is not a call.
        if bytes[at] == b'-' && bytes.get(at + 1) == Some(&b'-') {
            let end = match long_bracket(bytes, at + 2) {
                Some((open, level)) => long_bracket_end(bytes, at + 2 + open, level).1,
                None => source[at..]
                    .find('\n')
                    .map_or(bytes.len(), |newline| at + newline),
            };
            blank(&mut code, &source[at..end]);
            at = end;
            continue;
        }
        // A long string `[[ ... ]]`, brackets and all.
        if let Some((open, level)) = long_bracket(bytes, at) {
            let content = at + open;
            let (content_end, end) = long_bracket_end(bytes, content, level);
            blank(&mut code, &source[at..end]);
            strings.push((content, source[content..content_end].to_owned()));
            at = end;
            continue;
        }
        // A short string, up to the quote that opened it or the end of the line - an
        // unterminated one is a compile error, and the compiler is what says so.
        if bytes[at] == b'"' || bytes[at] == b'\'' {
            let quote = bytes[at];
            let mut end = at + 1;
            while end < bytes.len() && bytes[end] != quote && bytes[end] != b'\n' {
                let escape = bytes[end] == b'\\';
                end += 1;
                // An escape hides the next character, whatever it is - the quote included.
                if escape {
                    end += 1;
                    while end < bytes.len() && !source.is_char_boundary(end) {
                        end += 1;
                    }
                }
            }
            let end = end.min(bytes.len());
            code.push(quote as char);
            blank(&mut code, &source[at + 1..end]);
            strings.push((at + 1, source[at + 1..end].to_owned()));
            at = match bytes.get(end) == Some(&quote) {
                true => {
                    code.push(quote as char);
                    end + 1
                },
                false => end,
            };
            continue;
        }
        let c = source[at..]
            .chars()
            .next()
            .expect("a byte in a str is part of a character");
        code.push(c);
        at += c.len_utf8();
    }
    Lexed { code, strings }
}

/// Whether a byte can be part of a Lua name, so a name is never found inside a longer one.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Where `word` appears in `hay` as a word of its own, at or after `from`. `data.fetch` counts
/// as one word here: what it may not be is the tail of `mydata.fetch`.
fn find_word(hay: &str, word: &str, from: usize) -> Option<usize> {
    let mut at = from;
    while let Some(hit) = hay[at..].find(word) {
        let start = at + hit;
        let end = start + word.len();
        let free_before = start == 0 || !is_word_byte(hay.as_bytes()[start - 1]);
        let free_after = hay.as_bytes().get(end).is_none_or(|b| !is_word_byte(*b));
        if free_before && free_after {
            return Some(start);
        }
        at = end;
    }
    None
}

/// The Lua names in `code`, with their offsets. Only ever called on a [`Lexed::code`], where
/// nothing inside a comment or a string is left to find.
fn words(code: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = code.as_bytes();
    let mut at = 0;
    std::iter::from_fn(move || {
        while at < bytes.len() && !is_word_byte(bytes[at]) {
            at += 1;
        }
        let start = at;
        while at < bytes.len() && is_word_byte(bytes[at]) {
            at += 1;
        }
        (at > start).then(|| (start, &code[start..at]))
    })
}

/// Where the bracket at `open` closes. Depth-counted, and reliable because it is only ever run
/// over code with the strings and comments already taken out of it.
fn matching_bracket(code: &str, open: usize) -> Option<usize> {
    let opener = *code.as_bytes().get(open)?;
    let closer = match opener {
        b'{' => b'}',
        b'(' => b')',
        b'[' => b']',
        _ => return None,
    };
    let mut depth = 0usize;
    for (at, byte) in code.as_bytes().iter().enumerate().skip(open) {
        if *byte == opener {
            depth += 1;
        } else if *byte == closer {
            depth -= 1;
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}

// Permissions ------------------------------------------------------------------------------

/// The permission strings a plugin's source asks for in `zellij.request_permission { ... }`:
/// the ones zellij knows, and the ones it does not.
///
/// Read lexically, not by running the Lua: every string literal inside the braces of a
/// `request_permission` call, in either spelling (`"read_files"` or `"ReadFiles"`), which is
/// what the prelude accepts. The reading goes through [`lex`], so a call written in a comment
/// or quoted in a string is not one, and an apostrophe inside the braces cannot shift the
/// strings around it. Anything more elaborate - a permission list built at runtime - is not
/// seen, and the plugin's own call reports it in the ordinary way.
///
/// `unknown` is kept rather than dropped because a name zellij does not know is *silently*
/// ungranted: hocus pre-grants what this finds when a write is approved, so a typo becomes a
/// denial much later, in a pane nobody is looking at. [`lint`] is what says so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestedPermissions {
    pub known: Vec<PermissionType>,
    pub unknown: Vec<String>,
}

/// Every `request_permission` call: where it is, and every string literal inside its braces
/// with its own byte offset.
fn permission_calls(lexed: &Lexed) -> Vec<(usize, Vec<(usize, String)>)> {
    const CALL: &str = "request_permission";
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = find_word(&lexed.code, CALL, at) {
        let after = start + CALL.len();
        at = after;
        let Some(open) = lexed.code[after..].find(['{', '(']) else {
            break;
        };
        // Only whitespace may sit between the name and its argument, or this is some other
        // identifier that happens to end the same way.
        if !lexed.code[after..after + open].trim().is_empty() {
            continue;
        }
        let open = after + open;
        let Some(close) = matching_bracket(&lexed.code, open) else {
            break;
        };
        found.push((
            start,
            lexed
                .strings_in(open..close)
                .map(|(at, value)| (at, value.to_owned()))
                .collect(),
        ));
        at = close;
    }
    found
}

/// Every string literal inside a `request_permission` call, with its byte offset.
fn permission_strings(lexed: &Lexed) -> Vec<(usize, String)> {
    permission_calls(lexed)
        .into_iter()
        .flat_map(|(_, strings)| strings)
        .collect()
}

/// The permissions a source asks for, split into what zellij knows and what it does not.
pub fn requested_permissions(source: &str) -> RequestedPermissions {
    permissions_in(&lex(source))
}

fn permissions_in(lexed: &Lexed) -> RequestedPermissions {
    let mut out = RequestedPermissions::default();
    for (_, quoted) in permission_strings(lexed) {
        match parse_permission(&quoted) {
            Some(permission) => {
                if !out.known.contains(&permission) {
                    out.known.push(permission);
                }
            },
            None => {
                if !out.unknown.contains(&quoted) {
                    out.unknown.push(quoted);
                }
            },
        }
    }
    out
}

/// A permission in either spelling a plugin may use: `read_files` or `ReadFiles`.
fn parse_permission(name: &str) -> Option<PermissionType> {
    let pascal: String = name
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    pascal.parse::<PermissionType>().ok()
}

/// Permissions as a plugin author spells them: `read_files`, not `ReadFiles`.
pub fn permission_names(permissions: &[PermissionType]) -> Vec<String> {
    permissions.iter().map(|p| permission_name(*p)).collect()
}

/// One permission, spelled the way a plugin asks for it.
pub fn permission_name(permission: PermissionType) -> String {
    let mut out = String::new();
    for (i, c) in permission.to_string().chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Every permission a plugin may ask for, in the spelling it asks for it.
pub fn all_permission_names() -> Vec<String> {
    PermissionType::iter().map(permission_name).collect()
}

/// The known permission closest to `typo`, when one is close enough to name in a suggestion.
///
/// Ranked by shared words before edit distance, because these names are compounds and the word
/// someone got right says more than the letters they got wrong: `shared_data` is one word away
/// from `shared_plugin_data` and six letters away from `plugin_data`, and it is the first of
/// those that they meant.
fn nearest_permission(typo: &str) -> Option<String> {
    let words: Vec<&str> = typo.split('_').filter(|word| !word.is_empty()).collect();
    let mut best: Option<(usize, usize, String)> = None;
    for candidate in all_permission_names() {
        let shared = candidate
            .split('_')
            .filter(|word| words.contains(word))
            .count();
        let distance = edit_distance(typo, &candidate);
        let better = best
            .as_ref()
            .is_none_or(|(s, d, _)| shared > *s || (shared == *s && distance < *d));
        if better {
            best = Some((shared, distance, candidate));
        }
    }
    // No word in common and far enough away, and a suggestion is a guess: `weather` is not a
    // misspelling of anything.
    best.filter(|(shared, distance, name)| *shared > 0 || *distance * 2 <= name.len())
        .map(|(_, _, name)| name)
}

/// Levenshtein distance, for suggesting the permission someone meant.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            let next = (row[j] + 1).min(row[j + 1] + 1).min(previous + cost);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

// The store's calls ------------------------------------------------------------------------

/// The `zellij.data` calls a namespace or an attribute list can appear in.
const DATA_CALLS: [&str; 5] = [
    "data.define",
    "data.fetch",
    "data.watch",
    "data.count",
    "data.apply",
];

/// How much source one call's arguments may span before the reading gives up on them.
///
/// Lexical on purpose. A `namespace` key means one thing inside `zellij.data.fetch` and quite
/// another inside a hocus extension's registration payload, where it names the tool namespace
/// and is nothing to do with the store - so a rule about namespaces may only look at the
/// arguments of a data call, never at the file as a whole.
const CALL_WINDOW: usize = 2_000;

/// The extent of one call's arguments: its outermost bracket, or a window's worth of source
/// when that bracket does not close inside one.
fn call_arguments(lexed: &Lexed, at: usize) -> Range<usize> {
    // Clamped to a character boundary: a plugin's comments are as likely to be Japanese as
    // English, and slicing a source in the middle of one would panic rather than mis-lint.
    let mut end = lexed.code.len().min(at + CALL_WINDOW);
    while end > at && !lexed.code.is_char_boundary(end) {
        end -= 1;
    }
    let Some(open) = lexed.code[at..end].find(['{', '(']) else {
        return at..end;
    };
    let open = at + open;
    match matching_bracket(&lexed.code, open) {
        Some(close) if close < end => open..close + 1,
        _ => at..end,
    }
}

/// Every `zellij.data.*` call site, as (offset of the call, the extent of its arguments).
fn data_calls(lexed: &Lexed) -> Vec<(usize, Range<usize>)> {
    let mut found = Vec::new();
    for call in DATA_CALLS {
        let mut at = 0;
        while let Some(start) = find_word(&lexed.code, call, at) {
            found.push((start, call_arguments(lexed, start)));
            at = start + call.len();
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found
}

/// The one positional string a `data.apply(mutations, "namespace")` carries: the literal after
/// a comma in the call's own argument list, never one nested in a mutation.
fn positional_namespace(lexed: &Lexed, arguments: Range<usize>) -> Option<(usize, &str)> {
    let bytes = lexed.code.as_bytes();
    let mut depth = 0usize;
    let mut after_comma = false;
    for at in arguments.clone() {
        match bytes[at] {
            b'(' | b'{' | b'[' => {
                depth += 1;
                after_comma = false;
            },
            b')' | b'}' | b']' => {
                depth = depth.saturating_sub(1);
                after_comma = false;
            },
            b',' if depth == 1 => after_comma = true,
            b'"' | b'\'' if depth == 1 && after_comma => {
                return lexed.strings_in(at + 1..arguments.end).next();
            },
            b' ' | b'\t' | b'\n' | b'\r' => {},
            _ => after_comma = false,
        }
    }
    None
}

/// The string literal a `key = "..."` inside `range` carries, if it is a literal at all.
fn literal_value<'a>(lexed: &'a Lexed, range: Range<usize>, key: &str) -> Option<(usize, &'a str)> {
    let at = find_word(&lexed.code[..range.end], key, range.start)?;
    let rest = &lexed.code[at + key.len()..range.end];
    let equals = rest.find('=')?;
    if !rest[..equals].trim().is_empty() {
        return None;
    }
    lexed.literal_after(at + key.len() + equals + 1, range.end)
}

/// The `{ ... }` entries of the table at `table`, one range each.
fn table_entries(lexed: &Lexed, table: Range<usize>) -> Vec<Range<usize>> {
    let bytes = lexed.code.as_bytes();
    let mut entries = Vec::new();
    let mut at = table.start + 1;
    while at < table.end {
        if bytes[at] == b'{' {
            let Some(close) = matching_bracket(&lexed.code, at) else {
                break;
            };
            entries.push(at..close.min(table.end) + 1);
            at = close + 1;
            continue;
        }
        at += 1;
    }
    entries
}

/// The attribute one entry of an `attrs` list declares, read the way `prelude.lua`'s `define`
/// reads it: `name = "..."` when it has one, otherwise the first positional value. Only that
/// one string is a declaration - a kind, and a `default = "created_at"`, are not.
fn attribute_name(lexed: &Lexed, entry: Range<usize>) -> Option<(usize, &str)> {
    literal_value(lexed, entry.clone(), "name").or_else(|| lexed.strings_in(entry).next())
}

/// Whether a call runs before an ordinary callback can wait for a first permission grant.
fn data_call_runs_during_load(code: &str, call_at: usize) -> bool {
    let mut blocks = Vec::new();
    let mut pending_do = 0usize;
    for (at, word) in words(&code[..call_at]) {
        match word {
            "function" => {
                let is_load = definition(code, at).is_some_and(|definition| {
                    definition
                        .text
                        .split('(')
                        .next()
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(|word| word.trim_end_matches('='))
                        .any(|word| word.rsplit(['.', ':']).next() == Some("load"))
                });
                blocks.push(Some(is_load));
            },
            "if" | "repeat" => blocks.push(None),
            "for" | "while" => {
                blocks.push(None);
                pending_do += 1;
            },
            "do" if pending_do > 0 => pending_do -= 1,
            "do" => blocks.push(None),
            "end" | "until" => {
                blocks.pop();
            },
            _ => {},
        }
    }
    blocks.iter().rev().find_map(|block| *block).unwrap_or(true)
}

// Definitions ------------------------------------------------------------------------------

/// A named function's opening: where its statement starts (`local` included) and its signature
/// cut after the parameter list.
struct Definition {
    start: usize,
    text: String,
}

/// Whether a byte can be part of a dotted name: `M.render`, `Text:len`.
fn is_name_byte(byte: u8) -> bool {
    is_word_byte(byte) || matches!(byte, b'.' | b':')
}

/// Where a `local` qualifying the name at `at` begins, if one does.
fn leading_local(code: &str, at: usize) -> Option<usize> {
    let before = code[..at].trim_end();
    let start = before.strip_suffix("local")?.len();
    (start == 0 || !is_word_byte(before.as_bytes()[start - 1])).then_some(start)
}

/// The definition the `function` keyword at `at` opens, or `None` for an anonymous function -
/// one passed as an argument, returned, or otherwise never named.
fn definition(code: &str, at: usize) -> Option<Definition> {
    let after = at + "function".len();
    let name_start = after + (code[after..].len() - code[after..].trim_start().len());
    let name_len = code[name_start..]
        .bytes()
        .take_while(|byte| is_name_byte(*byte))
        .count();
    let start = if name_len > 0 {
        // `function M.load(config)`, `local function tick()`.
        leading_local(code, at).unwrap_or(at)
    } else {
        // `M.render = function(rows, cols)`, `local tick = function()`.
        let before = code[..at].trim_end().strip_suffix('=')?;
        // `==` compares; only a lone `=` assigns.
        if before.ends_with(['=', '~', '<', '>']) {
            return None;
        }
        let before = before.trim_end();
        let name_at = before
            .bytes()
            .rposition(|byte| !is_name_byte(byte))
            .map_or(0, |i| i + 1);
        if name_at == before.len() {
            return None;
        }
        leading_local(code, name_at).unwrap_or(name_at)
    };
    // The parameter list, which every function has, is where the signature ends.
    let params = name_start + name_len;
    let open = params + code[params..].find('(')?;
    if !code[params..open].trim().is_empty() {
        return None;
    }
    let close = matching_bracket(code, open)?;
    let text = code[start..=close]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some(Definition { start, text })
}

/// The 1-based line a byte offset falls on. Counted in line breaks rather than with `lines()`,
/// which would put an offset at the very start of a line on the line before it.
fn line_of(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset.min(source.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

// The shape of a source ---------------------------------------------------------------------

/// What one entry of an [`outline`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutlineKind {
    /// `require "name"`.
    Require,
    /// `request_permission { ... }`, with every name it asks for.
    Permission,
    /// A named function in either spelling: `function M.load(config)`, `local function tick()`
    /// or `M.render = function(rows, cols)`.
    Function,
    /// `on "event"`: a handler registered with `zellij.on`.
    Handler,
    /// `data.define "entity"`.
    Define,
    /// A hocus extension's registration message.
    Registration,
    /// One tool that registration offers hocus.
    Tool,
}

/// One line of an [`outline`]: what is defined or hooked there, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    /// 1-based, as an editor counts them.
    pub line: usize,
    pub kind: OutlineKind,
    /// The entry as an outline prints it: a signature cut after its parameter list, or a call
    /// with the one literal that says what it is about.
    pub text: String,
}

/// What a source defines and hooks, with the line each is on: the map a reader needs before
/// reading a range of it rather than a page.
///
/// Lexical like everything here, and read off [`Lexed::code`], so a definition in a comment or a
/// string is not one. An anonymous `function(...)` in argument position is not a definition, so
/// `zellij.on("key", function(event) ... end)` is one handler and no function. Sorted by
/// position.
pub fn outline(source: &str) -> Vec<OutlineEntry> {
    let lexed = lex(source);
    let code = lexed.code.as_str();
    let mut found: Vec<(usize, OutlineKind, String)> = Vec::new();
    for (at, word) in words(code) {
        match word {
            "function" => {
                if let Some(definition) = definition(code, at) {
                    found.push((definition.start, OutlineKind::Function, definition.text));
                }
            },
            // `zellij.on(` or an alias's `.on(`; a variable that happens to be called `on` is
            // not a handler.
            "on" if at > 0 && code.as_bytes()[at - 1] == b'.' => {
                if let Some((_, event)) = lexed.literal_argument(at + word.len(), code.len()) {
                    found.push((at, OutlineKind::Handler, format!("on \"{event}\"")));
                }
            },
            "require" => {
                if let Some((_, module)) = lexed.literal_argument(at + word.len(), code.len()) {
                    found.push((at, OutlineKind::Require, format!("require \"{module}\"")));
                }
            },
            _ => {},
        }
    }
    for (at, strings) in permission_calls(&lexed) {
        let names: Vec<String> = strings
            .iter()
            .map(|(_, name)| format!("\"{name}\""))
            .collect();
        found.push((
            at,
            OutlineKind::Permission,
            format!("request_permission {{ {} }}", names.join(", ")),
        ));
    }
    for (at, arguments) in data_calls(&lexed) {
        if !code[at..].starts_with("data.define") {
            continue;
        }
        let text = match literal_value(&lexed, arguments, "name") {
            Some((_, entity)) => format!("data.define \"{entity}\""),
            None => "data.define".to_owned(),
        };
        found.push((at, OutlineKind::Define, text));
    }
    // `zellij.hocus.tools` is how an extension registers, and its tools are the keys of the
    // table it is passed. The message name is what an older, hand-wired one sent, and its
    // tools are the `name = "..."` of a `tools = { ... }` list.
    if let Some(at) = find_word(code, REGISTER_CALL, 0) {
        found.push((at, OutlineKind::Registration, REGISTER_CALL.to_owned()));
        let tools = call_argument_table(code, at, REGISTER_CALL.len())
            .map(|table| keyed_tables(code, table))
            .unwrap_or_default();
        for (at, name) in tools {
            found.push((at, OutlineKind::Tool, format!("tool \"{name}\"")));
        }
    } else if let Some((at, _)) = lexed
        .strings
        .iter()
        .find(|(_, value)| value == REGISTER_MESSAGE)
    {
        found.push((*at, OutlineKind::Registration, REGISTER_MESSAGE.to_owned()));
        for (at, name) in registered_tools(&lexed) {
            found.push((at, OutlineKind::Tool, format!("tool \"{name}\"")));
        }
    }
    found.sort_by_key(|(at, _, _)| *at);
    found
        .into_iter()
        .map(|(at, kind, text)| OutlineEntry {
            line: line_of(source, at),
            kind,
            text,
        })
        .collect()
}

/// The first non-whitespace offset at or after `at`.
fn skip_whitespace(code: &str, at: usize) -> usize {
    at + (code[at..].len() - code[at..].trim_start().len())
}

/// Where the value assigned to the name at `name_at` begins - past a lone `=` - if one is.
fn assigned_value(code: &str, name_at: usize) -> Option<usize> {
    let name_len = code[name_at..]
        .bytes()
        .take_while(|byte| is_word_byte(*byte))
        .count();
    let equals = skip_whitespace(code, name_at + name_len);
    let bytes = code.as_bytes();
    if bytes.get(equals) != Some(&b'=') || bytes.get(equals + 1) == Some(&b'=') {
        return None;
    }
    Some(skip_whitespace(code, equals + 1))
}

/// The `{ ... }` assigned to the name at `name_at`: directly, as `tools = { ... }`, or - when
/// `follow_alias` - through one identifier, as `tools = TOOLS` with `TOOLS = { ... }` elsewhere.
fn assigned_table(code: &str, name_at: usize, follow_alias: bool) -> Option<Range<usize>> {
    let value = assigned_value(code, name_at)?;
    if code.as_bytes().get(value) == Some(&b'{') {
        return matching_bracket(code, value).map(|close| value..close + 1);
    }
    if !follow_alias {
        return None;
    }
    table_at(code, value)
}

/// The table `at` stands for: the `{ ... }` itself, or - `at` being an identifier - the table
/// assigned to that name elsewhere in the source. One hop, never a chain of them.
fn table_at(code: &str, at: usize) -> Option<Range<usize>> {
    if code.as_bytes().get(at) == Some(&b'{') {
        return matching_bracket(code, at).map(|close| at..close + 1);
    }
    let alias_len = code[at..]
        .bytes()
        .take_while(|byte| is_word_byte(*byte))
        .count();
    if alias_len == 0 {
        return None;
    }
    let alias = &code[at..at + alias_len];
    let mut from = 0;
    while let Some(hit) = find_word(code, alias, from) {
        from = hit + alias_len;
        if let Some(table) = assigned_table(code, hit, false) {
            return Some(table);
        }
    }
    None
}

/// The table the call to the name at `name_at` is passed, in every spelling a registration
/// uses: `zellij.hocus.tools { ... }`, `zellij.hocus.tools({ ... })`, or
/// `zellij.hocus.tools(TOOLS)` with the table assigned to `TOOLS` elsewhere.
fn call_argument_table(code: &str, name_at: usize, name_len: usize) -> Option<Range<usize>> {
    let mut at = skip_whitespace(code, name_at + name_len);
    if code.as_bytes().get(at) == Some(&b'(') {
        at = skip_whitespace(code, at + 1);
    }
    table_at(code, at)
}

/// The names a table keys its entries by, as (offset of the key, the name): the `add` of
/// `add = { ... }`. Top level only - a nested `parameters = { ... }` is part of an entry, not
/// another entry.
fn keyed_tables(code: &str, table: Range<usize>) -> Vec<(usize, String)> {
    let bytes = code.as_bytes();
    let mut names = Vec::new();
    let mut at = table.start + 1;
    while at + 1 < table.end {
        match bytes[at] {
            b'{' | b'(' | b'[' => match matching_bracket(code, at) {
                Some(close) => at = close + 1,
                None => break,
            },
            byte if is_word_byte(byte) && !is_name_byte(bytes[at - 1]) => {
                let len = code[at..]
                    .bytes()
                    .take_while(|byte| is_word_byte(*byte))
                    .count();
                if assigned_value(code, at).is_some_and(|value| bytes.get(value) == Some(&b'{')) {
                    names.push((at, code[at..at + len].to_owned()));
                }
                at += len;
            },
            _ => at += 1,
        }
    }
    names
}

/// How a plugin registers its tools with hocus. The prelude sends the message for it, and the
/// tools are the keys of the table this call is passed.
const REGISTER_CALL: &str = "zellij.hocus.tools";

/// What a hand-wired registration sent before `zellij.hocus.tools` existed - still recognised,
/// because a plugin written that way still registers.
const REGISTER_MESSAGE: &str = "hocus_extension_register";

/// The tools a registration offers, as (offset of the entry, its `name`): the entries of the
/// table `tools = ...` names. The worked example keeps them in a `local TOOLS = { ... }` and
/// registers `tools = TOOLS`, which is the one indirection followed.
fn registered_tools(lexed: &Lexed) -> Vec<(usize, String)> {
    let code = lexed.code.as_str();
    let mut from = 0;
    while let Some(key) = find_word(code, "tools", from) {
        from = key + "tools".len();
        let Some(table) = assigned_table(code, key, true) else {
            continue;
        };
        return table_entries(lexed, table)
            .into_iter()
            .filter_map(|entry| {
                literal_value(lexed, entry, "name").map(|(at, name)| (at, name.to_owned()))
            })
            .collect();
    }
    Vec::new()
}

/// What a source is going to do wrong at load or at run time, said before a reload cycle is
/// spent finding out.
///
/// Warnings, never refusals: the compiler is the one thing that says no to a write, and a lint
/// that blocked would have to be right every time. These are read lexically and can be wrong,
/// so each one names the fix and lets the author decide.
///
/// `name` is the plugin's name, as `plugin_write` spells it, because one rule
/// (a `hocus/` extension's registration permission) depends on where the file lands.
pub fn lint(name: &str, source: &str) -> Vec<String> {
    let lexed = lex(source);
    let file = name
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or("plugin")
        .trim_end_matches(".lua");
    let say =
        |offset: usize, what: String| format!("{file}.lua:{}: {what}", line_of(source, offset));
    let mut out = Vec::new();
    let permissions = permissions_in(&lexed);
    let asked = |permission: PermissionType| permissions.known.contains(&permission);

    // A permission zellij does not know is granted to nobody and refused to no one's face.
    for (offset, unknown) in permission_strings(&lexed) {
        if parse_permission(&unknown).is_some() {
            continue;
        }
        let suggestion = match nearest_permission(&unknown) {
            Some(nearest) => format!(" - did you mean {nearest:?}?"),
            None => ".".to_owned(),
        };
        out.push(say(
            offset,
            format!(
                "{unknown:?} is not a zellij permission, so it grants nothing{suggestion} The \
                 names are in the permission table in docs/LUA_PLUGINS.md."
            ),
        ));
    }

    let calls = data_calls(&lexed);
    if let Some((at, _)) = calls
        .iter()
        .find(|(at, _)| data_call_runs_during_load(&lexed.code, *at))
    {
        out.push(say(
            *at,
            "an uncached first load must await `zellij.async.permissions` before a data call; \
             cached grants take effect synchronously, but cannot make this first load safe."
                .to_owned(),
        ));
    }

    let mut uses_data = false;
    let mut said_shared = false;
    for (at, arguments) in calls {
        uses_data = true;
        let named = literal_value(&lexed, arguments.clone(), "namespace");
        let positional = match lexed.code[at..].starts_with("data.apply") {
            true => positional_namespace(&lexed, arguments.clone()),
            false => None,
        };
        for (offset, namespace) in named.into_iter().chain(positional) {
            if let Err(error) = validate_namespace(Some(namespace)) {
                out.push(say(offset, format!("{error}")));
            }
            // The only `shared:` in a file that is one: the same word in a comment, in a tool
            // description or in a hocus registration payload is not a namespace.
            if namespace.starts_with(SHARED_NAMESPACE_PREFIX)
                && !asked(PermissionType::SharedPluginData)
                && !said_shared
            {
                said_shared = true;
                out.push(say(
                    offset,
                    "a `shared:` namespace needs `shared_plugin_data` as well as `plugin_data`, \
                     in every plugin that touches it - the reader too."
                        .to_owned(),
                ));
            }
        }
        if !lexed.code[at..].starts_with("data.define") {
            continue;
        }
        // An entity may be called `id`; an attribute may not, so only the attribute list is
        // read - and within it, only the names being declared.
        let Some(key) = find_word(&lexed.code[..arguments.end], "attrs", arguments.start) else {
            continue;
        };
        let value = key + "attrs".len();
        let Some(equals) = lexed.code[value..arguments.end].find('=') else {
            continue;
        };
        let value = value + equals + 1;
        let Some(open) = lexed.code[value..arguments.end].find('{') else {
            continue;
        };
        let open = value + open;
        if !lexed.code[value..open].trim().is_empty() {
            continue;
        }
        let Some(close) = matching_bracket(&lexed.code, open) else {
            continue;
        };
        for entry in table_entries(&lexed, open..close) {
            let Some((offset, attribute)) = attribute_name(&lexed, entry) else {
                continue;
            };
            if RESERVED_ATTR_NAMES.contains(&attribute) {
                out.push(say(
                    offset,
                    format!(
                        "{attribute:?} is on every row already and cannot be declared - a define \
                         that names it fails whole. Drop the attribute; the column is there."
                    ),
                ));
            }
        }
    }

    if uses_data && !asked(PermissionType::PluginData) {
        out.push(format!(
            "{file}.lua: the data store needs `zellij.request_permission {{ \"plugin_data\" }}`, \
             which this source does not ask for."
        ));
    }
    // `zellij.hocus.tools { ... }` is how an extension registers; the message name is what an
    // older, hand-wired one sent.
    let registers = find_word(&lexed.code, REGISTER_CALL, 0).is_some()
        || lexed
            .strings
            .iter()
            .any(|(_, value)| value == REGISTER_MESSAGE);
    if name.trim().starts_with("hocus/")
        && registers
        && !asked(PermissionType::MessageAndLaunchOtherPlugins)
    {
        out.push(format!(
            "{file}.lua: registering with hocus (`zellij.hocus.tools`) is a plugin message, so \
             it needs `zellij.request_permission {{ \"message_and_launch_other_plugins\" }}`."
        ));
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_file_or_one_folder_deep_and_nothing_else() {
        let dir = Path::new("/home/someone/.config/zellij/plugins");
        assert_eq!(
            resolve_plugin_file(dir, "keys").unwrap(),
            dir.join("keys.lua")
        );
        assert_eq!(
            resolve_plugin_file(dir, " keys.lua ").unwrap(),
            dir.join("keys.lua")
        );
        assert_eq!(
            resolve_plugin_file(dir, "app/ui").unwrap(),
            dir.join("app/ui.lua")
        );
        for bad in [
            "",
            "../x",
            "a/../b",
            "a/b/c",
            "-x",
            "a b",
            "/etc/passwd",
            "x\0",
        ] {
            assert!(resolve_plugin_file(dir, bad).is_err(), "{bad:?}");
        }
        assert!(resolve_plugin_file(dir, &"a".repeat(MAX_PLUGIN_NAME_LEN + 1)).is_err());
        assert!(resolve_plugin_file(Path::new("plugins"), "keys").is_err());
    }

    /// The source the churned todo session actually wrote: a shared namespace spelled as if it
    /// were a private one, reserved columns declared, and the permission for sharing never asked
    /// for. Every one of those is a reload cycle spent finding out at run time.
    #[test]
    fn the_shared_store_traps_are_all_named_before_a_reload() {
        let source = r#"
local zellij = require("zellij")
local M = {}
function M.load()
  zellij.request_permission { "plugin_data" }
end
zellij.on("permission_request_result", function()
  zellij.data.define {
    name = "todo", version = 1, namespace = "hocus_todo",
    attrs = {
      { "title", "text", required = true },
      { "created_at", "int" },
    },
  }
end)
return M
"#;
        let found = lint("todo", source).join("\n");
        assert!(found.contains("hocus_todo"), "{found}");
        assert!(found.contains("shared:"), "{found}");
        assert!(found.contains("created_at"), "{found}");
        // The namespace is not shared-prefixed, so the permission for sharing is not demanded
        // twice: the namespace finding is the one to act on.
        assert!(!found.contains("shared_plugin_data"), "{found}");
    }

    #[test]
    fn a_shared_namespace_asks_for_the_permission_that_makes_it_work() {
        let source = r#"
zellij.request_permission { "plugin_data" }
zellij.on("permission_request_result", function()
  zellij.data.fetch { entity = "todo", namespace = "shared:todo" }
end)
"#;
        let found = lint("todo-pane", source).join("\n");
        assert!(found.contains("shared_plugin_data"), "{found}");
        let granted = source.replace(
            r#"{ "plugin_data" }"#,
            r#"{ "plugin_data", "shared_plugin_data" }"#,
        );
        assert!(
            !lint("todo-pane", &granted)
                .iter()
                .any(|found| found.contains("shared_plugin_data")),
            "{:?}",
            lint("todo-pane", &granted)
        );
    }

    #[test]
    fn data_calls_during_initial_load_explain_permission_cache_semantics() {
        for source in [
            r#"
zellij.request_permission { "plugin_data" }
zellij.data.fetch { entity = "todo", namespace = "todo" }
"#,
            r#"
function M.load()
  zellij.request_permission { "plugin_data" }
  zellij.data.fetch { entity = "todo", namespace = "todo" }
end
"#,
        ] {
            let found = lint("todo", source).join("\n");
            assert!(found.contains("uncached first load"), "{found}");
            assert!(
                found.contains("await `zellij.async.permissions`"),
                "{found}"
            );
            assert!(
                found.contains("cached grants take effect synchronously"),
                "{found}"
            );
        }
    }

    #[test]
    fn data_calls_in_ordinary_event_handlers_do_not_get_the_load_warning() {
        let source = r#"
zellij.request_permission { "plugin_data" }
zellij.on("permission_request_result", function()
  zellij.data.fetch { entity = "todo", namespace = "todo" }
end)
"#;
        assert!(
            !lint("todo", source)
                .iter()
                .any(|found| found.contains("uncached first load")),
            "{:?}",
            lint("todo", source)
        );
    }

    /// A `namespace` key means the tool namespace inside a hocus registration payload and the
    /// store's namespace inside a data call. Only the data call's is ours to judge.
    #[test]
    fn a_namespace_outside_a_data_call_is_not_the_stores() {
        let source = r#"
zellij.request_permission { "message_and_launch_other_plugins" }
local function register()
  zellij.message_to_plugin {
    message_name = "hocus_extension_register",
    message_payload = zellij.json.encode {
      version = 1, namespace = "weather", path = "x",
      tools = { { name = "forecast", safety = "read_only" } },
    },
  }
end
"#;
        assert_eq!(lint("hocus/weather", source), Vec::<String>::new());
    }

    #[test]
    fn a_permission_zellij_does_not_know_grants_nothing_and_says_so() {
        let source = r#"zellij.request_permission { "plugin_dat", "read_files" }"#;
        let found = lint("keys", source).join("\n");
        assert!(found.contains("\"plugin_dat\""), "{found}");
        assert!(found.contains("did you mean \"plugin_data\""), "{found}");
        // The word they got right beats the letters they got wrong.
        let shared = lint("keys", r#"zellij.request_permission { "shared_data" }"#).join("\n");
        assert!(
            shared.contains("did you mean \"shared_plugin_data\""),
            "{shared}"
        );
        // And a word that is not a permission at all gets no guess.
        let unrelated = lint("keys", r#"zellij.request_permission { "weather" }"#).join("\n");
        assert!(!unrelated.contains("did you mean"), "{unrelated}");
        assert_eq!(
            requested_permissions(source),
            RequestedPermissions {
                known: vec![PermissionType::ReadFiles],
                unknown: vec!["plugin_dat".to_owned()],
            }
        );
    }

    /// An extension that cannot send its registration message registers nothing, and the reload
    /// that says so happens a minute later.
    #[test]
    fn an_extension_needs_the_permission_its_registration_is_sent_with() {
        let source = r#"
zellij.request_permission { "plugin_data" }
zellij.on("permission_request_result", function()
  zellij.hocus.tools { forecast = { handler = function() return "sunny" end } }
end)
"#;
        let found = lint("hocus/todo", source).join("\n");
        assert!(
            found.contains("message_and_launch_other_plugins"),
            "{found}"
        );
        // The same file anywhere but the hocus folder is not an extension.
        assert!(!lint("todo", source)
            .iter()
            .any(|f| f.contains("message_and_launch_other_plugins")));
    }

    /// Nothing about a plugin that does none of this may be worth saying.
    /// A window that ends inside a multi-byte character must not take the linter (and with it
    /// the write) down. The trap here is the clamp, not the finding.
    #[test]
    fn a_source_full_of_wide_characters_is_read_without_panicking() {
        let padding = "-- ここに日本語のコメント\n".repeat(200);
        let source = format!(
            "{padding}zellij.request_permission {{ \"plugin_data\" }}\n\
             zellij.on(\"permission_request_result\", function()\n\
             zellij.data.fetch {{ entity = \"note\", namespace = \"note\" }} -- {padding}\n\
             end)\n"
        );
        let found = lint("wide", &source).join("\n");
        assert!(found.contains("shared:"), "{found}");
    }

    #[test]
    fn an_ordinary_plugin_is_quiet() {
        let source = r#"
local zellij = require("zellij")
local M = {}
function M.load()
  zellij.request_permission { "read_application_state" }
  zellij.subscribe { "key" }
end
function M.render() return "hi" end
return M
"#;
        assert_eq!(lint("keys", source), Vec::<String>::new());
    }

    /// The two examples the guide points a plugin author at are the ones the lint's rules were
    /// written from, so a rule that fires on them is a rule that will fire on everyone.
    #[test]
    fn the_worked_examples_are_quiet() {
        for (name, file) in [
            ("hocus/todo", "todo-extension.lua"),
            ("todo-pane", "todo-pane.lua"),
        ] {
            let source = std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../example-plugins")
                    .join(file),
            )
            .expect("the example plugin should be in the repo");
            assert_eq!(lint(name, &source), Vec::<String>::new(), "{file}");
        }
    }

    #[test]
    fn a_backup_is_a_plugin_name_that_a_listing_leaves_out() {
        assert_eq!(backup_name("keys"), "keys.bak");
        assert_eq!(backup_name(" keys.lua "), "keys.bak");
        assert_eq!(backup_name("app/ui"), "app/ui.bak");
        let dir = Path::new("/home/someone/.config/zellij/plugins");
        assert_eq!(
            resolve_plugin_file(dir, &backup_name("keys")).unwrap(),
            dir.join("keys.bak.lua")
        );
        assert!(is_backup_name("keys.bak.lua"));
        assert!(is_backup_name("app/ui.bak"));
        assert!(!is_backup_name("keys.lua"));
    }

    /// The scan is what a granted permission is read off, so it has to read Lua rather than
    /// text: a call in a comment or in a string asks for nothing at all.
    #[test]
    fn a_request_permission_that_is_not_code_grants_nothing() {
        let commented = r#"
-- zellij.request_permission { "write_files" }
--[[ zellij.request_permission { "run_commands" } ]]
local usage = [[ zellij.request_permission { "full_hd_access" } ]]
local also = "zellij.request_permission { 'read_cli_pipes' }"
zellij.request_permission { "read_files" }
"#;
        assert_eq!(
            requested_permissions(commented),
            RequestedPermissions {
                known: vec![PermissionType::ReadFiles],
                unknown: vec![],
            }
        );
    }

    /// One apostrophe used to invert the parity of every string after it, which turned the text
    /// between two permissions into a permission and lost the ones that followed.
    #[test]
    fn an_apostrophe_inside_the_braces_does_not_shift_the_strings_after_it() {
        let source = r#"
zellij.request_permission {
  "read_files",   -- it's the one this needs
  "write_files",
}
"#;
        assert_eq!(
            requested_permissions(source),
            RequestedPermissions {
                known: vec![PermissionType::ReadFiles, PermissionType::WriteFiles],
                unknown: vec![],
            }
        );
        assert!(
            lint("keys", source).is_empty(),
            "{:?}",
            lint("keys", source)
        );
    }

    /// `shared:` is a namespace only where a namespace goes. The word in a comment, or in the
    /// text a tool describes itself with, is nobody's permission problem.
    #[test]
    fn only_a_namespace_makes_the_shared_permission_a_finding() {
        let quiet = r#"
zellij.request_permission { "plugin_data" }
-- the pane reads the shared:todo rows this writes
zellij.on("permission_request_result", function()
  zellij.data.fetch { entity = "todo", description = "the shared:todo list" }
end)
"#;
        assert!(
            !lint("todo", quiet)
                .iter()
                .any(|found| found.contains("shared_plugin_data")),
            "{:?}",
            lint("todo", quiet)
        );
    }

    /// The reserved-column rule is about declarations. A value that happens to spell one is not
    /// a declaration, and a real one is named on its own line rather than on the `attrs` key's.
    #[test]
    fn only_a_declared_attribute_is_a_reserved_one() {
        let source = r#"
zellij.request_permission { "plugin_data" }
zellij.on("permission_request_result", function()
  zellij.data.define {
    name = "note", version = 1,
    attrs = {
      { "title", "text", default = "created_at" },
      { "kind", "text", default = "id" },
      { name = "updated_at", kind = "int" },
    },
  }
end)
"#;
        let found = lint("note", source);
        let reserved: Vec<&String> = found
            .iter()
            .filter(|found| found.contains("on every row already"))
            .collect();
        assert_eq!(reserved.len(), 1, "{found:?}");
        assert!(reserved[0].contains("updated_at"), "{reserved:?}");
        // On the line the attribute is declared on, not on the line `attrs` is on.
        assert!(reserved[0].starts_with("note.lua:9:"), "{reserved:?}");
    }

    /// The outline is the map a reader gets before a range read, so it has to name what the
    /// worked example is made of - its require, its load, its permission request, its handler,
    /// its define, its registration and every tool - each on the line it is on, in order.
    #[test]
    fn an_outline_of_the_todo_extension_names_its_lifecycle() {
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins/todo-extension.lua"),
        )
        .expect("the example plugin should be in the repo");
        let entries = outline(&source);
        let texts: Vec<&str> = entries.iter().map(|entry| entry.text.as_str()).collect();
        for expected in [
            "require \"zellij\"",
            "local function describe(todo)",
            "local function define()",
            "function M.load(config)",
            "on \"permission_request_result\"",
            "data.define \"todo\"",
            "zellij.hocus.tools",
            "tool \"add\"",
            "tool \"list\"",
            "tool \"complete\"",
        ] {
            assert!(
                texts.contains(&expected),
                "{expected:?} is not in {texts:?}"
            );
        }
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("request_permission { \"")),
            "{texts:?}"
        );
        assert!(
            entries.windows(2).all(|pair| pair[0].line <= pair[1].line),
            "sorted by line: {entries:?}"
        );
        // The line named is the line the thing is on, which is what a range read is built from.
        for entry in &entries {
            let line = source.lines().nth(entry.line - 1).unwrap_or_default();
            let word = match entry.kind {
                OutlineKind::Require => "require".to_owned(),
                OutlineKind::Permission => "request_permission".to_owned(),
                OutlineKind::Function => "function".to_owned(),
                OutlineKind::Handler => "zellij.on(".to_owned(),
                OutlineKind::Define => "data.define".to_owned(),
                OutlineKind::Registration => REGISTER_CALL.to_owned(),
                // A tool is the key its entry is written under: `add = {`.
                OutlineKind::Tool => format!(
                    "{} =",
                    entry
                        .text
                        .trim_start_matches("tool \"")
                        .trim_end_matches('"')
                ),
            };
            assert!(
                line.contains(&word),
                "line {} is {line:?}, not a {word}",
                entry.line
            );
        }
    }

    /// A registration is written both ways - `zellij.hocus.tools { ... }` inline, or through a
    /// `local TOOLS = { ... }` the call is handed - and its tools are the keys of that table,
    /// so a nested `parameters = { ... }` is part of a tool rather than another one.
    #[test]
    fn an_outline_reads_a_registration_in_either_spelling() {
        let inline = r#"
zellij.hocus.tools {
  add = { parameters = { properties = { title = {} } }, handler = function() end },
  list = { handler = function() end },
}
"#;
        let aliased = r#"
local TOOLS = {
  add = { parameters = { properties = { title = {} } }, handler = function() end },
  list = { handler = function() end },
}
zellij.hocus.tools(TOOLS)
"#;
        for source in [inline, aliased] {
            // Sorted by position, so the aliased spelling lists its tools before the call
            // that registers them.
            let mut texts: Vec<String> = outline(source)
                .into_iter()
                .filter(|entry| matches!(entry.kind, OutlineKind::Registration | OutlineKind::Tool))
                .map(|entry| entry.text)
                .collect();
            texts.sort();
            assert_eq!(
                texts,
                vec!["tool \"add\"", "tool \"list\"", "zellij.hocus.tools"],
                "{source}"
            );
        }
    }

    /// Every spelling of a named function is one definition, and an anonymous one is not: a
    /// handler's callback would otherwise be listed twice, once as the handler and once as a
    /// nameless function.
    #[test]
    fn every_named_spelling_is_a_definition_and_an_anonymous_function_is_not() {
        let source = r#"
local M = {}
local Text = {}
function M.load(config) end
local function tick() end
M.render = function(rows, cols) return "" end
function Text:len() return 0 end
zellij.on("key", function(event) return true end)
table.insert(M.jobs, function() end)
return M
"#;
        let entries = outline(source);
        let functions: Vec<&OutlineEntry> = entries
            .iter()
            .filter(|entry| entry.kind == OutlineKind::Function)
            .collect();
        let texts: Vec<&str> = functions.iter().map(|entry| entry.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "function M.load(config)",
                "local function tick()",
                "M.render = function(rows, cols)",
                "function Text:len()",
            ]
        );
        assert_eq!(
            functions.iter().map(|entry| entry.line).collect::<Vec<_>>(),
            vec![4, 5, 6, 7]
        );
        let handlers: Vec<&OutlineEntry> = entries
            .iter()
            .filter(|entry| entry.kind == OutlineKind::Handler)
            .collect();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].text, "on \"key\"");
        assert_eq!(handlers[0].line, 8);
    }

    /// The outline is read off the code, not the text: a definition quoted in a comment or a
    /// string is not one - and neither is a variable that happens to be called `on`.
    #[test]
    fn an_outline_ignores_what_is_only_written_about() {
        let source = r#"
-- function M.commented() end
local usage = [[ zellij.on("key", function() end) ]]
local on = true
if on then zellij.log("x") end
function M.real() end
"#;
        let entries = outline(source);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].text, "function M.real()");
        assert_eq!(entries[0].line, 6);
    }

    /// A finding at the very start of a line used to be reported one line early: `lines()` does
    /// not count a trailing line break as a line.
    #[test]
    fn a_call_at_the_start_of_a_line_is_on_that_line() {
        let source = "local M = {}\nzellij.request_permission { \"plugin_dat\" }\n";
        let found = lint("keys", source).join("\n");
        assert!(found.starts_with("keys.lua:2:"), "{found}");
    }
}
