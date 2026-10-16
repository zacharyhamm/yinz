//! Pure, bounded text operations. Indices always count Unicode scalar values.
use std::sync::{Arc, Mutex};

use regex::{Captures, Regex, RegexBuilder};
use steel::{
    rvals::{Custom, FromSteelVal, IntoSteelVal, Result, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};

const MAX: usize = 16 * 1024 * 1024;
const MAX_RESULTS: usize = 100_000;

#[derive(Clone)]
struct Pattern(Regex);
impl Custom for Pattern {}

#[derive(Clone, Default)]
struct StringBuilder(Arc<Mutex<String>>);
impl Custom for StringBuilder {}

fn error(message: impl std::fmt::Display) -> steel::rerrs::SteelErr {
    crate::scheme_error(message)
}

fn bounded(size: usize) -> Result<()> {
    if size > MAX {
        Err(error("text operation exceeds 16 MiB budget"))
    } else {
        Ok(())
    }
}

fn text(value: &SteelVal) -> Result<&str> {
    match value {
        SteelVal::StringV(value) => {
            bounded(value.len())?;
            Ok(value.as_str())
        },
        _ => Err(error("expected a string")),
    }
}

fn arity(args: &[SteelVal], min: usize, max: usize) -> Result<()> {
    if (min..=max).contains(&args.len()) {
        Ok(())
    } else {
        Err(error(format!("expected {min} to {max} arguments")))
    }
}

fn index(value: &SteelVal) -> Result<usize> {
    match value {
        SteelVal::IntV(value) if *value >= 0 => Ok(*value as usize),
        _ => Err(error("expected a nonnegative character index")),
    }
}

fn offset(text: &str, index: usize) -> Result<usize> {
    text.char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .nth(index)
        .ok_or_else(|| error("character index out of range"))
}

fn compile(args: &[SteelVal]) -> Result<SteelVal> {
    arity(args, 1, 2)?;
    let pattern = text(&args[0])?;
    if pattern.len() > 64 * 1024 {
        return Err(error("regex pattern exceeds 64 KiB"));
    }
    let mut builder = RegexBuilder::new(pattern);
    builder.size_limit(MAX).dfa_size_limit(MAX);
    if let Some(flags) = args.get(1) {
        for flag in text(flags)?.chars() {
            match flag {
                'i' => builder.case_insensitive(true),
                'm' => builder.multi_line(true),
                's' => builder.dot_matches_new_line(true),
                _ => return Err(error(format!("unsupported regex flag: {flag}"))),
            };
        }
    }
    Pattern(builder.build().map_err(error)?).into_steelval()
}

fn match_value(
    captures: &Captures<'_>,
    start: usize,
    end: usize,
    remaining: &mut usize,
) -> Result<SteelVal> {
    let bytes = captures
        .iter()
        .map(|capture| capture.map_or(8, |value| value.len().saturating_add(8)))
        .sum::<usize>()
        .saturating_add(64);
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or_else(|| error("regex results exceed 16 MiB budget"))?;
    let groups: Vec<_> = captures
        .iter()
        .map(|capture| {
            capture.map_or(SteelVal::BoolV(false), |value| {
                SteelVal::StringV(value.as_str().into())
            })
        })
        .collect();
    let mut map = steel::HashMap::new();
    map.insert(
        SteelVal::StringV("start".into()),
        SteelVal::IntV(start as isize),
    );
    map.insert(
        SteelVal::StringV("end".into()),
        SteelVal::IntV(end as isize),
    );
    map.insert(
        SteelVal::StringV("groups".into()),
        SteelVal::ListV(groups.into()),
    );
    Ok(SteelVal::HashMapV(steel::gc::Gc::new(map).into()))
}

fn find(pattern: &Pattern, source: &str, all: bool) -> Result<SteelVal> {
    bounded(source.len())?;
    let mut results = Vec::new();
    let mut remaining = MAX;
    let (mut previous_byte, mut previous_char) = (0, 0);
    for captures in pattern.0.captures_iter(source) {
        let matched = captures
            .get(0)
            .ok_or_else(|| error("missing full regex match"))?;
        let start = previous_char + source[previous_byte..matched.start()].chars().count();
        let end = start + matched.as_str().chars().count();
        let value = match_value(&captures, start, end, &mut remaining)?;
        if !all {
            return Ok(value);
        }
        if results.len() == MAX_RESULTS {
            return Err(error("regex results exceed 100000 matches"));
        }
        results.push(value);
        previous_byte = matched.end();
        previous_char = end;
    }
    Ok(if all {
        SteelVal::ListV(results.into())
    } else {
        SteelVal::BoolV(false)
    })
}

fn append(output: &mut String, part: &str) -> Result<()> {
    bounded(output.len().saturating_add(part.len()))?;
    output.push_str(part);
    Ok(())
}

// Expand captures incrementally so a small replacement cannot allocate an unbounded string.
fn expand(output: &mut String, template: &str, captures: &Captures<'_>) -> Result<()> {
    let mut rest = template;
    while let Some(dollar) = rest.find('$') {
        append(output, &rest[..dollar])?;
        rest = &rest[dollar + 1..];
        if let Some(tail) = rest.strip_prefix('$') {
            append(output, "$")?;
            rest = tail;
            continue;
        }
        let (name, tail) = if let Some(braced) = rest.strip_prefix('{') {
            if let Some(end) = braced.find('}') {
                (&braced[..end], &braced[end + 1..])
            } else {
                append(output, "$")?;
                continue;
            }
        } else {
            let end = rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if end == 0 {
                append(output, "$")?;
                continue;
            }
            (&rest[..end], &rest[end..])
        };
        let capture = name
            .parse::<usize>()
            .map_or_else(|_| captures.name(name), |index| captures.get(index));
        if let Some(capture) = capture {
            append(output, capture.as_str())?;
        }
        rest = tail;
    }
    append(output, rest)
}

fn replace(pattern: &Pattern, source: String, replacement: String) -> Result<String> {
    bounded(source.len())?;
    bounded(replacement.len())?;
    let mut output = String::new();
    let mut previous = 0;
    for captures in pattern.0.captures_iter(&source) {
        let matched = captures
            .get(0)
            .ok_or_else(|| error("missing full regex match"))?;
        append(&mut output, &source[previous..matched.start()])?;
        expand(&mut output, &replacement, &captures)?;
        previous = matched.end();
    }
    append(&mut output, &source[previous..])?;
    Ok(output)
}

fn contains(args: &[SteelVal]) -> Result<SteelVal> {
    arity(args, 2, 3)?;
    let source = text(&args[0])?;
    let needle = text(&args[1])?;
    let start = args.get(2).map(index).transpose()?.unwrap_or(0);
    let byte = offset(source, start)?;
    Ok(source[byte..]
        .find(needle)
        .map_or(SteelVal::BoolV(false), |found| {
            SteelVal::IntV((start + source[byte..byte + found].chars().count()) as isize)
        }))
}

fn split(args: &[SteelVal]) -> Result<SteelVal> {
    arity(args, 1, 2)?;
    let source = text(&args[0])?;
    let parts: Box<dyn Iterator<Item = &str> + '_> = match args.get(1) {
        Some(separator) => Box::new(source.split(text(separator)?)),
        None => Box::new(source.split_whitespace()),
    };
    let mut values = Vec::new();
    for part in parts {
        if values.len() == MAX_RESULTS {
            return Err(error("string-split exceeds 100000 fields"));
        }
        values.push(SteelVal::StringV(part.into()));
    }
    Ok(SteelVal::ListV(values.into()))
}

