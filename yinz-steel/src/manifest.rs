//! A reader-level scan for a plugin's pane declaration. The plugin bridge runs it in the server,
//! before it schedules the plugin, so it never parses, expands or evaluates source.
//!
//! It has its own reader rather than Steel's. Steel's lexer interns every identifier, string and
//! number it reads into process-global tables that are never freed, which would make the
//! long-lived server grow with every load, and it reads some literals (long here-string
//! delimiters, long numbers) in superlinear time. This reader looks at each character a bounded
//! number of times, interns nothing, and allocates only a bracket stack and the arguments of a
//! declaration form. It reads only as much of Steel's syntax as finding the live top-level forms
//! needs: comments, strings, here-strings, `|symbol|`s, characters, numbers, lists and prefixes,
//! following the pinned lexer's rules for where each token ends.
//!
//! It enforces Steel's limits on a source's size, tokens and nesting with the same accounting
//! and words as the evaluator's own check, because they decide whether the plugin runs at all:
//! an over-limit plugin fails to load with that error, rather than being given up as one that
//! declares no pane before anything evaluates it.
//!
//! Source that Steel's reader rejects (an unterminated string, comment or list, an invalid
//! escape, mismatched brackets, ...) reports no declaration error, whatever comes before the bad
//! text: evaluation then reports the real error. Its declaration is the one the top-level forms
//! before the bad text make, if they make exactly one valid one, so that a plugin with a pane
//! keeps it while its author fixes a syntax error, and the error shows in the pane.

use anyhow::{anyhow, Result};
use std::borrow::Cow;
use std::collections::BTreeMap;

const DECLARE: &str = "yinz-declare-pane";
const DECLARE_UNLESS: &str = "yinz-declare-pane-unless";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneDeclaration {
    None,
    Always,
    /// Declared unless the load configuration maps `key` to exactly `value`.
    Unless {
        key: String,
        value: String,
    },
}

impl PaneDeclaration {
    pub fn applies(&self, configuration: &BTreeMap<String, String>) -> bool {
        match self {
            Self::None => false,
            Self::Always => true,
            Self::Unless { key, value } => configuration.get(key) != Some(value),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::Always => "always".to_owned(),
            Self::Unless { key, value } => format!("unless {key}={value}"),
        }
    }
}

/// Read the declaration from the literal top-level forms of an entry file.
///
/// Source that Steel cannot read never gives an error, so that the reader's own error, which
/// evaluation reports, is not hidden behind a declaration error. It declares what the complete
/// top-level forms before the unreadable text declare.
pub fn pane_declaration(source: &str) -> Result<PaneDeclaration> {
    Ok(read_declaration(source)?.0)
}

/// [`pane_declaration`] of source Steel's reader can read, `None` for source it cannot.
///
/// For a caller that compares the declaration with another one: the declaration of unreadable
/// source is a guess, and evaluation reports the source's real error, which a mismatch of the
/// guess would otherwise hide.
pub fn readable_pane_declaration(source: &str) -> Result<Option<PaneDeclaration>> {
    let (declaration, readable) = read_declaration(source)?;
    Ok(readable.then_some(declaration))
}

/// The declaration, and whether Steel's reader can read all of `source`.
fn read_declaration(source: &str) -> Result<(PaneDeclaration, bool)> {
    let mut found = Found::default();
    let readable = match scan(source, &mut found) {
        Ok(()) => true,
        Err(Stop::Limit(limit)) => return Err(anyhow!("{limit}")),
        Err(Stop::Unreadable) => false,
    };
    let declaration = match found.error {
        Some(error) if readable => return Err(error),
        Some(_) => PaneDeclaration::None,
        None => found
            .declaration
            .map_or(PaneDeclaration::None, |(_, declaration)| declaration),
    };
    Ok((declaration, readable))
}

