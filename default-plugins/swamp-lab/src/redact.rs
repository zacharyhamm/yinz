//! Redaction of issue content before it leaves the machine.
//!
//! A port of the `swamp` CLI's `src/domain/issues/content_redactor.ts`: the same
//! patterns, applied in the same order, with the same placeholders and the same
//! summary categories, so an issue filed from here reads like one the CLI files.
//!
//! The patterns are the TypeScript source's, character for character, passed
//! through `js` to give `\d`, `\w`, `\s` and `\b` their JavaScript meanings.
//! Two things the `regex` crate lacks had to be rewritten, each noted where it
//! happens: lookahead (`BEARER`, `LONG_BASE64`) and case-insensitive matching
//! (this build has no Unicode case tables; see `any_case`).

use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::Serialize;

/// What a redaction was, for the summary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Secret,
    Hostname,
    Email,
    NationalId,
    CreditCard,
    PhoneNumber,
    HomeDirectoryPath,
    IpAddress,
}

impl Category {
    /// The name the TypeScript summary uses, which [`Summary::describe`] pluralizes.
    pub fn name(self) -> &'static str {
        match self {
            Category::Secret => "secret",
            Category::Hostname => "hostname",
            Category::Email => "email",
            Category::NationalId => "national ID",
            Category::CreditCard => "credit card",
            Category::PhoneNumber => "phone number",
            Category::HomeDirectoryPath => "home directory path",
            Category::IpAddress => "IP address",
        }
    }
}

/// How many redactions of each category, in the order each was first made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: usize,
    pub categories: Vec<(Category, usize)>,
}

impl Summary {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    pub fn count(&self, category: Category) -> usize {
        self.categories
            .iter()
            .find(|(c, _)| *c == category)
            .map_or(0, |(_, n)| *n)
    }

    /// The log line `formatRedactionSummary` prints: `Redacted 2 emails, 1 secret
    /// from issue content.`, or nothing when nothing was redacted.
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("Redacted {self} from issue content.")
    }

    fn add(&mut self, category: Category) {
        self.total += 1;
        match self.categories.iter_mut().find(|(c, _)| *c == category) {
            Some((_, n)) => *n += 1,
            None => self.categories.push((category, 1)),
        }
    }
}

/// The counts alone: `3 secrets, 2 IP addresses`.
impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (category, n)) in self.categories.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{n} {}", pluralize(category.name(), *n))?;
        }
        Ok(())
    }
}

fn pluralize(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_string()
    } else if word.ends_with('s') || word.ends_with('x') {
        format!("{word}es")
    } else {
        format!("{word}s")
    }
}

/// One line redaction changed, so the reporter can see what was altered. When a
/// redaction consumed newlines, one change spans the affected block and
/// `original`/`redacted` hold its lines joined with `\n`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineChange {
    /// 1-based.
    pub line_number: usize,
    pub original: String,
    pub redacted: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    pub text: String,
    pub summary: Summary,
    pub changes: Vec<LineChange>,
}

/// A title and body redacted together; `summary` covers both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleAndBody {
    pub title: Redaction,
    pub body: Redaction,
    pub summary: Summary,
}

/// Redacts sensitive and identifying information from `text`.
///
/// Placeholders are stable (`[HOST-1]`, `[IP-1]`, ...): the same raw value maps
/// to the same placeholder throughout, which keeps the diagnostic structure
/// without the identifying detail.
pub fn redact(text: &str) -> Redaction {
    let mut placeholders = Placeholders::default();
    let mut summary = Summary::default();
    let redacted = apply(text, &mut placeholders, &mut summary);
    Redaction {
        changes: diff_lines(text, &redacted),
        text: redacted,
        summary,
    }
}