fn copy(args: &[SteelVal]) -> Result<SteelVal> {
    arity(args, 1, 3)?;
    let source = text(&args[0])?;
    let start = args.get(1).map(index).transpose()?.unwrap_or(0);
    let start = offset(source, start)?;
    let end = args
        .get(2)
        .map(index)
        .transpose()?
        .map(|end| offset(source, end))
        .transpose()?
        .unwrap_or(source.len());
    source
        .get(start..end)
        .map(|value| SteelVal::StringV(value.into()))
        .ok_or_else(|| error("string-copy end precedes start"))
}

fn pad_left(args: &[SteelVal]) -> Result<SteelVal> {
    arity(args, 2, 3)?;
    let source = text(&args[0])?;
    let width = index(&args[1])?;
    let fill = args
        .get(2)
        .map(char::from_steelval)
        .transpose()?
        .unwrap_or(' ');
    let count = width.saturating_sub(source.chars().count());
    bounded(
        count
            .saturating_mul(fill.len_utf8())
            .saturating_add(source.len()),
    )?;
    let mut output = String::with_capacity(count * fill.len_utf8() + source.len());
    output.extend(std::iter::repeat_n(fill, count));
    output.push_str(source);
    Ok(SteelVal::StringV(output.into()))
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_value("regex", SteelVal::FuncV(compile));
    engine.register_fn("regex?", |value: SteelVal| {
        matches!(value, SteelVal::Custom(value) if value.read().as_any_ref().is::<Pattern>())
    });
    engine.register_fn(
        "regex-match?",
        |pattern: &Pattern, source: String| -> Result<bool> {
            bounded(source.len())?;
            Ok(pattern.0.is_match(&source))
        },
    );
    engine.register_fn("regex-find", |pattern: &Pattern, source: String| {
        find(pattern, &source, false)
    });
    engine.register_fn("regex-find-all", |pattern: &Pattern, source: String| {
        find(pattern, &source, true)
    });
    engine.register_fn("regex-replace-all", replace);
    engine.register_fn("regex-escape", |source: String| -> Result<String> {
        // Each input byte needs at most one escape prefix.
        bounded(source.len().saturating_mul(2))?;
        Ok(regex::escape(&source))
    });
    engine.register_fn("char-upper-case?", char::is_uppercase);
    engine.register_fn("char-alphabetic?", char::is_alphabetic);
    engine.register_value("string-contains", SteelVal::FuncV(contains));
    engine.register_value("string-search-forward", SteelVal::FuncV(contains));
    engine.register_value("string-split", SteelVal::FuncV(split));
    engine.register_value("string-copy", SteelVal::FuncV(copy));
    engine.register_value("string-pad-left", SteelVal::FuncV(pad_left));
    engine.register_fn("string-builder", StringBuilder::default);
    engine.register_fn(
        "string-builder-append!",
        |builder: &StringBuilder, source: String| -> Result<()> {
            append(&mut *builder.0.lock().map_err(error)?, &source)
        },
    );
    engine.register_fn(
        "string-builder->string",
        |builder: &StringBuilder| -> Result<String> {
            Ok(builder.0.lock().map_err(error)?.clone())
        },
    );
}

#[cfg(test)]
#[path = "unit/text.rs"]
mod tests;