/// What the top-level forms read so far declare.
#[derive(Default)]
struct Found {
    /// The declaration, with its line.
    declaration: Option<(usize, PaneDeclaration)>,
    /// The first malformed or second declaration.
    error: Option<anyhow::Error>,
}

/// Text Steel's reader rejects.
#[derive(Debug, PartialEq, Eq)]
struct Unreadable;

type Read<T> = std::result::Result<T, Unreadable>;

/// Why a scan ended before the end of the source.
enum Stop {
    Unreadable,
    /// One of Steel's source limits, in the evaluator's words.
    Limit(String),
}

impl From<Unreadable> for Stop {
    fn from(_: Unreadable) -> Self {
        Self::Unreadable
    }
}

/// Steel's limits on tokens and nesting, counted as the evaluator's own source check counts them: every
/// token, comments too, toward the token limit; and toward nesting, one more for each open list
/// than the prefixes in front of it, plus the prefixes still waiting for a datum.
#[derive(Default)]
struct Limits {
    tokens: usize,
    /// What each open list added to `depth`. Never deeper than the nesting limit.
    nesting: Vec<usize>,
    depth: usize,
    prefixes: usize,
}

impl Limits {
    fn tokens(&mut self, count: usize) -> std::result::Result<(), Stop> {
        self.tokens += count;
        if self.tokens > crate::MAX_SOURCE_TOKENS {
            return Err(Stop::Limit("Steel source exceeds token limit".to_owned()));
        }
        Ok(())
    }

    fn token(&mut self, token: &Token) -> std::result::Result<(), Stop> {
        match token {
            Token::Open { .. } => {
                self.nesting.push(self.prefixes + 1);
                self.depth += self.prefixes + 1;
                self.prefixes = 0;
            },
            Token::Close(_) => {
                self.depth -= self.nesting.pop().unwrap_or_default();
                self.prefixes = 0;
            },
            Token::Prefix(_) => self.prefixes += 1,
            _ => self.prefixes = 0,
        }
        if self.depth + self.prefixes > crate::MAX_SOURCE_NESTING {
            return Err(Stop::Limit(format!(
                "Steel source nesting exceeds {}",
                crate::MAX_SOURCE_NESTING
            )));
        }
        Ok(())
    }
}

