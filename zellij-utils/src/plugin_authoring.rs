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

/// The permission strings a plugin's source asks for in `zellij.request_permission { ... }`:
/// the ones zellij knows, and the ones it does not.
///
/// Read lexically, not by running the Lua: every quoted string inside the braces of a
/// `request_permission` call, in either spelling (`"read_files"` or `"ReadFiles"`), which is
/// what the prelude accepts. Anything more elaborate - a permission list built at runtime -
/// is not seen, and the plugin's own call reports it in the ordinary way.
///
/// `unknown` is kept rather than dropped because a name zellij does not know is *silently*
/// ungranted: hocus pre-grants what this finds when a write is approved, so a typo becomes a
/// denial much later, in a pane nobody is looking at. [`lint`] is what says so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestedPermissions {
    pub known: Vec<PermissionType>,
    pub unknown: Vec<String>,
}

/// Every quoted string inside a `request_permission` call, with its byte offset.
fn permission_strings(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(hit) = source[at..].find("request_permission") {
        let after = at + hit + "request_permission".len();
        let Some(open) = source[after..].find(['{', '(']) else {
            break;
        };
        // Only whitespace may sit between the name and its argument, or this is some other
        // identifier that happens to end the same way.
        if !source[after..after + open].trim().is_empty() {
            at = after;
            continue;
        }
        let open = after + open;
        let closer = if source.as_bytes()[open] == b'{' {
            '}'
        } else {
            ')'
        };
        let Some(close) = source[open..].find(closer) else {
            break;
        };
        let close = open + close;
        let inside = &source[open + 1..close];
        let mut offset = open + 1;
        for (i, part) in inside.split(['"', '\'']).enumerate() {
            if i % 2 == 1 {
                found.push((offset, part.to_owned()));
            }
            offset += part.len() + 1;
        }
        at = close;
    }
    found
}

