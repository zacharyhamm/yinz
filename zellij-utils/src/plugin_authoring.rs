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

/// Every string literal inside a `request_permission` call, with its byte offset.
fn permission_strings(lexed: &Lexed) -> Vec<(usize, String)> {
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
        found.extend(
            lexed
                .strings_in(open..close)
                .map(|(at, value)| (at, value.to_owned())),
        );
        at = close;
    }
    found
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

// Blocks, and what runs while a plugin loads -------------------------------------------------

/// A `function ... end` in the source: where its keyword is, where its `end` finishes, and
/// whether it is the plugin's `load`.
struct FunctionBlock {
    at: usize,
    end: usize,
    is_load: bool,
}

/// Every `function ... end` in `code`, found by pairing block keywords: `function`, `if` and
/// `do` open a block, `end` closes one, and `repeat`/`until` pair on their own. That covers
/// Lua's block grammar for this purpose - `for x = 1, 2 do` and `while c do` are closed by the
/// `do` they carry, and `then`, `else` and `elseif` open nothing.
fn function_blocks(code: &str) -> Vec<FunctionBlock> {
    let mut open: Vec<(usize, bool, bool)> = Vec::new();
    let mut blocks = Vec::new();
    for (at, word) in words(code) {
        match word {
            "function" => open.push((at, true, is_load_function(code, at))),
            "if" | "do" | "repeat" => open.push((at, false, false)),
            "until" => {
                open.pop();
            },
            "end" => {
                if let Some((start, is_function, is_load)) = open.pop() {
                    if is_function {
                        blocks.push(FunctionBlock {
                            at: start,
                            end: at + "end".len(),
                            is_load,
                        });
                    }
                }
            },
            _ => {},
        }
    }
    blocks
}

/// Whether the `function` keyword at `at` opens the plugin's `load`, in either spelling a
/// plugin uses: `function M.load(config)` or `M.load = function(config)`.
fn is_load_function(code: &str, at: usize) -> bool {
    let after = &code[at + "function".len()..];
    let named: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | ' ' | '\t'))
        .collect();
    let named = named.trim();
    if named == "load" || named.ends_with(".load") || named.ends_with(":load") {
        return true;
    }
    let before = code[..at].trim_end();
    let Some(before) = before.strip_suffix('=') else {
        return false;
    };
    let before = before.trim_end();
    before == "load" || before.ends_with(".load") || before.ends_with(" load")
}

/// Whether the call at `at` runs while the plugin is loading: in the chunk's own body, or in
/// `load` itself.
///
/// This is the whole of the permission timing trap. A grant arrives as an event, and no event
/// has arrived yet at either of those moments, so a gated call made there is refused however
/// long ago the permission was granted. The same call from an `on(...)` handler, from `pipe`
/// or from `update` is made after the answer and is fine.
fn runs_at_load(blocks: &[FunctionBlock], at: usize) -> bool {
    blocks
        .iter()
        .filter(|block| (block.at..block.end).contains(&at))
        .max_by_key(|block| block.at)
        .is_none_or(|block| block.is_load)
}

/// The 1-based line a byte offset falls on.
fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].lines().count().max(1)
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

    let blocks = function_blocks(&lexed.code);
    let mut uses_data = false;
    let mut said_shared = false;
    let mut said_load = false;
    for (at, arguments) in data_calls(&lexed) {
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
        if !said_load && runs_at_load(&blocks, at) {
            said_load = true;
            out.push(say(
                at,
                "a permission is not in force while `load` runs - it is granted a moment later - \
                 so this data call is refused. Define and fetch from a \
                 `zellij.on(\"permission_request_result\", ...)` handler instead."
                    .to_owned(),
            ));
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
    let registers = lexed
        .strings
        .iter()
        .any(|(_, value)| value == "hocus_extension_register");
    if name.trim().starts_with("hocus/")
        && registers
        && !asked(PermissionType::MessageAndLaunchOtherPlugins)
    {
        out.push(format!(
            "{file}.lua: registering with hocus is a plugin message, so it needs \
             `zellij.request_permission {{ \"message_and_launch_other_plugins\" }}`."
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
    fn a_data_call_from_load_is_named_as_the_denial_it_will_be() {
        let source = r#"
function M.load()
  zellij.request_permission { "plugin_data" }
  zellij.data.define { name = "note", version = 1, attrs = { { "title", "text" } } }
end
"#;
        let found = lint("note", source).join("\n");
        assert!(found.contains("permission_request_result"), "{found}");
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
local function register()
  zellij.message_to_plugin { message_name = "hocus_extension_register" }
end
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

    /// The rule is "this runs before the answer arrives", not "this file has no handler": a
    /// plugin that does all its store work from a key handler is doing it right.
    #[test]
    fn a_data_call_that_does_not_run_at_load_is_not_a_finding() {
        let source = r#"
local M = {}
function M.load()
  zellij.request_permission { "plugin_data" }
end
zellij.on("key", function(event)
  M.notes = zellij.data.fetch { entity = "note" }
  return true
end)
return M
"#;
        assert_eq!(lint("note", source), Vec::<String>::new());
        // And a define left in `load` is still named, handler in the file or not.
        let still_wrong = source.replace(
            "  zellij.request_permission { \"plugin_data\" }",
            "  zellij.request_permission { \"plugin_data\" }\n  zellij.data.define { name = \"note\", version = 1 }",
        );
        assert!(
            lint("note", &still_wrong)
                .iter()
                .any(|found| found.contains("permission_request_result")),
            "{:?}",
            lint("note", &still_wrong)
        );
    }

    /// A call in the chunk's own body runs at load time too, and is refused for the same reason.
    #[test]
    fn a_data_call_in_the_chunk_body_runs_at_load_time() {
        let source = r#"
zellij.request_permission { "plugin_data" }
local notes = zellij.data.fetch { entity = "note" }
"#;
        assert!(
            lint("note", source)
                .iter()
                .any(|found| found.contains("permission_request_result")),
            "{:?}",
            lint("note", source)
        );
    }
}