/// Reads `source` up to its end, or up to text Steel's reader rejects or a limit, into `found`.
fn scan(source: &str, found: &mut Found) -> std::result::Result<(), Stop> {
    if source.len() > crate::MAX_SOURCE_BYTES {
        return Err(Stop::Limit("Steel source exceeds 8 MiB".to_owned()));
    }
    let mut limits = Limits::default();
    let mut reader = Reader::new(source);
    let mut lines = Lines::default();
    // The closing bracket of each open list, innermost last.
    let mut closes: Vec<u8> = Vec::new();
    // Quotes and datum comments waiting for their datum, innermost list last.
    let mut prefixes: Vec<Prefix> = Vec::new();
    // For each open list that has prefixes waiting outside it, its depth and how many: those
    // wait for the list to end. A list without an entry shares the count of the one around it,
    // so the stacks stay one byte per open list however deep the nesting.
    let mut outside: Vec<(usize, usize)> = Vec::new();
    // The live top-level list being read, while it may still be a declaration.
    let mut form: Option<Form> = None;
    loop {
        let next = reader.next();
        // The comments before the token, and the token, even one Steel's lexer rejects.
        let comments = std::mem::take(&mut reader.comments);
        limits.tokens(comments + usize::from(!matches!(next, Ok(None))))?;
        let Some((start, token)) = next? else {
            break;
        };
        limits.token(&token)?;
        let datum = match token {
            Token::Prefix(prefix) => {
                prefixes.push(prefix);
                continue;
            },
            Token::Open { close, list } => {
                if closes.is_empty() {
                    form = (list && prefixes.is_empty()).then(|| Form {
                        line: lines.at(source, start),
                        head: None,
                        arguments: Vec::new(),
                    });
                }
                if prefixes.len() > waiting(&outside) {
                    outside.push((closes.len(), prefixes.len()));
                }
                closes.push(close);
                continue;
            },
            Token::Close(close) => {
                // A quote or datum comment needs a datum before the list ends.
                if closes.pop() != Some(close) || prefixes.len() > waiting(&outside) {
                    return Err(Stop::Unreadable);
                }
                if outside
                    .last()
                    .is_some_and(|(depth, _)| *depth == closes.len())
                {
                    outside.pop();
                }
                Datum::List
            },
            Token::Word { end, escaped } => Datum::Word {
                start,
                end,
                escaped,
            },
            Token::String { end, body } => Datum::String { start, end, body },
            Token::Other => Datum::Other,
        };
        let settled = settle(&mut prefixes, waiting(&outside));
        match closes.len() {
            0 => {
                // Only a list can be the form being read, which ends here.
                if !matches!(datum, Datum::List) {
                    continue;
                }
                let Some(form) = form.take() else {
                    continue;
                };
                if found.error.is_some() {
                    continue;
                }
                match form.declaration() {
                    Ok(None) => {},
                    Ok(Some(declaration)) => {
                        if let Some((first, _)) = found.declaration {
                            found.error = Some(anyhow!(
                                "line {}: the pane is declared more than once (first on line \
                                 {first})",
                                form.line
                            ));
                        } else {
                            found.declaration = Some((form.line, declaration));
                        }
                    },
                    Err(declaration_error) => found.error = Some(declaration_error),
                }
            },
            1 => {
                let Some(quoted) = settled else {
                    // A datum comment discarded it.
                    continue;
                };
                let Some(current) = form.as_mut() else {
                    continue;
                };
                if current.head.is_none() {
                    current.head = (!quoted).then(|| reader.head(&datum)).flatten();
                    if current.head.is_none() {
                        form = None;
                    }
                } else if current.arguments.len() < 3 {
                    let argument = match datum {
                        Datum::String { start, end, body }
                            if !quoted && current.arguments.len() < 2 =>
                        {
                            Argument::String(match body {
                                Some((start, end)) => source[start..end].to_owned(),
                                None => decode_string(&source[start..end]),
                            })
                        },
                        _ => Argument::Other,
                    };
                    current.arguments.push(argument);
                }
            },
            _ => {},
        }
    }
    if !closes.is_empty() || !prefixes.is_empty() {
        return Err(Stop::Unreadable);
    }
    Ok(())
}

/// How many of the waiting prefixes belong outside the innermost open list.
fn waiting(outside: &[(usize, usize)]) -> usize {
    outside.last().map_or(0, |(_, count)| *count)
}

/// A datum ends: pending quotes since `base` wrap it and the nearest datum comment discards it.
/// Returns `None` for a discarded datum, otherwise whether it was quoted.
fn settle(prefixes: &mut Vec<Prefix>, base: usize) -> Option<bool> {
    let mut quoted = false;
    while prefixes.len() > base {
        match prefixes.pop() {
            Some(Prefix::Comment) => return None,
            _ => quoted = true,
        }
    }
    Some(quoted)
}