/// The permissions a source asks for, split into what zellij knows and what it does not.
pub fn requested_permissions(source: &str) -> RequestedPermissions {
    let mut out = RequestedPermissions::default();
    for (_, quoted) in permission_strings(source) {
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

/// The `zellij.data` calls a namespace or an attribute list can appear in.
const DATA_CALLS: [&str; 5] = [
    "data.define",
    "data.fetch",
    "data.watch",
    "data.count",
    "data.apply",
];

/// The text of one call's arguments: from the call to the end of its outermost bracket, or a
/// window's worth of source when the brackets do not balance inside it.
///
/// Lexical on purpose. A `namespace` key means one thing inside `zellij.data.fetch` and quite
/// another inside a hocus extension's registration payload, where it names the tool namespace
/// and is nothing to do with the store - so a rule about namespaces may only look at the
/// arguments of a data call, never at the file as a whole.
const CALL_WINDOW: usize = 2_000;

fn call_arguments(source: &str, at: usize) -> &str {
    // Clamped to a character boundary: a plugin's comments are as likely to be Japanese as
    // English, and slicing a source in the middle of one would panic rather than mis-lint.
    let mut end = source.len().min(at + CALL_WINDOW);
    while end > at && !source.is_char_boundary(end) {
        end -= 1;
    }
    let rest = &source[at..end];
    let Some(open) = rest.find(['{', '(']) else {
        return rest;
    };
    let opener = rest.as_bytes()[open];
    let closer = if opener == b'{' { b'}' } else { b')' };
    let mut depth = 0usize;
    for (i, byte) in rest.as_bytes().iter().enumerate().skip(open) {
        if *byte == opener {
            depth += 1;
        } else if *byte == closer {
            depth -= 1;
            if depth == 0 {
                return &rest[open..=i];
            }
        }
    }
    rest
}

/// Every `zellij.data.*` call site in `source`, as (offset of the call, its arguments).
fn data_calls(source: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    for call in DATA_CALLS {
        let mut at = 0;
        while let Some(hit) = source[at..].find(call) {
            let start = at + hit;
            found.push((start, call_arguments(source, start)));
            at = start + call.len();
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found
}

/// Where a subslice of `source` begins in it.
fn offset_in(source: &str, part: &str) -> usize {
    part.as_ptr() as usize - source.as_ptr() as usize
}

/// The one positional string a `data.apply(mutations, "namespace")` carries: the literal after
/// a comma in the call's own argument list, never one nested in a mutation.
fn positional_namespace(arguments: &str) -> Option<(usize, &str)> {
    let bytes = arguments.as_bytes();
    let mut depth = 0usize;
    let mut after_comma = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
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
                let quote = bytes[i] as char;
                let rest = &arguments[i + 1..];
                let end = rest.find(quote)?;
                return Some((i + 1, &rest[..end]));
            },
            b' ' | b'\t' | b'\n' | b'\r' => {},
            _ => after_comma = false,
        }
        i += 1;
    }
    None
}

/// The string literal a `key = "..."` carries, if it is a literal at all.
fn literal_value<'a>(text: &'a str, key: &str) -> Option<(usize, &'a str)> {
    let at = text.find(key)?;
    let rest = &text[at + key.len()..];
    let equals = rest.find('=')?;
    if !rest[..equals].trim().is_empty() {
        return None;
    }
    let after = &rest[equals + 1..];
    let quote = after.find(['"', '\''])?;
    // A value that is not a literal - `namespace = M.config.name` - is not ours to judge.
    if !after[..quote].trim().is_empty() {
        return None;
    }
    let quoted = &after[quote + 1..];
    let end = quoted.find(after.as_bytes()[quote] as char)?;
    let offset = at + key.len() + equals + 1 + quote + 1;
    Some((offset, &quoted[..end]))
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
    let file = name
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or("plugin")
        .trim_end_matches(".lua");
    let say =
        |offset: usize, what: String| format!("{file}.lua:{}: {what}", line_of(source, offset));
    let mut out = Vec::new();
    let permissions = requested_permissions(source);
    let asked = |permission: PermissionType| permissions.known.contains(&permission);

    // A permission zellij does not know is granted to nobody and refused to no one's face.
    for (offset, unknown) in permission_strings(source) {
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

    let mut uses_data = false;
    for (at, arguments) in data_calls(source) {
        uses_data = true;
        let args_at = offset_in(source, arguments);
        let named = literal_value(arguments, "namespace");
        let positional = match source[at..].starts_with("data.apply") {
            true => positional_namespace(arguments),
            false => None,
        };
        for (offset, namespace) in named.into_iter().chain(positional) {
            if let Err(error) = validate_namespace(Some(namespace)) {
                out.push(say(args_at + offset, format!("{error}")));
            }
        }
        if !source[at..].starts_with("data.define") {
            continue;
        }
        // An entity may be called `id`; an attribute may not, so only the attribute list is
        // read - everything after the `attrs` key.
        let Some(attrs) = arguments.find("attrs") else {
            continue;
        };
        let attrs_at = args_at + attrs;
        for (i, part) in arguments[attrs..].split(['"', '\'']).enumerate() {
            if i % 2 == 1 && RESERVED_ATTR_NAMES.contains(&part) {
                out.push(say(
                    attrs_at,
                    format!(
                        "{part:?} is on every row already and cannot be declared - a define \
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
    if let Some(at) = source.find(SHARED_NAMESPACE_PREFIX) {
        if !asked(PermissionType::SharedPluginData) {
            out.push(say(
                at,
                "a `shared:` namespace needs `shared_plugin_data` as well as `plugin_data`, in \
                 every plugin that touches it - the reader too."
                    .to_owned(),
            ));
        }
    }
    if uses_data && !source.contains("permission_request_result") {
        out.push(format!(
            "{file}.lua: a permission is not in force while `load` runs - it is granted a moment \
             later - so a data call made from `load` is refused. Define and fetch from a \
             `zellij.on(\"permission_request_result\", ...)` handler instead."
        ));
    }
    if name.trim().starts_with("hocus/")
        && source.contains("hocus_extension_register")
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
}