/// Redacts a title and body with one placeholder map, so a value in both (an IP,
/// say) gets the same placeholder in both.
pub fn redact_title_and_body(title: &str, body: &str) -> TitleAndBody {
    let mut placeholders = Placeholders::default();
    let mut summary = Summary::default();
    let title_text = apply(title, &mut placeholders, &mut summary);
    let title_summary = summary.clone();
    let body_text = apply(body, &mut placeholders, &mut summary);
    // Categories the body did not add are left out rather than listed at zero,
    // which the TypeScript does and nothing there displays.
    let mut body_summary = Summary::default();
    for (category, n) in &summary.categories {
        let n = n - title_summary.count(*category);
        if n > 0 {
            body_summary.total += n;
            body_summary.categories.push((*category, n));
        }
    }
    TitleAndBody {
        title: Redaction {
            changes: diff_lines(title, &title_text),
            text: title_text,
            summary: title_summary,
        },
        body: Redaction {
            changes: diff_lines(body, &body_text),
            text: body_text,
            summary: body_summary,
        },
        summary,
    }
}

const MAX_DETAIL_LENGTH: usize = 160;

/// `line N: <original> -> <redacted>` per change, so the reporter can check their
/// evidence survived. `label` prefixes the location (`title line 1: ...`).
pub fn details(changes: &[LineChange], label: Option<&str>) -> Vec<String> {
    let prefix = match label {
        Some(label) if !label.is_empty() => format!("{label} line"),
        _ => "line".to_string(),
    };
    changes
        .iter()
        .map(|change| {
            format!(
                "{prefix} {}: {} -> {}",
                change.line_number,
                inline_truncate(&change.original),
                inline_truncate(&change.redacted)
            )
        })
        .collect()
}

/// Flattens newlines and caps the length, counted in UTF-16 units as JavaScript
/// counts it; a cut never splits a character, where JavaScript's may.
fn inline_truncate(text: &str) -> String {
    let flat = text.replace('\n', "\\n");
    if flat.encode_utf16().count() <= MAX_DETAIL_LENGTH {
        return flat;
    }
    let mut units = 0;
    let mut out: String = flat
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units < MAX_DETAIL_LENGTH
        })
        .collect();
    out.push('…');
    out
}

/// Placeholder assignments for one redaction pass, per category.
#[derive(Default)]
struct Placeholders(HashMap<&'static str, HashMap<String, String>>);

impl Placeholders {
    fn get(&mut self, category: &'static str, raw: &str) -> String {
        let assigned = self.0.entry(category).or_default();
        let next = assigned.len() + 1;
        assigned
            .entry(raw.to_string())
            .or_insert_with(|| format!("[{category}-{next}]"))
            .clone()
    }
}

// --- Patterns ---

/// JavaScript's `\s`: Unicode White_Space without U+0085, plus U+FEFF. Written
/// to go inside a character class.
const JS_SPACE: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    ) || ('\u{2000}'..='\u{200A}').contains(&c)
}

/// Compiles a JavaScript (non-`u`) pattern. `\d`, `\w` and `\b` are ASCII there
/// and Unicode in `regex`, and `\s` is a slightly different set, so they are
/// spelled out; `\/` is unescaped. Only the forms the patterns below use are
/// handled: no `\D`, `\S` or `\W` inside a class.
fn js(pattern: &str) -> Regex {
    let mut out = String::with_capacity(pattern.len() * 2);
    let mut in_class = false;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match (chars.next(), in_class) {
                (Some('d'), false) => out.push_str("[0-9]"),
                (Some('d'), true) => out.push_str("0-9"),
                (Some('D'), false) => out.push_str("[^0-9]"),
                (Some('w'), false) => out.push_str("[0-9A-Za-z_]"),
                (Some('w'), true) => out.push_str("0-9A-Za-z_"),
                (Some('s'), false) => out.push_str(&format!("[{JS_SPACE}]")),
                (Some('s'), true) => out.push_str(JS_SPACE),
                (Some('S'), false) => out.push_str(&format!("[^{JS_SPACE}]")),
                (Some('b'), false) => out.push_str(r"(?-u:\b)"),
                (Some('/'), _) => out.push('/'),
                (Some(e), _) => {
                    out.push('\\');
                    out.push(e);
                },
                (None, _) => out.push('\\'),
            },
            '[' if !in_class => {
                in_class = true;
                out.push(c);
            },
            ']' if in_class => {
                in_class = false;
                out.push(c);
            },
            _ => out.push(c),
        }
    }
    Regex::new(&out).unwrap_or_else(|e| panic!("redaction pattern {pattern:?}: {e}"))
}