#[derive(Clone, Copy)]
enum Prefix {
    Quote,
    Comment,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Head {
    Declare,
    Unless,
}

enum Argument {
    String(String),
    Other,
}

/// A live top-level list, with what it holds directly once its head names a declaration.
struct Form {
    line: usize,
    head: Option<Head>,
    /// At most three: a third already makes either declaration invalid.
    arguments: Vec<Argument>,
}

impl Form {
    fn declaration(&self) -> Result<Option<PaneDeclaration>> {
        let line = self.line;
        match (self.head, self.arguments.as_slice()) {
            (None, _) => Ok(None),
            (Some(Head::Declare), []) => Ok(Some(PaneDeclaration::Always)),
            (Some(Head::Declare), _) => Err(anyhow!("line {line}: ({DECLARE}) takes no arguments")),
            (Some(Head::Unless), [Argument::String(key), Argument::String(value)]) => {
                Ok(Some(PaneDeclaration::Unless {
                    key: key.clone(),
                    value: value.clone(),
                }))
            },
            (Some(Head::Unless), _) => Err(anyhow!(
                "line {line}: ({DECLARE_UNLESS} \"<key>\" \"<value>\") takes two string literals"
            )),
        }
    }
}

/// A finished datum, by where its text is when its value may matter.
enum Datum {
    List,
    Word {
        start: usize,
        end: usize,
        escaped: bool,
    },
    /// A string literal: `body` is its text when it has no escapes to decode.
    String {
        start: usize,
        end: usize,
        body: Option<(usize, usize)>,
    },
    Other,
}

enum Token {
    Open {
        close: u8,
        /// False for a vector or bytevector literal, which is never a call.
        list: bool,
    },
    Close(u8),
    Prefix(Prefix),
    /// A symbol, keyword or symbol-shaped word, ending at `end`. `escaped` for `|..|`.
    Word {
        end: usize,
        escaped: bool,
    },
    /// A string literal ending at `end`: `".."`, or a here-string, whose value is its `body`.
    String {
        end: usize,
        body: Option<(usize, usize)>,
    },
    /// Any other atom: numbers, characters, booleans.
    Other,
}

/// Line numbers of increasing byte offsets, counting each newline once.
#[derive(Default)]
struct Lines {
    counted: usize,
    line: usize,
}

impl Lines {
    fn at(&mut self, source: &str, offset: usize) -> usize {
        let offset = offset.max(self.counted);
        self.line += source.as_bytes()[self.counted..offset]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        self.counted = offset;
        self.line + 1
    }
}

/// The characters, besides digits, that Steel's lexer keeps reading a number through.
const NUMBER_CHARS: &str = "+-./@aAbBcCdDeEfFin";

struct Reader<'a> {
    text: &'a str,
    at: usize,
    /// The second half of a word that Steel's lexer splits after a leading `+`.
    queued: Option<(usize, Token)>,
    /// Comments skipped since this was last taken: Steel counts them as tokens.
    comments: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Self {
        let mut reader = Self::inside(text);
        // A shebang line is skipped, up to its newline.
        if text.starts_with("#!") {
            reader.at = text.find('\n').unwrap_or(text.len());
        }
        reader
    }

    /// A reader of text inside a token, such as a literal's body.
    fn inside(text: &'a str) -> Self {
        Self {
            text,
            at: 0,
            queued: None,
            comments: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, expected: char) -> bool {
        let matched = self.peek() == Some(expected);
        if matched {
            self.at += expected.len_utf8();
        }
        matched
    }

    /// The next token and where it starts, skipping whitespace and comments.
    fn next(&mut self) -> Read<Option<(usize, Token)>> {
        if let Some(queued) = self.queued.take() {
            return Ok(Some(queued));
        }
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            let start = self.at;
            let Some(c) = self.bump() else {
                return Ok(None);
            };
            let token = match c {
                ';' => {
                    match self.text[self.at..].find('\n') {
                        Some(newline) => self.at += newline + 1,
                        None => self.at = self.text.len(),
                    }
                    self.comments += 1;
                    continue;
                },
                '"' => {
                    if !self.quoted('"', None)?.closed {
                        return Err(Unreadable);
                    }
                    Token::String {
                        end: self.at,
                        body: None,
                    }
                },
                '(' => Token::Open {
                    close: b')',
                    list: true,
                },
                '[' => Token::Open {
                    close: b']',
                    list: true,
                },
                '{' => Token::Open {
                    close: b'}',
                    list: true,
                },
                ')' => Token::Close(b')'),
                ']' => Token::Close(b']'),
                '}' => Token::Close(b'}'),
                '\'' | '`' => Token::Prefix(Prefix::Quote),
                ',' => {
                    self.eat('@');
                    Token::Prefix(Prefix::Quote)
                },
                '+' | '-' | '.' => self.number(start)?,
                '#' => match self.peek() {
                    Some('x' | 'X' | 'd' | 'D' | 'o' | 'O' | 'b' | 'B') => {
                        self.bump();
                        self.number(start)?
                    },
                    Some('|') => {
                        self.bump();
                        self.block_comment()?;
                        self.comments += 1;
                        continue;
                    },
                    Some(';') => {
                        self.bump();
                        Token::Prefix(Prefix::Comment)
                    },
                    Some('#') => return Err(Unreadable),
                    Some('<') => {
                        self.bump();
                        if self.eat('<') {
                            let body = self.here_string()?;
                            Token::String {
                                end: self.at,
                                body: Some(body),
                            }
                        } else {
                            self.word(start)?
                        }
                    },
                    _ => self.hash(start)?,
                },
                c if c.is_ascii_digit() => {
                    self.at = start;
                    self.number(start)?
                },
                _ => {
                    self.at = start;
                    self.word(start)?
                },
            };
            return Ok(Some((start, token)));
        }
    }

