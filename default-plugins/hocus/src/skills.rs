//! Prompts kept on disk and typed as slash commands.
//!
//! A skill is a `SKILL.md` — optional YAML frontmatter carrying `name` and
//! `description`, then a markdown body — in a folder named after it. Typing
//! `/<name> <text>` sends the body to the model as a user turn, with the
//! trailing text as its arguments. That is the whole feature: a skill is a
//! prompt, never a tool, so whatever the model does with it goes through the
//! same confirmation the model's own calls get.
//!
//! Two folders are read, and only these two:
//!
//! - `<project root>/.hocus/skills/<name>/SKILL.md` for the active project;
//! - `<user plugin dir>/hocus/skills/<name>/SKILL.md`, or the KDL `skills_dir`.
//!
//! Hocus is its own agent with its own tools, so it never reads another
//! agent's skill folders (`.claude/`, `.agents/`): a skill written for Claude
//! Code names tools hocus does not have. The file format is borrowed so that
//! nothing new has to be learned to write one.
//!
//! A wasm plugin cannot open a file outside its preopens, so the folders are
//! read by a shell script run on the host (`scan_argv`) whose output is parsed
//! here (`parse_listing`). Everything in this module is pure for that reason:
//! the tests feed it strings.

use std::collections::{BTreeMap, BTreeSet};

use hocus_proto::paths;

/// Context key on the `RunCommandResult` that answers a scan.
pub const SCAN_CONTEXT: &str = "hocus_skill_scan";
/// Context key naming which tier the scan was for.
pub const SOURCE_CONTEXT: &str = "hocus_skill_source";
/// Context key carrying the id of the scan being answered. Two scans of the
/// same tier can be out at once — a project switch starts one before the last
/// has landed — and they may answer in either order, so a result names the
/// request it belongs to and a result for anything but the newest is dropped.
pub const SCAN_ID_CONTEXT: &str = "hocus_skill_scan_id";

/// The most a body may be. A skill is re-sent on every later turn of the
/// conversation, the same as a tool result, so a long one is a standing cost.
pub const MAX_SKILL_BYTES: usize = 24_000;
/// How many skills one tier may hold.
pub const MAX_SKILLS: usize = 32;
/// The output cap handed to the host for a scan: every skill at its cap, with
/// room for the record framing. Past it the host cuts the listing and marks
/// it, and `parse_listing` says so.
pub const MAX_SCAN_OUTPUT: usize = MAX_SKILLS * (MAX_SKILL_BYTES + 1024);
const MAX_NAME_LEN: usize = 64;
/// The placeholder a body may use for the text typed after the command.
pub const ARGUMENTS: &str = "$ARGUMENTS";
/// What the host appends when it cuts a command's output at the limit.
const TRUNCATED: &str = "\n[truncated]";