/// JavaScript's `i` flag, for ASCII words: an alternation with each letter as a
/// two-case class. Without `u`, JavaScript folds only ASCII to ASCII, so this is
/// exact, and it needs no Unicode case tables, which this build's `regex` lacks.
fn any_case(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| {
            word.chars()
                .map(|c| {
                    if c.is_ascii_alphabetic() {
                        format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
                    } else {
                        c.to_string()
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("|")
}

const PUBLIC_HOST_ALLOWLIST: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "npmjs.org",
    "registry.npmjs.org",
    "npmjs.com",
    "deno.land",
    "jsr.io",
    "swamp-club.com",
    "api.swamp-club.com",
    "swamp.dev",
    "localhost",
    "example.com",
    "example.org",
    "example.net",
];

/// Credit card: 13-19 digit sequences with optional separators.
static CREDIT_CARD: LazyLock<Regex> = LazyLock::new(|| js(r"\b(?:\d[ -]*?){12,18}\d\b"));

/// SSN / national ID: XXX-XX-XXXX.
static SSN: LazyLock<Regex> = LazyLock::new(|| js(r"\b\d{3}-\d{2}-\d{4}\b"));

/// A `+` prefix or at least one separator is required, so bare digit runs like
/// batch IDs or offsets are not phone numbers.
static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    js(
        r"(?:\+\d{1,3}[\s.-]?)\(?\d{2,4}\)?[\s.-]?\d{3,4}[\s.-]?\d{3,4}\b|\(?\d{2,4}\)?[\s.-]\d{3,4}[\s.-]\d{3,4}\b",
    )
});

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    js(
        r"\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?)*\.[a-zA-Z]{2,}\b",
    )
});

static AWS_KEY: LazyLock<Regex> = LazyLock::new(|| js(yinz_types::secrets::AWS_KEY_PATTERN));

static GITHUB_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| js(yinz_types::secrets::GITHUB_TOKEN_PATTERN));

/// The TypeScript ends this with `(?=\s|$)`, checked by [`ends_at_space`]
/// instead. The separator is spaces and tabs only, so a match never crosses a
/// newline into the next line's first token.
static BEARER: LazyLock<Regex> = LazyLock::new(|| js(r"\bBearer[ \t]+([A-Za-z0-9_\-.~+/]+=*)"));

/// `sk_live_...`, `token-...` and the like, in any case.
static PREFIXED_KEY: LazyLock<Regex> = LazyLock::new(|| {
    js(&format!(
        r"\b(?:{})[-_][a-zA-Z0-9_\-]{{20,}}\b",
        any_case(&["sk", "pk", "rk", "ak", "key", "token", "secret"])
    ))
});

/// 40+ hex characters, like SHA tokens and API keys.
static LONG_HEX: LazyLock<Regex> = LazyLock::new(|| js(r"\b[0-9a-fA-F]{40,}\b"));

/// 32+ base64 characters, often tokens. The TypeScript's `(?=\s|$)` is checked
/// by [`ends_at_space`], as for [`BEARER`].
static LONG_BASE64: LazyLock<Regex> = LazyLock::new(|| js(r"\b[A-Za-z0-9+/]{32,}={0,2}"));

/// `NAME=value` where the name holds a sensitive keyword. The value is a quoted
/// span (so the interior can be redacted with its delimiters kept), an
/// `<angle-bracket placeholder>`, or an unquoted run that stops before a quote
/// or backtick: a bare `\S+` would consume the delimiter and break the
/// surrounding shell or YAML.
static ENV_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    js(&format!(
        r#"\b([A-Za-z_]*(?:{})[A-Za-z_]*)=("[^"\n]*"|'[^'\n]*'|`[^`\n]*`|<[^>\n]*>|[^\s"'`]+)"#,
        any_case(&[
            "PASSWORD",
            "SECRET",
            "TOKEN",
            "API_KEY",
            "APIKEY",
            "PRIVATE_KEY",
            "ACCESS_KEY",
            "AUTH",
        ])
    ))
});