    /// Nested `#| .. |#`, after its opening `#|`.
    fn block_comment(&mut self) -> Read<()> {
        let mut depth = 1usize;
        while let Some(c) = self.bump() {
            match c {
                '|' if self.eat('#') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                },
                '#' if self.eat('|') => depth += 1,
                _ => {},
            }
        }
        Err(Unreadable)
    }

    /// `#<<DELIMITER`, after its `#<<`: the delimiter runs to the end of the line, and the
    /// string to the delimiter's next appearance. Returns where the string's text is.
    fn here_string(&mut self) -> Read<(usize, usize)> {
        let line_start = self.at;
        let mut returns = false;
        let mut line_end = None;
        while let Some(c) = self.bump() {
            match c {
                '\n' => {
                    line_end = Some(self.at - 1);
                    break;
                },
                '\r' => returns = true,
                c if c.is_whitespace() => return Err(Unreadable),
                _ => {},
            }
        }
        let line = &self.text[line_start..line_end.unwrap_or(self.at)];
        // Steel skips carriage returns in the delimiter.
        let delimiter = if returns {
            Cow::Owned(line.replace('\r', ""))
        } else {
            Cow::Borrowed(line)
        };
        let body = self.at;
        if delimiter.is_empty() {
            // Steel's search ends after the first character of the body.
            self.bump().ok_or(Unreadable)?;
            return Ok((body, self.at));
        }
        let found = self.text[body..]
            .find(delimiter.as_ref())
            .ok_or(Unreadable)?;
        self.at = body + found + delimiter.len();
        Ok((body, body + found))
    }

    /// The rest of a `".."` string or `|..|` symbol, after its opening `delimiter`, up to and
    /// including its closing one. Unescaped characters go to `out` when given.
    fn quoted(&mut self, delimiter: char, mut out: Option<&mut String>) -> Read<Quoted> {
        let mut escapes = false;
        while let Some(c) = self.bump() {
            match c {
                c if c == delimiter => {
                    return Ok(Quoted {
                        closed: true,
                        escapes,
                    })
                },
                '\\' => {
                    escapes = true;
                    let unescaped = self.escape(delimiter)?;
                    if let (Some(out), Some(unescaped)) = (out.as_deref_mut(), unescaped) {
                        out.push(unescaped);
                    }
                },
                c => {
                    if let Some(out) = out.as_deref_mut() {
                        out.push(c);
                    }
                },
            }
        }
        Ok(Quoted {
            closed: false,
            escapes,
        })
    }

    /// One escape, after its backslash: the character it stands for, or `None` for a line
    /// continuation.
    fn escape(&mut self, delimiter: char) -> Read<Option<char>> {
        let c = self.bump().ok_or(Unreadable)?;
        Ok(Some(match c {
            '"' => '"',
            'a' => '\x07',
            'f' => '\x0c',
            'b' => '\x08',
            '\\' => '\\',
            '|' => '|',
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            '0' => '\0',
            'x' | 'u' => {
                let end = if c == 'u' && self.eat('{') { '}' } else { ';' };
                let digits = self.at;
                loop {
                    let c = self.bump().ok_or(Unreadable)?;
                    if c == end {
                        break;
                    }
                    if matches!(c, ';' | '\\' | '\n' | '(' | ')' | '[' | ']' | '{' | '}')
                        || c == delimiter
                    {
                        return Err(Unreadable);
                    }
                }
                let digits = &self.text[digits..self.at - end.len_utf8()];
                let code = u32::from_str_radix(digits, 16).map_err(|_| Unreadable)?;
                char::from_u32(code).ok_or(Unreadable)?
            },
            ' ' | '\t' | '\n' => {
                // Spaces, then one newline unless the escape was one, then indentation.
                let mut trimming = c == '\n';
                loop {
                    match self.peek().ok_or(Unreadable)? {
                        ' ' | '\t' => {
                            self.bump();
                        },
                        '\n' if !trimming => {
                            self.bump();
                            trimming = true;
                        },
                        _ if trimming => return Ok(None),
                        _ => return Err(Unreadable),
                    }
                }
            },
            _ => return Err(Unreadable),
        }))
    }

    /// A token starting at `start` that Steel's lexer reads as a number when it can: digits and
    /// number characters up to a delimiter. Anything else in it makes it a word.
    fn number(&mut self, start: usize) -> Read<Token> {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || NUMBER_CHARS.contains(c) {
                self.bump();
            } else if matches!(c, '(' | ')' | '[' | ']') || c.is_whitespace() {
                break;
            } else {
                return self.word(start);
            }
        }
        if number(&self.text[start..self.at])? {
            Ok(Token::Other)
        } else {
            self.word(start)
        }
    }

    /// A `#` token that is not a number, comment, datum comment or here-string, after its `#`.
    fn hash(&mut self, start: usize) -> Read<Token> {
        while let Some(c) = self.peek() {
            match c {
                '\\' => {
                    self.bump();
                    self.bump();
                },
                '\'' | '`' => {
                    self.bump();
                    break;
                },
                ',' => {
                    self.bump();
                    self.eat('@');
                    break;
                },
                '(' | '[' | ')' | ']' => break,
                c if c.is_whitespace() => break,
                _ => {
                    self.bump();
                },
            }
        }
        let text = &self.text[start..self.at];
        Ok(match text {
            "#true" | "#t" | "#false" | "#f" => Token::Other,
            "#'" | "#`" | "#," | "#,@" => Token::Prefix(Prefix::Quote),
            _ if text.starts_with("#:") => Token::Other,
            _ if text.starts_with("#\\") => {
                character(text)?;
                Token::Other
            },
            "#" | "#u8" if self.eat('(') => Token::Open {
                close: b')',
                list: false,
            },
            _ => self.word(start)?,
        })
    }

    /// The rest of a word that starts at `start`: a `|..|` symbol when it continues with `|`,
    /// otherwise everything up to a delimiter.
    fn word(&mut self, start: usize) -> Read<Token> {
        let escaped = self.eat('|');
        if escaped {
            self.quoted('|', None)?;
        } else {
            while let Some(c) = self.peek() {
                match c {
                    '(' | '[' | ')' | ']' | '{' | '}' | '\'' | '"' | '`' | ';' | ',' => break,
                    c if c.is_whitespace() => break,
                    '\\' => {
                        self.bump();
                        self.bump();
                    },
                    _ => {
                        self.bump();
                    },
                }
            }
        }
        let word = &self.text[start..self.at];
        // Steel reads a word that starts with `+` as `+` and then the rest.
        if word.len() > 1 && word.starts_with('+') {
            self.queued = Some((
                start + 1,
                Token::Word {
                    end: self.at,
                    escaped: false,
                },
            ));
            return Ok(Token::Word {
                end: start + 1,
                escaped: false,
            });
        }
        // A `|..|` symbol must start and end with a bar: one inside another word does not.
        if escaped && !(word.len() > 1 && word.starts_with('|') && word.ends_with('|')) {
            return Err(Unreadable);
        }
        Ok(Token::Word {
            end: self.at,
            escaped,
        })
    }

    /// The declaration a list's first datum names, if any.
    fn head(&self, datum: &Datum) -> Option<Head> {
        let Datum::Word {
            start,
            end,
            escaped,
        } = *datum
        else {
            return None;
        };
        let word = &self.text[start..end];
        let name = if escaped {
            decode_symbol(word)
        } else {
            Cow::Borrowed(word)
        };
        match name.as_ref() {
            DECLARE => Some(Head::Declare),
            DECLARE_UNLESS => Some(Head::Unless),
            _ => None,
        }
    }
}