/// Record framing in the scan output: a path, its enabled state, then a body. Neither byte can
/// occur in a markdown file, so nothing needs escaping.
const PATH_START: char = '\u{1}';
const BODY_START: char = '\u{2}';
const DISABLED: char = '\u{3}';

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// `<project root>/.hocus/skills`. Shadows the user's.
    Project,
    /// The user's own folder.
    User,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Project => "project",
            Source::User => "user",
        }
    }

    pub fn parse(label: &str) -> Option<Source> {
        match label {
            "project" => Some(Source::Project),
            "user" => Some(Source::User),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
    pub source: Source,
    pub enabled: bool,
    /// Things worth telling the author that did not stop the skill loading.
    pub warnings: Vec<String>,
}

/// What one scan produced: the skills that loaded, the files that did not and
/// why, and anything wrong with the listing as a whole.
#[derive(Debug, Default, PartialEq)]
pub struct Listing {
    pub skills: Vec<Skill>,
    pub skipped: Vec<(String, String)>,
    pub errors: Vec<String>,
}

/// Every loaded skill, by tier. Host state: one registry serves every chat.
#[derive(Default)]
pub struct Registry {
    tiers: BTreeMap<Source, BTreeMap<String, Skill>>,
    skipped: BTreeMap<Source, Vec<(String, String)>>,
    errors: BTreeMap<Source, Vec<String>>,
}

impl Registry {
    /// The skill behind `/name`, the project's ahead of the user's.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        [Source::Project, Source::User].iter().find_map(|source| {
            self.tiers
                .get(source)?
                .get(name)
                .filter(|skill| skill.enabled)
        })
    }

    /// Every name that can be typed, each once, sorted.
    pub fn names(&self) -> Vec<String> {
        self.tiers
            .values()
            .flat_map(|tier| {
                tier.values()
                    .filter(|skill| skill.enabled)
                    .map(|skill| skill.name.clone())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Every loaded skill, the project's first, sorted within a tier.
    pub fn all(&self) -> Vec<&Skill> {
        [Source::Project, Source::User]
            .iter()
            .filter_map(|source| self.tiers.get(source))
            .flat_map(|tier| tier.values())
            .collect()
    }

    pub fn is_shadowed(&self, skill: &Skill) -> bool {
        skill.source == Source::User
            && skill.enabled
            && self
                .tiers
                .get(&Source::Project)
                .and_then(|tier| tier.get(&skill.name))
                .is_some_and(|project| project.enabled)
    }

    pub fn skipped(&self) -> Vec<(Source, &str, &str)> {
        self.skipped
            .iter()
            .flat_map(|(source, entries)| {
                entries
                    .iter()
                    .map(move |(name, why)| (*source, name.as_str(), why.as_str()))
            })
            .collect()
    }

    pub fn errors(&self) -> Vec<(Source, &str)> {
        self.errors
            .iter()
            .flat_map(|(source, errors)| errors.iter().map(move |error| (*source, error.as_str())))
            .collect()
    }

    /// Replace one tier with what a scan found. `reserved` holds every name a
    /// skill may not take — the tools, the plugin's own commands, the
    /// extensions — since a skill that shadowed `/close_pane` would turn a
    /// command that runs a tool into one that sends a prompt.
    pub fn replace(&mut self, source: Source, listing: Listing, reserved: &BTreeSet<String>) {
        let mut tier = BTreeMap::new();
        let mut skipped = listing.skipped;
        for skill in listing.skills {
            if reserved.contains(&skill.name) {
                skipped.push((
                    skill.name,
                    "the name is taken by a tool or command".to_owned(),
                ));
                continue;
            }
            if tier.len() >= MAX_SKILLS {
                skipped.push((
                    skill.name,
                    format!("only the first {MAX_SKILLS} skills in a folder are loaded"),
                ));
                continue;
            }
            tier.insert(skill.name.clone(), skill);
        }
        self.tiers.insert(source, tier);
        self.skipped.insert(source, skipped);
        self.errors.insert(source, listing.errors);
    }

    /// Forget a tier: the project went away, or its folder is not readable.
    pub fn clear(&mut self, source: Source) {
        self.tiers.remove(&source);
        self.skipped.remove(&source);
        self.errors.remove(&source);
    }

    /// A scan that could not run leaves the tier as it was and says why.
    pub fn set_error(&mut self, source: Source, error: String) {
        self.errors.insert(source, vec![error]);
    }
}

/// The command that reads every `*/SKILL.md` under `roots`, as argv.
///
/// A shell loop rather than `find`, because the glob is the whole of the
/// rule — one folder deep, one file name — and it needs no binary beyond `sh`.
/// The record framing is `\x01<path>\x02<body>\n`; `head -c` one past the cap
/// is what lets "too long" be told from "exactly at the limit" without the
/// host reading an enormous file whole. Missing folders match nothing, which
/// is the right answer, not an error — but a folder that is there and cannot be
/// entered, or a file that cannot be read, exits non-zero, because those are the
/// cases where an empty listing would otherwise look like an emptied folder and
/// unload the tier.
fn scan_script() -> String {
    format!(
        r#"status=0
for root in "$@"; do
  if [ -d "$root" ] && {{ [ ! -r "$root" ] || [ ! -x "$root" ]; }}; then
    printf '%s: the folder could not be read\n' "$root" >&2
    status=1
    continue
  fi
  for f in "$root"/*/SKILL.md; do
    [ -f "$f" ] || continue
    printf '\001%s' "$f"
    [ -f "${{f%/SKILL.md}}/.disabled" ] && printf '\003'
    printf '\002'
    if ! head -c {} -- "$f"; then
      printf '%s: the file could not be read\n' "$f" >&2
      status=1
    fi
    printf '\n'
  done
done
exit $status"#,
        MAX_SKILL_BYTES + 1
    )
}

pub fn scan_argv(roots: &[String]) -> Vec<String> {
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        scan_script(),
        "sh".to_owned(),
    ];
    argv.extend(roots.iter().cloned());
    argv
}

const SKILL_SOURCE_VAR: &str = "HOCUS_SKILL_SOURCE";

/// Resolve one skill below a trusted tier root. The root may itself be a symlink (a configured
/// dotfiles folder commonly is); the model-chosen child and its files may not be.
pub fn resolve_paths(root: &str, name: Option<&str>) -> Result<(String, String, String), String> {
    let root = paths::normalize(root.trim())
        .map_err(|_| "the skill folder is not an absolute path".to_owned())?;
    let name = name.map(str::trim).unwrap_or_default();
    valid_name(name)?;
    let dir = paths::normalize(&format!("{root}/{name}"))
        .map_err(|_| format!("{name:?} is not a skill name"))?;
    if !std::path::Path::new(&dir).starts_with(&root) {
        return Err(format!("{name:?} is not a skill name"));
    }
    Ok((
        dir.clone(),
        format!("{dir}/SKILL.md"),
        format!("{dir}/.disabled"),
    ))
}

/// Validate a complete file before handing it to the host.
pub fn validate_source(name: &str, source: &str) -> Result<(), String> {
    valid_name(name)?;
    if source.len() > MAX_SKILL_BYTES {
        return Err(format!(
            "source is {} bytes; the limit is {MAX_SKILL_BYTES}. Shorten the skill.",
            source.len()
        ));
    }
    parse_skill(Source::User, name, "SKILL.md", source).map(|_| ())
}

/// Turn a skill filesystem tool into one host command. Mutations finish by printing a fresh scan
/// of the affected tier, so the registry and command completion change before the tool answers.
pub fn tool_argv(
    tool: &str,
    root: &str,
    name: Option<&str>,
    source: Option<&str>,
    overwrite: bool,
) -> Result<(Vec<String>, BTreeMap<String, String>), String> {
    let root = paths::normalize(root.trim())
        .map_err(|_| "the skill folder is not an absolute path".to_owned())?;
    let name = name.map(str::trim).unwrap_or_default();
    let (dir, file, marker) = resolve_paths(&root, Some(name))?;
    let guard = r#"if [ -L "$2" ] || [ -L "$3" ] || [ -L "$4" ]; then printf '%s\n' "$2 contains a symlink; refusing" >&2; exit 1; fi"#;
    let mut env = BTreeMap::new();
    let script = match tool {
        "skill_read" => format!(
            "{guard}\nif [ ! -f \"$3\" ]; then printf '%s\\n' \"$3: no such skill\" >&2; exit 1; fi\nexec head -c {} -- \"$3\"",
            MAX_SKILL_BYTES + 1
        ),
        "skill_write" => {
            let source = source.ok_or("source is required: send the complete SKILL.md")?;
            validate_source(name, source)?;
            env.insert(
                SKILL_SOURCE_VAR.to_owned(),
                format!("{}\n", source.trim_end_matches('\n')),
            );
            let noclobber = if overwrite { "" } else { "set -C\n" };
            format!(
                "{guard}\nmkdir -p \"$1\" || exit 1\nmkdir -p \"$2\" || exit 1\n{noclobber}printf %s \"$HOCUS_SKILL_SOURCE\" > \"$3\" || exit 1\nset -- \"$1\"\n{}",
                scan_script()
            )
        },
        "skill_disable" => format!(
            "{guard}\nif [ ! -f \"$3\" ]; then printf '%s\\n' \"$3: no such skill\" >&2; exit 1; fi\n: > \"$4\" || exit 1\nset -- \"$1\"\n{}",
            scan_script()
        ),
        "skill_enable" => format!(
            "{guard}\nif [ ! -f \"$3\" ]; then printf '%s\\n' \"$3: no such skill\" >&2; exit 1; fi\nrm -f -- \"$4\" || exit 1\nset -- \"$1\"\n{}",
            scan_script()
        ),
        other => return Err(format!("no such tool: {other}")),
    };
    Ok((
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            script,
            "sh".to_owned(),
            root,
            dir,
            file,
            marker,
        ],
        env,
    ))
}

/// Turn a scan's stdout into skills.
pub fn parse_listing(source: Source, stdout: &str) -> Listing {
    let mut listing = Listing::default();
    let mut stdout = stdout;
    if let Some(cut) = stdout.strip_suffix(TRUNCATED) {
        listing.errors.push(format!(
            "the listing was cut at {} bytes; skills after that point were not read",
            cut.len()
        ));
        // The last record is the one that was cut through.
        stdout = match cut.rfind(PATH_START) {
            Some(at) => &cut[..at],
            None => "",
        };
    }
    for record in stdout.split(PATH_START).skip(1) {
        let Some((path, body)) = record.split_once(BODY_START) else {
            listing
                .errors
                .push(format!("unreadable record in the listing: {record:?}"));
            continue;
        };
        let body = body.strip_suffix('\n').unwrap_or(body);
        let (path, enabled) = match path.strip_suffix(DISABLED) {
            Some(path) => (path, false),
            None => (path, true),
        };
        let name = skill_name(path);
        if let Err(why) = valid_name(&name) {
            listing.skipped.push((name, why));
            continue;
        }
        if body.len() > MAX_SKILL_BYTES {
            listing.skipped.push((
                name,
                format!("the body is over {MAX_SKILL_BYTES} bytes; shorten it"),
            ));
            continue;
        }
        match parse_skill(source, &name, path, body) {
            Ok(mut skill) => {
                skill.enabled = enabled;
                listing.skills.push(skill);
            },
            Err(why) => listing.skipped.push((name, why)),
        }
    }
    listing
}

/// The folder the file sits in — `/a/b/bench/SKILL.md` is `bench`.
fn skill_name(path: &str) -> String {
    let parent = path.rsplit('/').nth(1).unwrap_or_default();
    parent.to_owned()
}

/// A skill's name is a command, so it is held to what a command can be:
/// lowercase, digits, `_` and `-`, never option-shaped.
pub fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the folder has no name".to_owned());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "a skill name may be at most {MAX_NAME_LEN} characters"
        ));
    }
    let mut chars = name.chars();
    let first = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest =
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'));
    if first && rest {
        Ok(())
    } else {
        Err(
            "a skill name is lowercase letters, digits, '_' and '-', starting with a letter or \
             digit"
                .to_owned(),
        )
    }
}

fn parse_skill(source: Source, name: &str, path: &str, text: &str) -> Result<Skill, String> {
    let (frontmatter, body, mut warnings) = split_frontmatter(text);
    let body = trim_blank_lines(body);
    if body.is_empty() {
        return Err("the file has no body after the frontmatter".to_owned());
    }
    if let Some(declared) = frontmatter.get("name") {
        if declared != name {
            warnings.push(format!(
                "the frontmatter says name: {declared}, but the folder is {name}; the folder \
                 wins"
            ));
        }
    }
    let description = match frontmatter.get("description") {
        Some(description) if !description.trim().is_empty() => one_line(description),
        _ => one_line(
            body.lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default(),
        ),
    };
    Ok(Skill {
        name: name.to_owned(),
        description,
        body: body.to_owned(),
        path: path.to_owned(),
        source,
        enabled: true,
        warnings,
    })
}

/// A description is shown on one line of a list.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_blank_lines(text: &str) -> &str {
    text.trim_matches(|c: char| c == '\n' || c == '\r' || c == ' ' || c == '\t')
}

/// The subset of YAML a skill header uses: `---` fences, top-level `key:
/// value` scalars, `|` and `>` block scalars, comments. Anything nested is
/// stepped over — a `metadata:` map carries nothing hocus reads. Not a YAML
/// parser, and not pretending to be one: an unfenced file is all body, and an
/// unterminated fence is a warning plus all body.
fn split_frontmatter(text: &str) -> (BTreeMap<String, String>, &str, Vec<String>) {
    let mut warnings = Vec::new();
    let Some(after_open) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (BTreeMap::new(), text, warnings);
    };
    let mut header_end = None;
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            header_end = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((header_len, body_at)) = header_end else {
        warnings.push("the frontmatter is never closed; the whole file is the body".to_owned());
        return (BTreeMap::new(), text, warnings);
    };
    let header = &after_open[..header_len];
    let body = &after_open[body_at..];

    let mut fields = BTreeMap::new();
    let lines: Vec<&str> = header.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        // Indented: the continuation of a nested map nobody asked for.
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            continue;
        }
        let value = value.trim();
        let value = match value {
            "|" | ">" | "|-" | ">-" => {
                let mut block = Vec::new();
                while index < lines.len()
                    && (lines[index].trim().is_empty() || lines[index].starts_with([' ', '\t']))
                {
                    block.push(lines[index].trim());
                    index += 1;
                }
                if value.starts_with('|') {
                    block.join("\n")
                } else {
                    block.join(" ")
                }
            },
            _ => unquote(value).to_owned(),
        };
        fields.insert(key.to_owned(), value);
    }
    (fields, body, warnings)
}

fn unquote(value: &str) -> &str {
    let stripped = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        });
    stripped.unwrap_or(value)
}

/// What the model is sent for `/<name> <tail>`.
///
/// The body is wrapped in a tag that names the skill and its file: the model
/// is being handed text nobody typed, and an unmarked block of instructions
/// would read as the user's own words. `$ARGUMENTS` in the body takes the
/// tail; a body without it gets the tail appended after the tag, so the
/// arguments are always somewhere the model can see them.
pub fn expand(skill: &Skill, tail: &str) -> String {
    let tail = tail.trim();
    let takes_arguments = skill.body.contains(ARGUMENTS);
    let body = if takes_arguments {
        skill.body.replace(ARGUMENTS, tail)
    } else {
        skill.body.clone()
    };
    let mut out = format!(
        "<skill name=\"{}\" path=\"{}\">\n{body}\n</skill>",
        skill.name,
        skill.path.replace('"', "'")
    );
    if !takes_arguments && !tail.is_empty() {
        out.push_str("\n\n");
        out.push_str(tail);
    }
    out
}

/// `/bench fix the flaky test` as `("bench", "fix the flaky test")`. `None`
/// for a line that is not a command at all.
pub fn split_command(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix('/')?;
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, tail) = rest.split_at(name_end);
    (!name.is_empty()).then(|| (name, tail.trim()))
}

#[cfg(test)]
#[path = "./unit/skills_tests.rs"]
mod tests;