/// Values with no secret in them, which must survive: already-masked values and
/// shell variable references — exactly the "before" side of masking-bug evidence.
static NON_SECRET_VALUE: LazyLock<Regex> = LazyLock::new(|| js(r"^(?:\*+|\$\{[^}]*\})$"));

/// A camelCase name whose only keyword is AUTH is a configuration identifier
/// (`authSource` in a URI query), not an env var. One holding a high-confidence
/// keyword ([`is_high_confidence_secret`]) is still redacted.
static CAMEL_CASE: LazyLock<Regex> = LazyLock::new(|| js(r"[a-z][A-Z]"));

fn is_high_confidence_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["PASSWORD", "SECRET", "TOKEN", "KEY"]
        .iter()
        .any(|k| upper.contains(k))
}

/// URLs with credentials. RFC 3986 forbids whitespace and `/` in userinfo, so
/// the user and password exclude them; otherwise the password could swallow
/// everything (across lines) up to an unrelated later `@`, like a `ws://host:port`
/// URL followed by an `@scope/package` mention.
static CONNECTION_STRING: LazyLock<Regex> = LazyLock::new(|| {
    js(r"(\w+:\/\/)([^:/?#@\s]+):([^@/\s]+)@([^/:?#\s]+)(:\d+)?(\/[^\s?#]*)?(\?[^\s#]*)?(#\S*)?")
});

static IPV4: LazyLock<Regex> = LazyLock::new(|| js(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"));

/// Simplified: at least two colons in a hex group sequence.
static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    js(
        r"\b(?:[0-9a-fA-F]{1,4}:){2,7}[0-9a-fA-F]{1,4}\b|::(?:[0-9a-fA-F]{1,4}:){0,5}[0-9a-fA-F]{1,4}\b|(?:[0-9a-fA-F]{1,4}:){1,6}::[0-9a-fA-F]{0,4}\b",
    )
});

/// The prefix and username are captured apart so the replacement is rebuilt
/// positionally: a username equal to a path component (`/home/home`) cannot
/// replace the wrong one.
static HOME_PATH: LazyLock<Regex> =
    LazyLock::new(|| js(r"(\/Users\/|\/home\/|C:\\Users\\|C:\/Users\/)([^\s/\\]+)"));

/// At least two dots; versions and allowlisted hosts are filtered afterwards.
static FQDN: LazyLock<Regex> =
    LazyLock::new(|| js(r"\b(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?\.){2,}[a-zA-Z]{2,}\b"));

static INTERNAL_HOST: LazyLock<Regex> = LazyLock::new(|| {
    js(
        r"\b[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?\.(?:internal|local|lan|corp|intranet|private|home)\b",
    )
});

static VERSION: LazyLock<Regex> = LazyLock::new(|| js(r"^\d+\.\d+\.\d+(?:\.\d+)?$"));

static TIMESTAMP_PART: LazyLock<Regex> = LazyLock::new(|| js(r"^\d{1,2}$"));

/// A key naming a digest, just before a long hex value: that value is a checksum,
/// not a secret.
static SAFE_HEX_KEY: LazyLock<Regex> = LazyLock::new(|| {
    js(&format!(
        r#"(?:"|\\")?(?:{})(?:"|\\")?[\s]*:[\s]*(?:"|\\")?$"#,
        any_case(&[
            "checksum",
            "sha256",
            "sha1",
            "sha512",
            "digest",
            "hash",
            "contentHash",
            "etag",
            "md5",
        ])
    ))
});

/// How far back, in UTF-16 units, [`SAFE_HEX_KEY`] looks.
const SAFE_HEX_KEY_WINDOW: usize = 40;