struct Quoted {
    closed: bool,
    escapes: bool,
}

/// The value of a `".."` literal the reader has already accepted.
fn decode_string(literal: &str) -> String {
    let mut value = String::new();
    let mut reader = Reader::inside(&literal[1..]);
    let _ = reader.quoted('"', Some(&mut value));
    value
}

/// The name a `|..|` symbol the reader has already accepted stands for: its text between the
/// bars, with escapes applied when it has any that leave something.
fn decode_symbol(word: &str) -> Cow<'_, str> {
    let mut value = String::new();
    let mut reader = Reader::inside(&word[1..]);
    let escapes = reader
        .quoted('|', Some(&mut value))
        .is_ok_and(|quoted| quoted.escapes);
    if escapes && !value.is_empty() {
        Cow::Owned(value)
    } else {
        Cow::Borrowed(&word[1..word.len() - 1])
    }
}

/// Whether a `#\` character literal is one Steel accepts.
fn character(text: &str) -> Read<()> {
    const NAMES: [&str; 9] = [
        "alarm",
        "backspace",
        "delete",
        "escape",
        "newline",
        "null",
        "return",
        "space",
        "tab",
    ];
    let rest = text
        .get(2..)
        .filter(|rest| !rest.is_empty())
        .ok_or(Unreadable)?;
    if NAMES.iter().any(|name| rest.eq_ignore_ascii_case(name)) {
        return Ok(());
    }
    let first = rest.as_bytes()[0];
    if !(matches!(first, b'u' | b'x') && text.len() > 3) {
        let mut chars = rest.chars();
        return match (chars.next(), chars.next()) {
            (Some(_), None) => Ok(()),
            _ => Err(Unreadable),
        };
    }
    let payload = if first == b'u' && rest.as_bytes().get(1) == Some(&b'{') {
        if !rest.ends_with('}') {
            return Err(Unreadable);
        }
        &rest[2..rest.len() - 1]
    } else {
        &rest[1..]
    };
    let code = u32::from_str_radix(payload, 16).map_err(|_| Unreadable)?;
    char::from_u32(code).map(|_| ()).ok_or(Unreadable)
}

