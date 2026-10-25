use super::*;
use hegel::generators;

/// Splits a `serialize()` output (with no flag prefix) into its index levels and its text
/// body. The body is digits and commas only, so the last `$` always ends the indices.
fn split_serialized(serialized: &str) -> (Vec<Vec<usize>>, &str) {
    let (indices, body) = match serialized.rsplit_once('$') {
        Some((indices, body)) => (indices, body),
        None => return (vec![], serialized),
    };
    let levels = indices
        .split('$')
        .map(|level| {
            if level.is_empty() {
                vec![]
            } else {
                level
                    .split(',')
                    .map(|i| i.parse::<usize>().expect("index is not a number"))
                    .collect()
            }
        })
        .collect();
    (levels, body)
}

/// Independent decoder for the serialized text body: parse the comma separated numbers back
/// into bytes and rebuild the string.
fn decode_body(body: &str) -> Result<String, String> {
    if body.is_empty() {
        return Ok(String::new());
    }
    let mut bytes = Vec::new();
    for part in body.split(',') {
        bytes.push(
            part.parse::<u8>()
                .map_err(|e| format!("{:?}: {}", part, e))?,
        );
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Arbitrary text paired with an arbitrary substring. Half the time the substring is a real
/// slice of the text so the substring searches actually find something. The empty substring
/// is included: it must match nowhere rather than loop forever.
#[hegel::composite]
fn text_and_substring(tc: &hegel::TestCase) -> (String, String) {
    let text = tc.draw(generators::text());
    let char_count = text.chars().count();
    let from_text = tc.draw(generators::booleans());
    let substring = if from_text && char_count > 0 {
        let start = tc.draw(generators::integers::<usize>().max_value(char_count - 1));
        let end = tc.draw(
            generators::integers::<usize>()
                .min_value(start + 1)
                .max_value(char_count),
        );
        text.chars().skip(start).take(end - start).collect()
    } else {
        tc.draw(generators::text())
    };
    (text, substring)
}

#[hegel::test]
fn serialized_body_decodes_back_to_the_original_text(tc: hegel::TestCase) {
    let text = tc.draw(generators::text());
    let serialized = Text::new(&text).serialize();
    let (levels, body) = split_serialized(&serialized);
    assert!(
        levels.is_empty(),
        "unexpected index levels {:?} for {:?}",
        levels,
        text
    );
    assert_eq!(
        decode_body(body),
        Ok(text.clone()),
        "body {:?} did not decode back to {:?}",
        body,
        text
    );
}

#[hegel::test]
fn substring_indices_stay_within_the_text(tc: hegel::TestCase) {
    let (text, substring) = tc.draw(text_and_substring());
    let len = Text::new(&text).len();
    for (name, serialized) in [
        (
            "dim_substring",
            Text::new(&text).dim_substring(&substring).serialize(),
        ),
        (
            "color_substring",
            Text::new(&text).color_substring(0, &substring).serialize(),
        ),
        (
            "color_nth_substring",
            Text::new(&text)
                .color_nth_substring(0, &substring, 0)
                .serialize(),
        ),
    ] {
        let (levels, _) = split_serialized(&serialized);
        for index in levels.iter().flatten() {
            assert!(
                *index < len,
                "{} put index {} outside of {:?} (len {}) for substring {:?}",
                name,
                index,
                text,
                len,
                substring
            );
        }
    }
}

#[hegel::test]
fn substring_helpers_never_panic(tc: hegel::TestCase) {
    let (text, substring) = tc.draw(text_and_substring());
    let occurrence = tc.draw(generators::integers::<usize>());
    let helpers: Vec<Text> = vec![
        Text::new(&text).dim_substring(&substring),
        Text::new(&text).unbold_substring(&substring),
        Text::new(&text).error_color_substring(&substring),
        Text::new(&text).success_color_substring(&substring),
        Text::new(&text).color_substring(0, &substring),
        Text::new(&text).color_nth_substring(0, &substring, occurrence),
        Text::new(&text).error_color_nth_substring(&substring, occurrence),
        Text::new(&text).success_color_nth_substring(&substring, occurrence),
        Text::new(&text).color_last_substring(0, &substring),
        Text::new(&text).error_color_last_substring(&substring),
        Text::new(&text).success_color_last_substring(&substring),
    ];
    for text_element in &helpers {
        let _ = text_element.serialize();
    }
}

#[hegel::test]
fn arbitrary_indices_serialize_without_panicking(tc: hegel::TestCase) {
    let text = tc.draw(generators::text());
    let indices = tc.draw(generators::vecs(generators::integers::<usize>()));
    let level = tc.draw(generators::integers::<usize>().max_value(64));
    let serialized = Text::new(&text)
        .color_indices(level, indices.clone())
        .dim_indices(indices.clone())
        .error_color_indices(indices.clone())
        .serialize();
    let (levels, body) = split_serialized(&serialized);
    assert_eq!(
        decode_body(body),
        Ok(text.clone()),
        "indices {:?} at level {} corrupted the body of {:?}",
        indices,
        level,
        text
    );
    assert!(
        levels.len() > level,
        "level {} missing from {:?} for text {:?}",
        level,
        levels,
        text
    );
}

fn text_body(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn disabled_flag_serializes_with_a_d_prefix() {
    let serialized = Text::new("x").disabled().serialize();
    assert_eq!(serialized, format!("d{}", text_body("x")));
}

#[test]
fn opaque_and_disabled_serialize_in_push_order() {
    let serialized = Text::new("x").disabled().opaque().serialize();
    assert_eq!(serialized, format!("zd{}", text_body("x")));
}

#[test]
fn all_flags_serialize_in_selected_opaque_disabled_order() {
    let serialized = Text::new("x").disabled().opaque().selected().serialize();
    assert_eq!(serialized, format!("xzd{}", text_body("x")));
}

#[test]
fn flags_do_not_disturb_indices_or_text() {
    let serialized = Text::new("Foo bar baz")
        .disabled()
        .color_indices(0, vec![0, 1, 2])
        .serialize();
    assert!(serialized.starts_with('d'));
    assert!(serialized.contains("0,1,2$"));
    assert!(serialized.ends_with(&text_body("Foo bar baz")));
}

/// An excluded start bound excludes that index. `a..b` cannot spell one, but
/// `(Bound::Excluded(n), _)` can, and striking `n` itself would be off by one.
#[test]
fn an_excluded_start_bound_leaves_its_own_index_alone() {
    let text = Text::new("abcde").strike_range((Bound::Excluded(2), Bound::Unbounded));
    assert_eq!(text.indices.get(8), Some(&vec![3, 4]));
}

/// The empty substring used to spin forever: `find("")` returns `Some(0)` and the cursor
/// advanced by `substr.len()` == 0.
#[test]
fn empty_substring_matches_nothing() {
    let serialized = Text::new("Foo bar baz").dim_substring("").serialize();
    assert_eq!(serialized, text_body("Foo bar baz"), "no indices expected");
}