/// Common TLDs, sorted. They tell a hostname from a dotted code identifier:
/// `includes`, `get`, `env` and `length` are not TLDs.
const COMMON_TLDS: &[&str] = &[
    "ac", "ad", "ae", "af", "ag", "ai", "al", "am", "ao", "app", "ar", "as", "at", "au", "az",
    "ba", "be", "bg", "bh", "bi", "biz", "bj", "bn", "bo", "br", "bs", "bt", "bw", "by", "bz",
    "ca", "cat", "cc", "cd", "cf", "cg", "ch", "ci", "ck", "cl", "cloud", "cm", "cn", "co", "com",
    "cr", "cu", "cv", "cx", "cy", "cz", "de", "dev", "dj", "dk", "dm", "do", "dz", "ec", "edu",
    "ee", "eg", "er", "es", "et", "eu", "fi", "fj", "fk", "fm", "fo", "fr", "ga", "gd", "ge", "gg",
    "gh", "gi", "gl", "gm", "gn", "gov", "gp", "gq", "gr", "gs", "gt", "gu", "gw", "gy", "hk",
    "hm", "hn", "hr", "ht", "hu", "id", "ie", "il", "im", "in", "info", "int", "io", "iq", "ir",
    "is", "it", "je", "jm", "jo", "jobs", "jp", "ke", "kg", "kh", "ki", "km", "kn", "kp", "kr",
    "kw", "ky", "kz", "la", "land", "lb", "lc", "li", "lk", "lr", "ls", "lt", "lu", "lv", "ly",
    "ma", "mc", "md", "me", "mg", "mh", "mil", "mk", "ml", "mm", "mn", "mo", "mp", "mq", "mr",
    "ms", "mt", "mu", "museum", "mv", "mw", "mx", "my", "mz", "na", "name", "nc", "ne", "net",
    "nf", "ng", "ni", "nl", "no", "np", "nr", "nu", "nz", "om", "onion", "org", "pa", "page", "pe",
    "pf", "pg", "ph", "pk", "pl", "pm", "pn", "pr", "pro", "ps", "pt", "pw", "py", "qa", "re",
    "ro", "rs", "ru", "run", "rw", "sa", "sb", "sc", "sd", "se", "sg", "sh", "si", "site", "sk",
    "sl", "sm", "sn", "so", "sr", "ss", "st", "su", "sv", "sx", "sy", "sz", "tc", "td", "tel",
    "tf", "tg", "th", "tj", "tk", "tl", "tm", "tn", "to", "today", "tr", "tt", "tv", "tw", "tz",
    "ua", "ug", "uk", "us", "uy", "uz", "va", "vc", "ve", "vg", "vi", "vn", "vu", "website", "wf",
    "ws", "xxx", "ye", "yt", "za", "zm", "zw",
];

// --- Checks ---