/// How Steel's lexer reads a word made only of digits and number characters: `true` as a
/// number, `false` as a symbol. A rational with a zero denominator is a lexer error. Each check
/// is a pass over the text; no value is computed, so a long number costs only its length.
fn number(text: &str) -> Read<bool> {
    let (text, radix) = match text.get(0..2) {
        Some("#x" | "#X") => (&text[2..], 16),
        Some("#d" | "#D") => (&text[2..], 10),
        Some("#o" | "#O") => (&text[2..], 8),
        Some("#b" | "#B") => (&text[2..], 2),
        _ => (text, 10),
    };
    let imaginary = |part: &str| {
        if part == "+" || part == "-" {
            Some(Real::Number)
        } else {
            real(part, radix)
        }
    };
    let parts = if let Some((magnitude, angle)) = text.split_once('@') {
        real(magnitude, radix).zip(real(angle, radix))
    } else {
        match complex_parts(text) {
            Some((Part::Real(real_part), None)) => {
                real(real_part, radix).map(|r| (r, Real::Number))
            },
            Some((Part::Imaginary(part), None)) if part.starts_with(['+', '-']) => {
                imaginary(part).map(|i| (i, Real::Number))
            },
            Some((Part::Real(real_part), Some(Part::Imaginary(part)))) => {
                real(real_part, radix).zip(imaginary(part))
            },
            _ => None,
        }
    };
    match parts {
        None => Ok(false),
        Some((Real::ZeroDenominator, _) | (_, Real::ZeroDenominator)) => Err(Unreadable),
        Some(_) => Ok(true),
    }
}

#[derive(Clone, Copy)]
enum Real {
    Number,
    /// A rational whose denominator is zero.
    ZeroDenominator,
}

enum Part<'a> {
    Real(&'a str),
    /// The text before its trailing `i`.
    Imaginary(&'a str),
}

/// A number's text split at its sign changes, as Steel splits a complex number: one part, or a
/// real and an imaginary part. `None` for more than two signs.
fn complex_parts(text: &str) -> Option<(Part<'_>, Option<Part<'_>>)> {
    fn part(text: &str) -> Part<'_> {
        match text.strip_suffix('i') {
            Some(imaginary) => Part::Imaginary(imaginary),
            None => Part::Real(text),
        }
    }
    let mut signs = [0usize; 2];
    let mut count = 0;
    let mut chars = text.char_indices();
    while let Some((index, c)) = chars.next() {
        if c == '+' || c == '-' {
            if count == 2 {
                return None;
            }
            signs[count] = index;
            count += 1;
        } else if c == 'e' || c == 'E' {
            // A sign after an exponent marker belongs to the exponent.
            chars.next();
        }
    }
    match signs[..count] {
        [] | [0] => Some((part(text), None)),
        [index] | [0, index] => Some((part(&text[..index]), Some(part(&text[index..])))),
        _ => None,
    }
}

fn real(text: &str, radix: u32) -> Option<Real> {
    if matches!(text, "-inf.0" | "+inf.0" | "+nan.0" | "-nan.0") {
        return Some(Real::Number);
    }
    let (mut dot, mut exponent, mut fraction) = (false, false, None);
    for (index, c) in text.char_indices() {
        match c {
            'e' | 'E' if radix < 15 => {
                if exponent {
                    return None;
                }
                exponent = true;
            },
            '/' => {
                if fraction.is_some() {
                    return None;
                }
                fraction = Some(index);
            },
            '.' => {
                if dot {
                    return None;
                }
                dot = true;
            },
            _ => {},
        }
    }
    if exponent || dot {
        return (radix == 10 && text.parse::<f64>().is_ok()).then_some(Real::Number);
    }
    let Some(slash) = fraction else {
        return integer(text, radix).map(|_| Real::Number);
    };
    integer(&text[..slash], radix)?;
    Some(if integer(&text[slash + 1..], radix)? {
        Real::ZeroDenominator
    } else {
        Real::Number
    })
}

/// Whether Steel reads `text` as an integer in `radix`, as `num_bigint` parses one: an optional
/// sign, then digits. `Some(true)` for zero.
fn integer(text: &str, radix: u32) -> Option<bool> {
    let mut digits = text;
    if let Some(rest) = digits.strip_prefix('-')
        && !rest.starts_with('+')
    {
        digits = rest;
    }
    if let Some(rest) = digits.strip_prefix('+')
        && !rest.starts_with('+')
    {
        digits = rest;
    }
    let valid = !digits.is_empty()
        && digits
            .chars()
            .all(|c| c.to_digit(36).is_some_and(|digit| digit < radix));
    valid.then(|| digits.bytes().all(|byte| byte == b'0'))
}

#[cfg(test)]
#[path = "unit/manifest.rs"]
mod tests;