fn is_luhn_valid(digits: &str) -> bool {
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let n = u32::from(b.wrapping_sub(b'0'));
            if i % 2 == 1 {
                let doubled = n * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                n
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn is_public_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    PUBLIC_HOST_ALLOWLIST.iter().any(|allowed| {
        lower == *allowed
            || lower
                .strip_suffix(allowed)
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

fn is_safe_ipv4(ip: &str) -> bool {
    if ip == "0.0.0.0" || ip == "255.255.255.255" {
        return true;
    }
    let parts: Vec<Option<u32>> = ip.split('.').map(|p| p.parse().ok()).collect();
    let [Some(a), Some(b), Some(c), _] = parts[..] else {
        return false;
    };
    // 127.0.0.0/8 loopback, 169.254.0.0/16 link-local, then the RFC 5737
    // documentation ranges.
    a == 127
        || (a == 169 && b == 254)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
}

fn is_safe_ipv6(ip: &str) -> bool {
    let lower = ip.to_ascii_lowercase();
    lower == "::1" || lower.starts_with("fe80:")
}

/// `HH:MM:SS` also matches the IPv6 pattern.
fn is_likely_timestamp(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 || !parts.iter().all(|p| TIMESTAMP_PART.is_match(p)) {
        return false;
    }
    let n: Vec<u32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    matches!(n[..], [h, m, s] if h <= 23 && m <= 59 && s <= 59)
}

/// The TypeScript's `(?=\s|$)`. Rejecting here and resuming after the match finds
/// what the lookahead would: the match's run of token characters is maximal, so
/// no shorter or later match inside it can end anywhere else.
fn ends_at_space(source: &str, end: usize) -> bool {
    source[end..].chars().next().is_none_or(is_js_space)
}

/// Up to [`SAFE_HEX_KEY_WINDOW`] UTF-16 units of `source` before `offset`.
fn preceding(source: &str, offset: usize) -> &str {
    let mut units = 0;
    let mut start = offset;
    for (i, c) in source[..offset].char_indices().rev() {
        units += c.len_utf16();
        if units > SAFE_HEX_KEY_WINDOW {
            break;
        }
        start = i;
    }
    &source[start..offset]
}

// --- Passes ---

fn apply(text: &str, placeholders: &mut Placeholders, summary: &mut Summary) -> String {
    // Order matters: more specific patterns first, then broader ones.

    // 1. Connection strings, before general host matching.
    let text = CONNECTION_STRING
        .replace_all(text, |c: &Captures| {
            summary.add(Category::Secret);
            summary.add(Category::Hostname);
            let host = placeholders.get("HOST", &c[4]);
            let part = |i| c.get(i).map_or("", |m| m.as_str());
            format!(
                "{}[REDACTED-USER]:***@{host}{}{}{}{}",
                &c[1],
                part(5),
                part(6),
                part(7),
                part(8)
            )
        })
        .into_owned();

    // 2. Env-var secrets. A quoted value is redacted inside its delimiters;
    //    masked (`***`) and `${VAR}` values pass untouched. Distinct values get
    //    distinct placeholders, so before/after evidence (a masked-vs-unmasked
    //    table) keeps its contrast instead of collapsing to identical rows.
    let text = ENV_SECRET
        .replace_all(&text, |c: &Captures| {
            let (whole, name, value) = (&c[0], &c[1], &c[2]);
            let quote = match value.chars().next() {
                Some('<') => return whole.to_string(),
                Some(q @ ('"' | '\'' | '`')) => Some(q),
                _ => None,
            };
            let inner = match quote {
                Some(_) => &value[1..value.len() - 1],
                None => value,
            };
            if inner.is_empty()
                || NON_SECRET_VALUE.is_match(inner)
                || (CAMEL_CASE.is_match(name) && !is_high_confidence_secret(name))
            {
                return whole.to_string();
            }
            summary.add(Category::Secret);
            let placeholder = placeholders.get("REDACTED-SECRET", inner);
            let quote = quote.map(String::from).unwrap_or_default();
            format!("{name}={quote}{placeholder}{quote}")
        })
        .into_owned();

    // 3. AWS keys, 4. GitHub tokens.
    let mut secret = |c: &Captures| {
        summary.add(Category::Secret);
        placeholders.get("REDACTED-SECRET", &c[0])
    };
    let text = AWS_KEY.replace_all(&text, &mut secret).into_owned();
    let text = GITHUB_TOKEN.replace_all(&text, &mut secret).into_owned();

    // 5. Bearer tokens.
    let text = BEARER
        .replace_all(&text, |c: &Captures| {
            if !ends_at_space(&text, c.get(0).map_or(0, |m| m.end())) {
                return c[0].to_string();
            }
            summary.add(Category::Secret);
            format!("Bearer {}", placeholders.get("REDACTED-SECRET", &c[1]))
        })
        .into_owned();

    // 6. Prefixed API keys.
    let text = PREFIXED_KEY
        .replace_all(&text, |c: &Captures| {
            summary.add(Category::Secret);
            placeholders.get("REDACTED-SECRET", &c[0])
        })
        .into_owned();

    // 7. Email addresses.
    let text = EMAIL
        .replace_all(&text, |_: &Captures| {
            summary.add(Category::Email);
            "[REDACTED-EMAIL]"
        })
        .into_owned();

    // 8. SSNs.
    let text = SSN
        .replace_all(&text, |_: &Captures| {
            summary.add(Category::NationalId);
            "[REDACTED-ID]"
        })
        .into_owned();

    // 9. Credit cards, Luhn-validated to cut false positives.
    let text = CREDIT_CARD
        .replace_all(&text, |c: &Captures| {
            let digits: String = c[0]
                .chars()
                .filter(|&ch| ch != '-' && !is_js_space(ch))
                .collect();
            if (13..=19).contains(&digits.chars().count()) && is_luhn_valid(&digits) {
                summary.add(Category::CreditCard);
                return "[REDACTED-CC]".to_string();
            }
            c[0].to_string()
        })
        .into_owned();

    // 10. Phone numbers. At least one structural character beyond digits and
    //     whitespace is required, so numeric data like status codes survives.
    let text = PHONE
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            if !m.contains(['+', '(', ')', '.', '-']) {
                return m.to_string();
            }
            let digits = m.chars().filter(char::is_ascii_digit).count();
            if (7..=15).contains(&digits) {
                summary.add(Category::PhoneNumber);
                return "[REDACTED-PHONE]".to_string();
            }
            m.to_string()
        })
        .into_owned();

    // 11. Home directory usernames.
    let text = HOME_PATH
        .replace_all(&text, |c: &Captures| {
            summary.add(Category::HomeDirectoryPath);
            format!("{}[REDACTED]", &c[1])
        })
        .into_owned();

    // 12. IPv6, before IPv4 so a mapped address is not half-matched.
    let text = IPV6
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            if is_safe_ipv6(m) || is_likely_timestamp(m) {
                return m.to_string();
            }
            summary.add(Category::IpAddress);
            placeholders.get("IP", m)
        })
        .into_owned();

    // 13. IPv4. Exactly four octets, so a three-part version like 1.23.4 never
    //     matches.
    let text = IPV4
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            if is_safe_ipv4(m) {
                return m.to_string();
            }
            summary.add(Category::IpAddress);
            placeholders.get("IP", m)
        })
        .into_owned();

    // 14. Internal hostnames (.internal, .local, .lan, .corp, ...).
    let text = INTERNAL_HOST
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            if is_public_host(m) {
                return m.to_string();
            }
            summary.add(Category::Hostname);
            placeholders.get("HOST", m)
        })
        .into_owned();

    // 15. FQDNs. One followed by `(` is a method call, not a host.
    let text = FQDN
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            let end = c.get(0).map_or(0, |m| m.end());
            let tld = m.rsplit('.').next().unwrap_or(m).to_ascii_lowercase();
            if is_public_host(m)
                || VERSION.is_match(m)
                || text[end..].starts_with('(')
                || COMMON_TLDS.binary_search(&tld.as_str()).is_err()
            {
                return m.to_string();
            }
            summary.add(Category::Hostname);
            placeholders.get("HOST", m)
        })
        .into_owned();

    // 16. Long hex strings, after every specific pattern.
    let text = LONG_HEX
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            let start = c.get(0).map_or(0, |m| m.start());
            if SAFE_HEX_KEY.is_match(preceding(&text, start)) {
                return m.to_string();
            }
            summary.add(Category::Secret);
            placeholders.get("REDACTED-SECRET", m)
        })
        .into_owned();

    // 17. Long base64 strings, when mixed-case.
    LONG_BASE64
        .replace_all(&text, |c: &Captures| {
            let m = &c[0];
            let end = c.get(0).map_or(0, |m| m.end());
            if ends_at_space(&text, end)
                && m.contains(|ch: char| ch.is_ascii_lowercase())
                && m.contains(|ch: char| ch.is_ascii_uppercase())
            {
                summary.add(Category::Secret);
                return placeholders.get("REDACTED-SECRET", m);
            }
            m.to_string()
        })
        .into_owned()
}

fn diff_lines(original: &str, redacted: &str) -> Vec<LineChange> {
    if original == redacted {
        return Vec::new();
    }
    let before: Vec<&str> = original.split('\n').collect();
    let after: Vec<&str> = redacted.split('\n').collect();
    if before.len() == after.len() {
        return before
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(_, (b, a))| b != a)
            .map(|(i, (b, a))| LineChange {
                line_number: i + 1,
                original: b.to_string(),
                redacted: a.to_string(),
            })
            .collect();
    }
    // A redaction consumed newlines: align the matching head and tail lines and
    // report the middle as one block.
    let head = before
        .iter()
        .zip(&after)
        .take_while(|(b, a)| b == a)
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(b, a)| b == a)
        .count();
    vec![LineChange {
        line_number: head + 1,
        original: before[head..before.len() - tail].join("\n"),
        redacted: after[head..after.len() - tail].join("\n"),
    }]
}

#[cfg(test)]
#[path = "unit/redact.rs"]
mod tests;
